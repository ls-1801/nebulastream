/*
    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        https://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

#include <EmitOperatorHandler.hpp>

#include <algorithm>
#include <barrier>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <initializer_list>
#include <map>
#include <memory>
#include <random>
#include <ranges>
#include <set>
#include <source_location>
#include <thread>
#include <tuple>
#include <unordered_map>
#include <utility>
#include <vector>
#include <DataTypes/DataType.hpp>
#include <Identifiers/Identifiers.hpp>
#include <Identifiers/NESStrongType.hpp>
#include <MemoryLayout/RowLayout.hpp>
#include <Nautilus/Interface/BufferRef/RowTupleBufferRef.hpp>
#include <Nautilus/Interface/RecordBuffer.hpp>
#include <Runtime/AbstractBufferProvider.hpp>
#include <Runtime/BufferManager.hpp>
#include <Runtime/Execution/OperatorHandler.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <Sequencing/SequenceData.hpp>
#include <Sequencing/SequenceNumber.hpp>
#include <Util/Logger/LogLevel.hpp>
#include <Util/Logger/Logger.hpp>
#include <Util/Logger/impl/NesLogger.hpp>
#include <fmt/format.h>
#include <folly/Synchronized.h>
#include <gtest/gtest.h>
#include <BaseUnitTest.hpp>
#include <EmitPhysicalOperator.hpp>
#include <ErrorHandling.hpp>
#include <ExecutionContext.hpp>
#include <PipelineExecutionContext.hpp>

namespace NES
{

class EmitPhysicalOperatorTest : public Testing::BaseUnitTest
{
    struct MockedPipelineContext final : PipelineExecutionContext
    {
        bool emitBuffer(const TupleBuffer& buffer, ContinuationPolicy) override
        {
            buffers.wlock()->emplace_back(buffer);
            return true;
        }

        TupleBuffer allocateTupleBuffer() override { return bufferManager->getBufferBlocking(); }

        [[nodiscard]] WorkerThreadId getId() const override { return INITIAL<WorkerThreadId>; }

        [[nodiscard]] uint64_t getNumberOfWorkerThreads() const override { return 1; }

        [[nodiscard]] std::shared_ptr<AbstractBufferProvider> getBufferManager() const override { return bufferManager; }

        [[nodiscard]] PipelineId getPipelineId() const override { return PipelineId(1); }

        std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& getOperatorHandlers() override
        {
            return *operatorHandlers;
        }

        void setOperatorHandlers(std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& opHandlers) override
        {
            operatorHandlers = &opHandlers;
        }

        MockedPipelineContext(folly::Synchronized<std::vector<TupleBuffer>>& buffers, std::shared_ptr<BufferManager> bufferManager)
            : buffers(buffers), bufferManager(std::move(bufferManager))
        {
        }

        void repeatTask(const TupleBuffer&, std::chrono::milliseconds) override { INVARIANT(false, "This function should not be called"); }

        ///NOLINTNEXTLINE(cppcoreguidelines-avoid-const-or-ref-data-members) lifetime is ensured by the `run` method.
        folly::Synchronized<std::vector<TupleBuffer>>& buffers;
        std::shared_ptr<BufferManager> bufferManager;
        std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>* operatorHandlers = nullptr;
    };

public:
    static void SetUpTestSuite()
    {
        Logger::setupLogging("EmitPhysicalOperatorTest.log", LogLevel::LOG_DEBUG);
        NES_DEBUG("Setup EmitPhysicalOperatorTest test class.");
    }

    void SetUp() override
    {
        BaseUnitTest::SetUp();
        reset();
    }

    EmitPhysicalOperator createUUT()
    {
        auto schema = Schema{}.addField("A_FIELD", DataType::Type::UINT32);
        auto layout = std::make_shared<RowLayout>(512, schema);
        EmitPhysicalOperator emit{OperatorHandlerId(0), std::make_shared<RowTupleBufferRef>(layout)};
        handlers.insert_or_assign(OperatorHandlerId(0), std::make_shared<EmitOperatorHandler>());
        return emit;
    }

    void run(const std::function<void(ExecutionContext&, RecordBuffer&)>& test, TupleBuffer buffer)
    {
        MockedPipelineContext pec{buffers, bm};
        pec.setOperatorHandlers(handlers);
        Arena arena(bm);

        ExecutionContext executionContext{&pec, &arena};
        executionContext.sequenceRangePtr = buffer.getSequenceRangePtr();
        executionContext.originId = buffer.getOriginId();

        RecordBuffer recordBuffer(std::addressof(buffer));
        test(executionContext, recordBuffer);
    }

    ///NOLINTBEGIN(fuchsia-default-arguments-declarations)

    void checkNumberOfBuffers(size_t numberOfBuffers, std::source_location location = std::source_location::current())
    {
        const testing::ScopedTrace scopedTrace(location.file_name(), static_cast<int>(location.line()), "checkNumberOfBuffers");
        EXPECT_EQ(buffers.rlock()->size(), numberOfBuffers) << fmt::format("expects {} buffers to be emitted", numberOfBuffers);
    }

    void checkBufferAt(
        size_t index,
        SequenceRange expectedRange,
        OriginId originId = INITIAL<OriginId>,
        size_t numberOfTuples = 0,
        std::source_location location = std::source_location::current())
    {
        const testing::ScopedTrace scopedTrace(location.file_name(), static_cast<int>(location.line()), "checkBufferAt");
        ASSERT_GE(buffers.rlock()->size(), index) << fmt::format("Index out of bound when checking buffer at {}", index);
        EXPECT_EQ(buffers.rlock()->at(index).getNumberOfTuples(), numberOfTuples) << fmt::format("Expected {} tuples", numberOfTuples);
        EXPECT_EQ(buffers.rlock()->at(index).getSequenceRange(), expectedRange)
            << fmt::format("Expected SequenceRange {}", expectedRange);
        EXPECT_EQ(buffers.rlock()->at(index).getOriginId(), OriginId(originId)) << fmt::format("Expected OriginId {}", originId);
    }

    void checkForDups(std::source_location location = std::source_location::current())
    {
        const testing::ScopedTrace scopedTrace(location.file_name(), static_cast<int>(location.line()), "checkForDups");
        auto uniqueRanges = (*buffers.rlock())
            | std::views::transform([](const auto& buffer)
                                    { return buffer.getSequenceRange(); })
            | std::ranges::to<std::set>();

        EXPECT_EQ(buffers.rlock()->size(), uniqueRanges.size()) << "Received duplicate sequence ranges";
    }

    /// Checks that for each root sequence number, the emitted sub-ranges form a contiguous
    /// partition from [N, N+1). This replaces the old chunk-based termination check.
    void checkRangesComplete(std::source_location location = std::source_location::current())
    {
        const testing::ScopedTrace scopedTrace(location.file_name(), static_cast<int>(location.line()), "checkRangesComplete");
        /// Group ranges by root sequence number
        std::map<size_t, std::vector<SequenceRange>> rangesByRoot;
        for (const auto& buffer : *buffers.rlock())
        {
            const auto& range = buffer.getSequenceRange();
            rangesByRoot[range.rootSequence()].push_back(range);
        }

        for (auto& [root, ranges] : rangesByRoot)
        {
            /// Sort ranges by start
            std::ranges::sort(ranges, [](const SequenceRange& a, const SequenceRange& b) { return a.start < b.start; });

            /// Check that the first range starts at SequenceNumber(root) (or a child thereof)
            /// and the last range ends at SequenceNumber(root + 1)
            EXPECT_EQ(ranges.back().end, SequenceNumber(root + 1))
                << fmt::format("Root sequence {}: last sub-range does not end at {}", root, root + 1);

            /// Check that ranges are contiguous (each range's end == next range's start)
            for (size_t i = 0; i + 1 < ranges.size(); ++i)
            {
                EXPECT_EQ(ranges[i].end, ranges[i + 1].start)
                    << fmt::format("Root sequence {}: gap between sub-ranges at index {}", root, i);
            }
        }
    }

    TupleBuffer createBuffer(
        SequenceRange range,
        OriginId originId = INITIAL<OriginId>,
        size_t numberOfTuples = 0)
    {
        auto buffer = bm->getBufferBlocking();
        buffer.setNumberOfTuples(numberOfTuples);
        buffer.setSequenceRange(range);
        buffer.setOriginId(originId);

        return buffer;
    }

    ///NOLINTEND(fuchsia-default-arguments-declarations)

    void reset() { buffers.wlock()->clear(); }

    folly::Synchronized<std::vector<TupleBuffer>> buffers;
    std::shared_ptr<BufferManager> bm = BufferManager::create(512, 100000);
    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>> handlers;

    std::random_device rd;
};

TEST_F(EmitPhysicalOperatorTest, BasicTest)
{
    auto buffer = createBuffer(SequenceRange(SequenceNumber(1), SequenceNumber(2)));
    EmitPhysicalOperator emit = createUUT();

    run(
        [&](auto& executionContext, auto& recordBuffer)
        {
            emit.open(executionContext, recordBuffer);
            emit.close(executionContext, recordBuffer);
        },
        buffer);

    checkBufferAt(0, SequenceRange(SequenceNumber(1), SequenceNumber(2)));
    checkForDups();
    checkRangesComplete();
}

TEST_F(EmitPhysicalOperatorTest, ChunkNumberTest)
{
    /// 5 sub-ranges that partition [1, 2): [1, 1.1), [1.1, 1.2), [1.2, 1.3), [1.3, 1.4), [1.4, 2)
    std::vector<TupleBuffer> inputBuffers;
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({1, 1}), SequenceNumber({1, 2}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({1, 2}), SequenceNumber({1, 3}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({1, 3}), SequenceNumber({1, 4}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({1, 4}), SequenceNumber(2))));


    bool hasMorePermutations = true;
    while (hasMorePermutations)
    {
        reset();
        EmitPhysicalOperator emit = createUUT();
        for (auto& buffer : inputBuffers)
        {
            run(
                [&](auto& executionContext, auto& recordBuffer)
                {
                    emit.open(executionContext, recordBuffer);
                    emit.close(executionContext, recordBuffer);
                },
                buffer);
        }
        checkNumberOfBuffers(5);
        checkForDups();
        checkRangesComplete();

        hasMorePermutations = std::ranges::next_permutation(
                                  inputBuffers,
                                  std::less{},
                                  [](const TupleBuffer& buffer)
                                  { return SequenceData(buffer.getSequenceRange()); })
                                  .found;
    }
}

/// Tests if all permutations result in a sane set of sub-ranges.
/// This means every root sequence number should have contiguous sub-ranges that cover its full range, and no duplicates.
TEST_F(EmitPhysicalOperatorTest, SequenceChunkNumberTest)
{
    std::vector<TupleBuffer> inputBuffers;
    /// Sequence 1: 3 sub-ranges partitioning [1, 2)
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({1, 1}), SequenceNumber({1, 2}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({1, 2}), SequenceNumber(2))));

    /// Sequence 2: 2 sub-ranges partitioning [2, 3)
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber(2), SequenceNumber({2, 1}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({2, 1}), SequenceNumber(3))));

    /// Sequence 3: 3 sub-ranges partitioning [3, 4)
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber(3), SequenceNumber({3, 1}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({3, 1}), SequenceNumber({3, 2}))));
    inputBuffers.emplace_back(createBuffer(SequenceRange(SequenceNumber({3, 2}), SequenceNumber(4))));

    bool hasMorePermutations = true;
    while (hasMorePermutations)
    {
        reset();
        EmitPhysicalOperator emit = createUUT();
        for (auto& buffer : inputBuffers)
        {
            run(
                [&](auto& executionContext, auto& recordBuffer)
                {
                    emit.open(executionContext, recordBuffer);
                    emit.close(executionContext, recordBuffer);
                },
                buffer);
        }
        checkNumberOfBuffers(8);
        checkForDups();
        checkRangesComplete();
        hasMorePermutations = std::ranges::next_permutation(
                                  inputBuffers,
                                  std::less{},
                                  [](const TupleBuffer& buffer)
                                  { return SequenceData(buffer.getSequenceRange()); })
                                  .found;
    };
}

TEST_F(EmitPhysicalOperatorTest, ConcurrentSequenceChunkNumberTest)
{
    for (auto [numberOfSequences, maxChunksPerSequence, numberOfThreads] :
         std::initializer_list<std::tuple<size_t, size_t, size_t>>{{2, 10, 2}, {1000, 2, 4}, {10, 100, 4}, {1000, 20, 10}})
    {
        reset();
        std::vector<TupleBuffer> inputBuffers;
        for (size_t seq = 0; seq < numberOfSequences; seq++)
        {
            auto seqBase = seq + 1;
            std::uniform_int_distribution<size_t> chunkDist(2, maxChunksPerSequence);
            auto numChunks = chunkDist(rd);
            /// Generate sub-ranges partitioning [seqBase, seqBase+1)
            /// Using child sequence numbers: [seqBase, seqBase.1), [seqBase.1, seqBase.2), ..., [seqBase.(N-1), seqBase+1)
            for (size_t chunk = 0; chunk < numChunks - 1; chunk++)
            {
                auto subStart = (chunk == 0) ? SequenceNumber(seqBase) : SequenceNumber(seqBase).child(chunk);
                auto subEnd = SequenceNumber(seqBase).child(chunk + 1);
                inputBuffers.emplace_back(createBuffer(SequenceRange(subStart, subEnd)));
            }
            auto lastStart = SequenceNumber(seqBase).child(numChunks - 1);
            inputBuffers.emplace_back(createBuffer(SequenceRange(lastStart, SequenceNumber(seqBase + 1))));
        }

        EmitPhysicalOperator emit = createUUT();
        std::ranges::shuffle(inputBuffers, rd);
        std::barrier<> barrier(static_cast<int>(numberOfThreads) + 1);
        std::vector<std::jthread> threads;
        threads.reserve(numberOfThreads);
        for (size_t threadId = 0; threadId < numberOfThreads; threadId++)
        {
            threads.emplace_back(
                [threadId, &inputBuffers, this, &emit, &barrier, numberOfThreads]()
                {
                    barrier.arrive_and_wait();
                    for (size_t index = threadId; index < inputBuffers.size(); index += numberOfThreads)
                    {
                        run(
                            [&](auto& executionContext, auto& recordBuffer)
                            {
                                emit.open(executionContext, recordBuffer);
                                emit.close(executionContext, recordBuffer);
                            },
                            inputBuffers.at(index));
                    }
                });
        }
        barrier.arrive_and_wait();
        threads.clear();

        checkNumberOfBuffers(inputBuffers.size());
        checkForDups();
        checkRangesComplete();
    }
}
}
