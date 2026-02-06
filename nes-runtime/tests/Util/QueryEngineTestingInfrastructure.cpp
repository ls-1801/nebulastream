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

#include <QueryEngineTestingInfrastructure.hpp>

#include <algorithm>
#include <bit>
#include <cassert>
#include <chrono>
#include <cstddef>
#include <exception>
#include <functional>
#include <future>
#include <initializer_list>
#include <iterator>
#include <memory>
#include <ostream>
#include <ranges>
#include <span>
#include <tuple>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Runtime/AbstractBufferProvider.hpp>
#include <Runtime/BufferManager.hpp>
#include <Runtime/Execution/QueryStatus.hpp>
#include <Runtime/QueryTerminationType.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <Sequencing/SequenceData.hpp>
#include <Sources/SourceHandle.hpp>
#include <Util/Overloaded.hpp>
#include <Util/UUID.hpp>
#include <BackpressureChannel.hpp>
#include <fmt/format.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <ErrorHandling.hpp>
#include <ExecutableQueryPlan.hpp>
#include <Listeners/QueryLog.hpp>
#include <QueryEngine.hpp>
#include <QueryEngineConfiguration.hpp>
#include <TestSource.hpp>
#include <adaptive_engine/Buffer.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include <adaptive_engine/PipelineStage.hpp>
#include <adaptive_engine/SourceHandle.hpp>
#include <MemoryTestUtils.hpp>

namespace NES::Testing
{

std::vector<std::byte> identifiableData(size_t identifier)
{
    std::vector data(DEFAULT_BUFFER_SIZE / sizeof(size_t), identifier);
    auto bytes = std::as_bytes(std::span{data.begin(), data.end()});
    return {bytes.begin(), bytes.end()};
}

bool verifyIdentifier(const TupleBuffer& buffer, size_t identifier)
{
    if (buffer.getBufferSize() == 0)
    {
        return false;
    }

    return std::ranges::all_of(buffer.getAvailableMemoryArea<size_t>(), [&](const auto& element) { return element == identifier; });
}

std::ostream& TestPipeline::toString(std::ostream& os) const
{
    return os << "TestPipeline";
}

testing::AssertionResult TestSinkController::waitForNumberOfReceivedBuffersOrMore(size_t numberOfExpectedBuffers)
{
    auto buffers = receivedBuffers.lock();
    if (buffers->size() >= numberOfExpectedBuffers)
    {
        return testing::AssertionSuccess();
    }

    auto check = receivedBufferTrigger.wait_for(
        buffers.as_lock(), DEFAULT_LONG_AWAIT_TIMEOUT, [&]() { return buffers->size() >= numberOfExpectedBuffers; });

    if (check)
    {
        return testing::AssertionSuccess();
    }

    return testing::AssertionFailure() << fmt::format(
               "The expected number of TupleBuffers were not received after {}. Expected: {}, but Received {}",
               std::chrono::duration_cast<std::chrono::milliseconds>(DEFAULT_LONG_AWAIT_TIMEOUT),
               numberOfExpectedBuffers,
               buffers->size());
}

void TestSinkController::insertBuffer(TupleBuffer&& buffer)
{
    ++invocations;
    receivedBuffers.lock()->push_back(std::move(buffer));
    receivedBufferTrigger.notify_one();
}

std::vector<TupleBuffer> TestSinkController::takeBuffers()
{
    auto buffers = receivedBuffers.exchange({});
    std::ranges::sort(
        buffers,
        std::less{},
        [](const auto& buffer) { return SequenceData{buffer.getSequenceNumber(), buffer.getChunkNumber(), buffer.isLastChunk()}; });
    return buffers;
}

std::ostream& TestSink::toString(std::ostream& os) const
{
    return os << "TestSink";
}

// ============================================================================
// TestBufferWrapper implementation
// ============================================================================

TestBufferWrapper::TestBufferWrapper(TupleBuffer buf) : buffer(std::move(buf)), metadata{}
{
    metadata.sequence_number = buffer.getSequenceNumber().getRawValue();
    metadata.origin_id = buffer.getOriginId().getRawValue();
    metadata.watermark = buffer.getWatermark().getRawValue();
    metadata.num_tuples = buffer.getNumberOfTuples();
    metadata.chunk_number = static_cast<uint32_t>(buffer.getChunkNumber().getRawValue());
    metadata.last_chunk = buffer.isLastChunk();
}

// ============================================================================
// TestBufferProvider implementation
// ============================================================================

TestBufferProvider::TestBufferProvider(std::shared_ptr<AbstractBufferProvider> nesProvider)
    : nesProvider_(std::move(nesProvider))
{
    INVARIANT(nesProvider_ != nullptr, "TestBufferProvider requires a valid AbstractBufferProvider");
}

adaptive_engine::BufferHandle TestBufferProvider::wrap(void* data, size_t /*size*/, const adaptive_engine::BufferMetadata& /*metadata*/)
{
    INVARIANT(data != nullptr, "Cannot wrap null TupleBuffer pointer");

    auto* tupleBufferPtr = static_cast<TupleBuffer*>(data);
    tupleBufferPtr->retain();
    auto* wrapper = new TestBufferWrapper(*tupleBufferPtr);
    tupleBufferPtr->release();

    return adaptive_engine::BufferHandle{wrapper};
}

void TestBufferProvider::release(adaptive_engine::BufferHandle handle)
{
    if (handle.opaque == nullptr)
    {
        return;
    }
    delete static_cast<TestBufferWrapper*>(handle.opaque);
}

void* TestBufferProvider::get_data(adaptive_engine::BufferHandle handle)
{
    INVARIANT(handle.opaque != nullptr, "Cannot get data from null handle");
    auto* wrapper = static_cast<TestBufferWrapper*>(handle.opaque);
    return wrapper->buffer.getAvailableMemoryArea<uint8_t>().data();
}

size_t TestBufferProvider::get_size(adaptive_engine::BufferHandle handle)
{
    INVARIANT(handle.opaque != nullptr, "Cannot get size from null handle");
    auto* wrapper = static_cast<TestBufferWrapper*>(handle.opaque);
    return wrapper->buffer.getBufferSize();
}

const adaptive_engine::BufferMetadata& TestBufferProvider::get_metadata(adaptive_engine::BufferHandle handle)
{
    INVARIANT(handle.opaque != nullptr, "Cannot get metadata from null handle");
    auto* wrapper = static_cast<TestBufferWrapper*>(handle.opaque);
    return wrapper->metadata;
}

adaptive_engine::BufferHandle TestBufferProvider::allocate(size_t size)
{
    INVARIANT(nesProvider_ != nullptr, "Buffer provider not initialized");

    std::optional<TupleBuffer> buffer;
    if (size <= nesProvider_->getBufferSize())
    {
        buffer = nesProvider_->getBufferNoBlocking();
    }
    else
    {
        buffer = nesProvider_->getUnpooledBuffer(size);
    }

    if (!buffer.has_value())
    {
        return adaptive_engine::BufferHandle{nullptr};
    }

    auto* wrapper = new TestBufferWrapper(std::move(buffer.value()));
    return adaptive_engine::BufferHandle{wrapper};
}

// ============================================================================
// AdaptiveTestPipeline implementation
// ============================================================================

AdaptiveTestPipeline::AdaptiveTestPipeline(std::shared_ptr<TestPipelineController> controller, std::string stageId)
    : controller_(std::move(controller)), stageId_(std::move(stageId))
{
}

AdaptiveTestPipeline::~AdaptiveTestPipeline()
{
    controller_->destruction.set_value();
}

void AdaptiveTestPipeline::start(adaptive_engine::ExecutionContext& /*ctx*/)
{
    std::this_thread::sleep_for(controller_->startDuration.load());
    controller_->start.set_value();
    if (controller_->failOnStart)
    {
        throw Exception("I should throw here.", 9999);
    }
}

void AdaptiveTestPipeline::execute(adaptive_engine::ExecutionContext& ctx, adaptive_engine::BufferHandle input)
{
    if (controller_->invocations.fetch_add(1) + 1 == controller_->throwOnNthInvocation)
    {
        throw Exception("I should throw here.", 9999);
    }

    // Handle repeat functionality using watermark as repeat counter
    const size_t maxRepeats = controller_->repeatCount.load();
    if (maxRepeats > 0)
    {
        auto* bufferProvider = ctx.get_buffer_provider();
        const auto& metadata = bufferProvider->get_metadata(input);
        const uint64_t currentRepeatCount = metadata.watermark;
        if (currentRepeatCount < maxRepeats)
        {
            // For repeats, we need to request re-execution
            ctx.repeat_task();
            return;
        }
    }

    // Emit buffer to downstream
    ctx.emit_buffer(input);
}

void AdaptiveTestPipeline::stop(adaptive_engine::ExecutionContext& ctx)
{
    std::this_thread::sleep_for(controller_->stopDuration.load());
    if (controller_->failOnStop)
    {
        throw Exception("I should throw here.", 9999);
    }

    auto stopCalls = stopCalled_.fetch_add(1);
    auto repeatsDuringStop = controller_->repeatCountDuringStop.load();
    if (stopCalls == repeatsDuringStop)
    {
        controller_->stop.set_value();
    }
    else if (stopCalls > repeatsDuringStop)
    {
        controller_->stop.set_exception(std::make_exception_ptr(TestException("Pipeline was terminated too often")));
    }
    else
    {
        ctx.repeat_task();
    }
}

// ============================================================================
// AdaptiveTestSink implementation
// ============================================================================

AdaptiveTestSink::AdaptiveTestSink(std::shared_ptr<TestBufferProvider> bufferProvider, std::shared_ptr<TestSinkController> controller, std::string stageId)
    : bufferProvider_(std::move(bufferProvider)), controller_(std::move(controller)), stageId_(std::move(stageId))
{
}

AdaptiveTestSink::~AdaptiveTestSink()
{
    controller_->destruction.set_value();
}

void AdaptiveTestSink::start(adaptive_engine::ExecutionContext& /*ctx*/)
{
    controller_->start.set_value();
}

void AdaptiveTestSink::execute(adaptive_engine::ExecutionContext& ctx, adaptive_engine::BufferHandle input)
{
    // Extract TupleBuffer from the handle and store it
    auto* wrapper = static_cast<TestBufferWrapper*>(input.opaque);
    if (wrapper != nullptr)
    {
        controller_->insertBuffer(Testing::copyBuffer(wrapper->buffer, *bufferProvider_->nesProvider_));
    }

    // Handle repeat functionality
    const size_t maxRepeats = controller_->repeatCount.load();
    if (maxRepeats > 0 && wrapper != nullptr)
    {
        const uint64_t currentRepeatCount = wrapper->metadata.watermark;
        if (currentRepeatCount < maxRepeats)
        {
            ctx.repeat_task();
        }
    }
}

void AdaptiveTestSink::stop(adaptive_engine::ExecutionContext& ctx)
{
    auto stopCalls = stopCalled_.fetch_add(1);
    auto repeatsDuringStop = controller_->repeatCountDuringStop.load();
    if (stopCalls == repeatsDuringStop)
    {
        controller_->stop.set_value();
    }
    else if (stopCalls > repeatsDuringStop)
    {
        controller_->stop.set_exception(std::make_exception_ptr(TestException("Sink was terminated too often")));
    }
    else
    {
        ctx.repeat_task();
    }
}

// ============================================================================
// AdaptiveTestSourceHandle implementation
// ============================================================================

AdaptiveTestSourceHandle::AdaptiveTestSourceHandle(
    std::unique_ptr<TestSource> source,
    OriginId sourceId,
    std::shared_ptr<TestBufferProvider> bufferProvider)
    : source_(std::move(source)), sourceId_(sourceId), bufferProvider_(std::move(bufferProvider))
{
}

std::optional<adaptive_engine::BufferHandle> AdaptiveTestSourceHandle::next_buffer(adaptive_engine::ExecutionContext& /*ctx*/)
{
    if (!opened_)
    {
        return std::nullopt;
    }

    // Allocate a buffer for the source to fill
    auto handle = bufferProvider_->allocate(bufferProvider_->nesProvider_->getBufferSize());
    if (handle.opaque == nullptr)
    {
        return std::nullopt;
    }

    auto* wrapper = static_cast<TestBufferWrapper*>(handle.opaque);
    auto result = source_->fillTupleBuffer(wrapper->buffer, stopSource_.get_token());

    if (result.isEoS())
    {
        // End of stream or shutdown
        bufferProvider_->release(handle);
        return std::nullopt;
    }

    // Success - update metadata from filled buffer
    wrapper->metadata.sequence_number = sequenceNumber_.fetch_add(1);
    wrapper->metadata.origin_id = sourceId_.getRawValue();
    wrapper->metadata.watermark = wrapper->buffer.getWatermark().getRawValue();
    wrapper->metadata.num_tuples = wrapper->buffer.getNumberOfTuples();
    wrapper->metadata.chunk_number = static_cast<uint32_t>(wrapper->buffer.getChunkNumber().getRawValue());
    wrapper->metadata.last_chunk = wrapper->buffer.isLastChunk();
    return handle;
}

void AdaptiveTestSourceHandle::open(adaptive_engine::ExecutionContext& /*ctx*/)
{
    source_->open(bufferProvider_->nesProvider_);
    opened_ = true;
}

void AdaptiveTestSourceHandle::close(adaptive_engine::ExecutionContext& /*ctx*/)
{
    stopSource_.request_stop();
    source_->close();
    opened_ = false;
}

std::string AdaptiveTestSourceHandle::get_id() const
{
    return fmt::format("TestSource-{}", sourceId_.getRawValue());
}

void AdaptiveTestSourceHandle::request_stop()
{
    stopSource_.request_stop();
}

std::tuple<std::shared_ptr<ExecutablePipeline>, std::shared_ptr<TestSinkController>>
createSinkPipeline(PipelineId id, BackpressureController backpressureController, std::shared_ptr<AbstractBufferProvider> bm)
{
    auto sinkController = std::make_shared<TestSinkController>(std::move(backpressureController));
    auto stage = std::make_unique<TestSink>(std::move(bm), sinkController);
    auto pipeline = ExecutablePipeline::create(id, std::move(stage), {});
    return {pipeline, sinkController};
}

std::tuple<std::shared_ptr<ExecutablePipeline>, std::shared_ptr<TestPipelineController>>
createPipeline(PipelineId id, const std::vector<std::shared_ptr<ExecutablePipeline>>& successors)
{
    auto pipelineCtrl = std::make_shared<TestPipelineController>();
    auto stage = std::make_unique<TestPipeline>(pipelineCtrl);
    std::vector<std::weak_ptr<ExecutablePipeline>> weakSuccessors;
    for (const auto& succ : successors)
    {
        weakSuccessors.push_back(succ);
    }
    auto pipeline = ExecutablePipeline::create(id, std::move(stage), weakSuccessors);
    return {pipeline, pipelineCtrl};
}

QueryPlanBuilder::identifier_t QueryPlanBuilder::addPipeline(const std::vector<identifier_t>& predecssors)
{
    auto identifier = nextIdentifier++;
    for (auto pred : predecssors)
    {
        INVARIANT(!std::holds_alternative<SinkDescriptor>(objects[pred]), "Sink Descriptor cannot be a predecessor");
        forwardRelations[pred].push_back(identifier);
        backwardRelations[identifier].push_back(pred);
    }

    objects[identifier] = PipelineDescriptor{PipelineId(pipelineIdCounter++)};
    return identifier;
}

QueryPlanBuilder::identifier_t QueryPlanBuilder::addSource()
{
    auto identifier = nextIdentifier++;
    objects[identifier] = SourceDescriptor{OriginId(originIdCounter++)};
    forwardRelations[identifier] = {};
    return identifier;
}

QueryPlanBuilder::identifier_t QueryPlanBuilder::addSink(const std::vector<identifier_t>& predecessors)
{
    auto identifier = nextIdentifier++;
    for (auto pred : predecessors)
    {
        assert(!std::holds_alternative<SinkDescriptor>(objects[pred]) && "Sink Descriptor cannot be a predecessor");
        forwardRelations[pred].push_back(identifier);
        backwardRelations[identifier].push_back(pred);
    }

    objects[identifier] = SinkDescriptor{PipelineId(pipelineIdCounter++)};
    return identifier;
}

QueryPlanBuilder::TestPlanCtrl QueryPlanBuilder::build(LocalQueryId queryId, std::shared_ptr<BufferManager> bm) &&
{
    auto isSource = std::ranges::views::filter([](const std::pair<identifier_t, QueryComponentDescriptor>& kv)
                                               { return std::holds_alternative<SourceDescriptor>(kv.second); });
    std::vector<std::unique_ptr<SourceHandle>> sources;

    std::vector<std::shared_ptr<ExecutablePipeline>> pipelines;
    std::unordered_map<identifier_t, OriginId> sourceIds;
    std::unordered_map<identifier_t, PipelineId> pipelineIds;

    std::unordered_map<identifier_t, ExecutablePipelineStage*> stages;
    std::unordered_map<identifier_t, std::shared_ptr<TestSourceControl>> sourceCtrls;
    std::unordered_map<identifier_t, std::shared_ptr<TestSinkController>> sinkCtrls;
    std::unordered_map<identifier_t, std::shared_ptr<TestPipelineController>> pipelineCtrls;
    std::unordered_map<identifier_t, std::shared_ptr<ExecutablePipeline>> cache{};

    auto [backpressureController, backpressureListener] = createBackpressureChannel();
    std::function<std::shared_ptr<ExecutablePipeline>(identifier_t)> getOrCreatePipeline = [&](identifier_t identifier)
    {
        if (auto it = cache.find(identifier); it != cache.end())
        {
            return it->second;
        }

        auto result = std::visit(
            Overloaded{
                [](SourceDescriptor) -> std::shared_ptr<ExecutablePipeline>
                {
                    INVARIANT(false, "Source cannot be a successor");
                    std::terminate(); /// Ensures termination if INVARIANT is a no-op in release mode.
                },
                [&](SinkDescriptor descriptor) -> std::shared_ptr<ExecutablePipeline>
                {
                    auto [sink, ctrl] = createSinkPipeline(descriptor.pipelineId, std::move(backpressureController), bm);
                    pipelines.push_back(sink);
                    stages[identifier] = sink->stage.get();
                    sinkCtrls[identifier] = ctrl;
                    pipelineIds.emplace(identifier, descriptor.pipelineId);
                    return pipelines.back();
                },
                [&](PipelineDescriptor descriptor) -> std::shared_ptr<ExecutablePipeline>
                {
                    std::vector<std::shared_ptr<ExecutablePipeline>> successors;
                    std::ranges::transform(forwardRelations.at(identifier), std::back_inserter(successors), getOrCreatePipeline);
                    auto [pipeline, pipelineCtrl] = createPipeline(descriptor.pipelineId, successors);
                    stages[identifier] = pipeline->stage.get();
                    pipelines.push_back(std::move(pipeline));
                    pipelineIds.emplace(identifier, descriptor.pipelineId);
                    pipelineCtrls[identifier] = pipelineCtrl;
                    return pipelines.back();
                }},
            objects[identifier]);

        cache[identifier] = result;
        return result;
    };

    for (auto source : objects | isSource)
    {
        // Build successor pipelines (needed for legacy pipeline tracking)
        std::vector<std::weak_ptr<ExecutablePipeline>> successors;
        std::ranges::transform(forwardRelations.at(source.first), std::back_inserter(successors), getOrCreatePipeline);
        auto [s, ctrl] = getTestSource(backpressureListener, std::get<SourceDescriptor>(source.second).sourceId, bm);
        sourceIds.emplace(source.first, s->getSourceId());
        sources.emplace_back(std::move(s));
        sourceCtrls[source.first] = ctrl;
    }

    // Create empty adaptive stages and edges - the NES test infra uses QueryEngine which wraps the adaptive engine
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> adaptiveStages;
    std::vector<adaptive_engine::Edge> edges;

    return {
        .query = std::make_unique<ExecutableQueryPlan>(queryId, std::move(sources), std::move(adaptiveStages), std::move(edges)),
        .sourceIds = sourceIds,
        .pipelineIds = pipelineIds,
        .sourceCtrls = sourceCtrls,
        .sinkCtrls = sinkCtrls,
        .pipelineCtrls = pipelineCtrls,
        .stages = stages};
}

QueryPlanBuilder::QueryPlanBuilder(
    identifier_t nextIdentifier, PipelineId::Underlying pipelineIdCounter, OriginId::Underlying originIdCounter)
    : nextIdentifier(nextIdentifier), pipelineIdCounter(pipelineIdCounter), originIdCounter(originIdCounter)
{
}

TestingHarness::TestingHarness(size_t numberOfThreads, size_t numberOfBuffers)
    : bm(BufferManager::create(DEFAULT_BUFFER_SIZE, numberOfBuffers)), numberOfThreads(numberOfThreads)
{
}

TestingHarness::TestingHarness() : TestingHarness(NUMBER_OF_THREADS, NUMBER_OF_BUFFERS_PER_SOURCE)
{
}

QueryPlanBuilder TestingHarness::buildNewQuery() const
{
    return QueryPlanBuilder{lastIdentifier, lastPipelineIdCounter, lastOriginIdCounter};
}

std::pair<LocalQueryId, std::unique_ptr<ExecutableQueryPlan>> TestingHarness::addNewQuery(QueryPlanBuilder&& builder)
{
    const auto queryId = LocalQueryId(UUIDToString(generateUUID()));
    lastIdentifier = builder.nextIdentifier;
    lastOriginIdCounter = builder.originIdCounter;
    lastPipelineIdCounter = builder.pipelineIdCounter;
    /// NOLINTNEXTLINE
    auto [plan, pSourceIds, pPipelineIds, pSourceCtrls, pSinkCtrls, pPipelineCtrls, pStages] = std::move(builder).build(queryId, bm);
    sourceIds.insert(pSourceIds.begin(), pSourceIds.end());
    pipelineIds.insert(pPipelineIds.begin(), pPipelineIds.end());
    sourceControls.insert(pSourceCtrls.begin(), pSourceCtrls.end());
    sinkControls.insert(pSinkCtrls.begin(), pSinkCtrls.end());
    pipelineControls.insert(pPipelineCtrls.begin(), pPipelineCtrls.end());
    stages.insert(pStages.begin(), pStages.end());
    return {queryId, std::move(plan)};
}

void TestingHarness::expectQueryStatusEvents(LocalQueryId id, std::initializer_list<QueryState> states)
{
    for (auto state : states)
    {
        switch (state)
        {
            case QueryState::Registered:
                EXPECT_CALL(*status, mockLogQueryStatusChange(id, QueryState::Registered, ::testing::_)).Times(1);
                break;
            case QueryState::Started:
                EXPECT_CALL(*status, mockLogQueryStatusChange(id, QueryState::Started, ::testing::_))
                    .Times(1)
                    .WillOnce(::testing::Invoke([](auto, auto, auto) { }));
                break;
            case QueryState::Running:
                queryRunning.emplace(id, std::make_unique<std::promise<void>>());
                EXPECT_CALL(*status, mockLogQueryStatusChange(id, QueryState::Running, ::testing::_))
                    .Times(1)
                    .WillOnce(::testing::Invoke(
                        [this](auto id, auto, auto)
                        {
                            queryRunning.at(id)->set_value();
                        }));
                break;
            case QueryState::Stopped:
                ASSERT_TRUE(queryTermination.try_emplace(id, std::make_unique<std::promise<void>>()).second)
                    << "Registered multiple query terminations";
                EXPECT_CALL(*status, mockLogQueryStatusChange(id, QueryState::Stopped, ::testing::_))
                    .Times(1)
                    .WillOnce(::testing::Invoke(
                        [this](auto id, auto, auto)
                        {
                            queryTermination.at(id)->set_value();
                        }));
                break;
            case QueryState::Failed:
                ASSERT_TRUE(queryTermination.try_emplace(id, std::make_unique<std::promise<void>>()).second)
                    << "Registered multiple query terminations";
                EXPECT_CALL(*status, mockLogQueryFailure(id, ::testing::_, ::testing::_))
                    .Times(1)
                    .WillOnce(::testing::Invoke(
                        [this](const auto& id, const auto&, auto)
                        {
                            queryTermination.at(id)->set_value();
                        }));
                break;
        }
    }
}

void TestingHarness::expectSourceTermination(LocalQueryId queryId, QueryPlanBuilder::identifier_t source, QueryTerminationType type)
{
    EXPECT_CALL(*status, mockLogSourceTermination(queryId, sourceIds.at(source), type, ::testing::_)).Times(1);
}

void TestingHarness::start()
{
    for (const auto& queryTermination : queryTermination)
    {
        queryTerminationFutures[queryTermination.first] = queryTermination.second->get_future().share();
    }
    for (const auto& queryRunning : queryRunning)
    {
        queryRunningFutures[queryRunning.first] = queryRunning.second->get_future().share();
    }
    QueryEngineConfiguration configuration{};
    configuration.numWorkerThreads.setValue(numberOfThreads);
    // Use default WorkerThreadId(0) for test infrastructure
    qm = std::make_unique<QueryEngine>(configuration, this->statListener, this->status, this->bm);
}

void TestingHarness::startQuery(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> query) const
{
    qm->start(queryId, std::move(query));
}

void TestingHarness::stopQuery(LocalQueryId id) const
{
    qm->stop(id);
}

void TestingHarness::stop()
{
    qm.reset();
}

testing::AssertionResult TestingHarness::waitForQepTermination(LocalQueryId id, std::chrono::milliseconds timeout) const
{
    return waitForFuture(queryTerminationFutures.at(id), timeout);
}

testing::AssertionResult TestingHarness::waitForQepRunning(LocalQueryId id, std::chrono::milliseconds timeout)
{
    return waitForFuture(queryRunningFutures.at(id), timeout);
}
}
