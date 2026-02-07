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
#include <Sequencing/RangeCompletionTracker.hpp>

#include <algorithm>
#include <atomic>
#include <cstddef>
#include <iostream>
#include <limits>
#include <optional>
#include <random>
#include <thread>
#include <tuple>
#include <unordered_map>
#include <utility>
#include <vector>
#include <Sequencing/SequenceNumber.hpp>
#include <Time/Timestamp.hpp>
#include <gmock/gmock-matchers.h>
#include <gtest/gtest.h>
#include <folly/Synchronized.h>
#include <BaseUnitTest.hpp>

using namespace ::testing;

namespace NES
{

static auto SeqWithWatermark(size_t rootSeq, Timestamp watermark)
{
    return Optional(Pair(rootSeq, watermark));
}

TEST(RangeCompletionTest, SingleInsert)
{
    folly::Synchronized<RangeCompletionTracker> tracker;
    ASSERT_THAT(
        tracker.wlock()->insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)), Timestamp(32)),
        SeqWithWatermark(1, Timestamp(32)));
}

TEST(RangeCompletionTest, MultipleChunks)
{
    folly::Synchronized<RangeCompletionTracker> tracker;
    /// First sub-range [1, 1.1) -- not completing the sequence
    EXPECT_EQ(tracker.wlock()->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), Timestamp(2)), std::nullopt);
    /// Second sub-range [1.1, 2) -- completes the sequence
    ASSERT_THAT(
        tracker.wlock()->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), Timestamp(12)),
        SeqWithWatermark(1, Timestamp(12)));
}

TEST(RangeCompletionTest, MultipleChunksOutOfOrder)
{
    folly::Synchronized<RangeCompletionTracker> tracker;
    /// Insert the closing sub-range [1.1, 2) first
    EXPECT_EQ(tracker.wlock()->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), Timestamp(42)), std::nullopt);
    /// Then insert the opening sub-range [1, 1.1) -- this completes the sequence
    ASSERT_THAT(
        tracker.wlock()->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), Timestamp(2)),
        SeqWithWatermark(1, Timestamp(42)));
}

TEST(RangeCompletionTest, InsertAfterCompletionStartsNewTracking)
{
    folly::Synchronized<RangeCompletionTracker> tracker;
    /// First sub-range [1, 1.1)
    EXPECT_EQ(tracker.wlock()->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), Timestamp(2)), std::nullopt);
    /// Second sub-range [1.1, 2) -- completes sequence
    ASSERT_THAT(
        tracker.wlock()->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), Timestamp(12)),
        SeqWithWatermark(1, Timestamp(12)));
    /// Inserting another sub-range [1.2, 2) after completion does not crash --
    /// the sequence was already erased from tracking, so this starts a new incomplete entry.
    EXPECT_EQ(tracker.wlock()->insert(SequenceRange(SequenceNumber({1, 2}), SequenceNumber(2)), Timestamp(12)), std::nullopt);
}

TEST(RangeCompletionTest, DifferentSequenceNumbers)
{
    folly::Synchronized<RangeCompletionTracker> tracker;
    /// Sequence 1: sub-range [1.1, 2) -- closing part, not yet complete
    EXPECT_EQ(tracker.wlock()->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), Timestamp(32)), std::nullopt);
    /// Sequence 101: sub-range [101, 101.1) -- opening part, not yet complete
    EXPECT_EQ(tracker.wlock()->insert(SequenceRange(SequenceNumber(101), SequenceNumber({101, 1})), Timestamp(32)), std::nullopt);
    /// Sequence 1: sub-range [1, 1.1) -- completes sequence 1
    ASSERT_THAT(
        tracker.wlock()->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), Timestamp(32)),
        SeqWithWatermark(1, Timestamp(32)));
    /// Sequence 101: sub-range [101.1, 102) -- completes sequence 101
    ASSERT_THAT(
        tracker.wlock()->insert(SequenceRange(SequenceNumber({101, 1}), SequenceNumber(102)), Timestamp(32)),
        SeqWithWatermark(101, Timestamp(32)));
}

class ConcurrentRangeCompletionTest
    : public ::testing::TestWithParam<std::tuple<size_t, size_t, size_t>>
{
};

TEST_P(ConcurrentRangeCompletionTest, RandomInserts)
{
    auto [maxSequenceNumber, maxChunkNumber, numberOfThreads] = GetParam();
    std::random_device rd;
    std::uniform_int_distribution<size_t> chunkNumbers(2, maxChunkNumber + 1);
    std::uniform_int_distribution<Timestamp::Underlying> watermarks(1, 10000000);
    std::unordered_map<size_t, Timestamp::Underlying> maxWaterMark;

    /// Prepare Inserts: generate sub-ranges for each sequence number
    std::vector<std::pair<SequenceRange, Timestamp::Underlying>> inserts;
    for (size_t i = size_t(1); i < maxSequenceNumber + size_t(1); ++i)
    {
        auto maxWatermarkForCurrentSequence = std::numeric_limits<Timestamp::Underlying>::min();
        auto chunks = chunkNumbers(rd);
        auto watermark = watermarks(rd);
        /// Generate sub-ranges partitioning [i, i+1)
        for (size_t j = 0; j < chunks; ++j)
        {
            auto subStart = (j == 0) ? SequenceNumber(i) : SequenceNumber(i).child(j);
            auto subEnd = (j == chunks - 1) ? SequenceNumber(i + 1) : SequenceNumber(i).child(j + 1);
            inserts.push_back({SequenceRange(subStart, subEnd), watermark});
            maxWatermarkForCurrentSequence = std::max(watermark, maxWatermarkForCurrentSequence);
        }
        maxWaterMark[i] = maxWatermarkForCurrentSequence;
    }

    /// Add inserts so they can be evenly divided on all threads
    auto moreInserts = numberOfThreads - (inserts.size() % numberOfThreads);
    auto lastSeq = maxSequenceNumber + size_t(1);
    auto maxWatermarkForLastSequence = std::numeric_limits<Timestamp::Underlying>::min();
    for (size_t i = 0; i < moreInserts; ++i)
    {
        auto watermark = watermarks(rd);
        auto subStart = (i == 0) ? SequenceNumber(lastSeq) : SequenceNumber(lastSeq).child(i);
        auto subEnd = (i == moreInserts - 1) ? SequenceNumber(lastSeq + 1) : SequenceNumber(lastSeq).child(i + 1);
        inserts.push_back({SequenceRange(subStart, subEnd), watermark});
        maxWatermarkForLastSequence = std::max(maxWatermarkForLastSequence, watermark);
        std::cout << watermark << '\n';
    }
    maxWaterMark[lastSeq] = maxWatermarkForLastSequence;

    /// Shuffle Inserts to create out of order
    std::mt19937 g(rd());
    std::ranges::shuffle(inserts, g);

    folly::Synchronized<RangeCompletionTracker> tracker;
    std::atomic_size_t completed;

    std::vector<std::jthread> threads;
    threads.reserve(numberOfThreads);
    size_t perThread = inserts.size() / numberOfThreads;
    for (size_t i = 0; i < numberOfThreads; ++i)
    {
        threads.emplace_back(
            [&, i]()
            {
                for (size_t j = 0; j < perThread; ++j)
                {
                    auto [range, watermark] = inserts[(perThread * i) + j];
                    if (auto opt = tracker.wlock()->insert(range, Timestamp(watermark)))
                    {
                        ++completed;
                        EXPECT_EQ(opt->first, range.rootSequence());
                        EXPECT_EQ(opt->second.getRawValue(), maxWaterMark[range.rootSequence()]) << range;
                    }
                }
            });
    }

    threads.clear();

    EXPECT_EQ(completed.load(), maxSequenceNumber + size_t(1));
}

INSTANTIATE_TEST_CASE_P(
    RangeCompletionTest,
    ConcurrentRangeCompletionTest,
    ::testing::Combine(::testing::Values(10, 1000, 50000), ::testing::Values(1, 5, 50), ::testing::Values(1, 4, 16)));

}
