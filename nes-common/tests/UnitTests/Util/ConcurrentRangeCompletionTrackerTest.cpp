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
#include <cstdint>
#include <limits>
#include <random>
#include <thread>
#include <tuple>
#include <unordered_map>
#include <vector>
#include <Sequencing/SequenceNumber.hpp>
#include <folly/Synchronized.h>
#include <gtest/gtest.h>
#include <BaseUnitTest.hpp>

namespace NES
{

TEST(ConcurrentRangeCompletionTrackerTest, SingleInsert)
{
    folly::Synchronized<RangeCompletionTracker<uint64_t>> tracker;
    {
        auto locked = tracker.wlock();
        locked->insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)), uint64_t(32));
        EXPECT_EQ(locked->getCompletedUpTo(), 1);
        EXPECT_EQ(locked->getCompletedValue(), uint64_t(32));
    }
}

TEST(ConcurrentRangeCompletionTrackerTest, MultipleChunks)
{
    folly::Synchronized<RangeCompletionTracker<uint64_t>> tracker;
    {
        auto locked = tracker.wlock();
        locked->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), uint64_t(2));
        EXPECT_EQ(locked->getCompletedUpTo(), 0);
        locked->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), uint64_t(12));
        EXPECT_EQ(locked->getCompletedUpTo(), 1);
        EXPECT_EQ(locked->getCompletedValue(), uint64_t(12));
    }
}

TEST(ConcurrentRangeCompletionTrackerTest, MultipleChunksOutOfOrder)
{
    folly::Synchronized<RangeCompletionTracker<uint64_t>> tracker;
    {
        auto locked = tracker.wlock();
        locked->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), uint64_t(42));
        EXPECT_EQ(locked->getCompletedUpTo(), 0);
        locked->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), uint64_t(2));
        EXPECT_EQ(locked->getCompletedUpTo(), 1);
        EXPECT_EQ(locked->getCompletedValue(), uint64_t(42));
    }
}

TEST(ConcurrentRangeCompletionTrackerTest, DifferentSequenceNumbers)
{
    folly::Synchronized<RangeCompletionTracker<uint64_t>> tracker;
    {
        auto locked = tracker.wlock();
        locked->insert(SequenceRange(SequenceNumber({1, 1}), SequenceNumber(2)), uint64_t(32));
        locked->insert(SequenceRange(SequenceNumber(101), SequenceNumber({101, 1})), uint64_t(32));
        locked->insert(SequenceRange(SequenceNumber(1), SequenceNumber({1, 1})), uint64_t(32));
        /// Sequence 1 is complete, but frontier is only at 1 (missing 2..100)
        EXPECT_EQ(locked->getCompletedUpTo(), 1);
        EXPECT_EQ(locked->getCompletedValue(), uint64_t(32));
    }
}

/// Sliding window shuffle: swap each element with a random element within
/// a window of the given size. This simulates realistic out-of-order delivery
/// where sequences arrive mostly in order but with local reordering.
template <typename T>
static void slidingWindowShuffle(std::vector<T>& vec, size_t windowSize, std::mt19937& rng)
{
    if (vec.size() <= 1)
    {
        return;
    }
    for (size_t i = 0; i < vec.size(); ++i)
    {
        auto lo = (i > windowSize) ? (i - windowSize) : size_t(0);
        auto hi = std::min(i + windowSize, vec.size() - 1);
        std::uniform_int_distribution<size_t> dist(lo, hi);
        std::swap(vec[i], vec[dist(rng)]);
    }
}

class ConcurrentRangeCompletionTrackerParamTest : public ::testing::TestWithParam<std::tuple<size_t, size_t, size_t>>
{
};

TEST_P(ConcurrentRangeCompletionTrackerParamTest, RandomInserts)
{
    auto [maxSequenceNumber, maxChunkNumber, numberOfThreads] = GetParam();
    std::random_device rd;
    std::mt19937 g(rd());
    std::uniform_int_distribution<size_t> chunkNumbers(2, maxChunkNumber + 1);
    std::uniform_int_distribution<uint64_t> watermarks(1, 10000000);
    std::unordered_map<size_t, uint64_t> maxWaterMark;

    /// Prepare Inserts: generate sub-ranges for each sequence number
    std::vector<std::tuple<SequenceRange, uint64_t>> inserts;
    for (size_t i = size_t(1); i < maxSequenceNumber + size_t(1); ++i)
    {
        auto maxWatermarkForCurrentSequence = std::numeric_limits<uint64_t>::min();
        auto chunks = chunkNumbers(g);
        auto watermark = watermarks(g);
        for (size_t j = 0; j < chunks; ++j)
        {
            auto subStart = (j == 0) ? SequenceNumber(i) : SequenceNumber(i).child(j);
            auto subEnd = (j == chunks - 1) ? SequenceNumber(i + 1) : SequenceNumber(i).child(j + 1);
            inserts.emplace_back(SequenceRange(subStart, subEnd), watermark);
            maxWatermarkForCurrentSequence = std::max(watermark, maxWatermarkForCurrentSequence);
        }
        maxWaterMark[i] = maxWatermarkForCurrentSequence;
    }

    /// Add inserts so they can be evenly divided on all threads
    auto moreInserts = numberOfThreads - (inserts.size() % numberOfThreads);
    auto lastSeq = maxSequenceNumber + size_t(1);
    auto maxWatermarkForLastSequence = std::numeric_limits<uint64_t>::min();
    for (size_t i = 0; i < moreInserts; ++i)
    {
        auto watermark = watermarks(g);
        auto subStart = (i == 0) ? SequenceNumber(lastSeq) : SequenceNumber(lastSeq).child(i);
        auto subEnd = (i == moreInserts - 1) ? SequenceNumber(lastSeq + 1) : SequenceNumber(lastSeq).child(i + 1);
        inserts.emplace_back(SequenceRange(subStart, subEnd), watermark);
        maxWatermarkForLastSequence = std::max(maxWatermarkForLastSequence, watermark);
    }
    maxWaterMark[lastSeq] = maxWatermarkForLastSequence;

    /// Sliding window shuffle: realistic out-of-order delivery
    /// Window size of 128 means each element can move ~128 positions from its original spot
    constexpr size_t shuffleWindow = 128;
    slidingWindowShuffle(inserts, shuffleWindow, g);

    folly::Synchronized<RangeCompletionTracker<uint64_t>> tracker;

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
                    auto& [range, watermark] = inserts[(perThread * i) + j];
                    tracker.wlock()->insert(range, watermark);
                }
            });
    }

    threads.clear();

    auto locked = tracker.rlock();
    EXPECT_EQ(locked->getCompletedUpTo(), maxSequenceNumber + size_t(1));
    ASSERT_TRUE(locked->getCompletedValue().has_value());
    EXPECT_EQ(locked->getCompletedValue().value(), maxWaterMark[maxSequenceNumber + size_t(1)]);
}

INSTANTIATE_TEST_CASE_P(
    ConcurrentRangeCompletionTrackerTest,
    ConcurrentRangeCompletionTrackerParamTest,
    ::testing::Combine(::testing::Values(10, 1000, 5000), ::testing::Values(1, 5, 50), ::testing::Values(1, 4, 16)));

}
