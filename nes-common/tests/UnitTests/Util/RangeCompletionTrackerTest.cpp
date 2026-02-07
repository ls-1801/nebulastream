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

#include <cstddef>
#include <optional>
#include <utility>
#include <Sequencing/SequenceNumber.hpp>
#include <Time/Timestamp.hpp>
#include <gmock/gmock-matchers.h>
#include <gtest/gtest.h>

using namespace ::testing;

namespace NES
{

class RangeCompletionTrackerTest : public ::testing::Test
{
};

static auto CompletedWithWatermark(size_t rootSeq, Timestamp watermark)
{
    return Optional(Pair(rootSeq, watermark));
}

/// Single buffer with no splitting: insert [1, 2) -> immediately complete
TEST_F(RangeCompletionTrackerTest, SingleBufferNoSplit)
{
    RangeCompletionTracker tracker;
    auto result = tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(2)),
        Timestamp(100));
    ASSERT_THAT(result, CompletedWithWatermark(1, Timestamp(100)));
}

/// Two-way split: insert [1, 1.1) then [1.1, 2) -> complete after second insert
TEST_F(RangeCompletionTrackerTest, TwoWaySplitInOrder)
{
    RangeCompletionTracker tracker;

    auto r1 = tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(50));
    EXPECT_EQ(r1, std::nullopt);

    auto r2 = tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)),
        Timestamp(200));
    ASSERT_THAT(r2, CompletedWithWatermark(1, Timestamp(200)));
}

/// Out-of-order: insert [1.1, 2) first, then [1, 1.1) -> complete after merge
TEST_F(RangeCompletionTrackerTest, TwoWaySplitOutOfOrder)
{
    RangeCompletionTracker tracker;

    auto r1 = tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)),
        Timestamp(300));
    EXPECT_EQ(r1, std::nullopt);

    auto r2 = tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(50));
    ASSERT_THAT(r2, CompletedWithWatermark(1, Timestamp(300)));
}

/// Three-way split: [1, 1.1), [1.1, 1.2), [1.2, 2)
TEST_F(RangeCompletionTrackerTest, ThreeWaySplit)
{
    RangeCompletionTracker tracker;

    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(10)), std::nullopt);

    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(2)),
        Timestamp(30)), std::nullopt);

    auto r = tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(std::vector<size_t>{1, 2})),
        Timestamp(20));
    ASSERT_THAT(r, CompletedWithWatermark(1, Timestamp(30)));
}

/// Multi-level fracturing: [1, 1.1) splits into [1, 1.0.1) and [1.0.1, 1.1)
TEST_F(RangeCompletionTrackerTest, MultiLevelFracturing)
{
    RangeCompletionTracker tracker;

    /// First half of a two-way split, itself split into two sub-ranges
    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 0, 1})),
        Timestamp(10)), std::nullopt);

    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 0, 1}), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(20)), std::nullopt);

    /// Second half of the original two-way split
    auto r = tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)),
        Timestamp(30));
    ASSERT_THAT(r, CompletedWithWatermark(1, Timestamp(30)));
}

/// Hole: insert non-adjacent ranges, verify NOT complete
TEST_F(RangeCompletionTrackerTest, HoleNotComplete)
{
    RangeCompletionTracker tracker;

    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(10)), std::nullopt);

    /// Skip [1.1, 1.2) — there's a hole
    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(2)),
        Timestamp(30)), std::nullopt);

    /// Still not complete — the hole [1.1, 1.2) is missing
}

/// Multiple concurrent sequences tracked independently
TEST_F(RangeCompletionTrackerTest, MultipleConcurrentSequences)
{
    RangeCompletionTracker tracker;

    /// Sequence 1: two-way split
    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(10)), std::nullopt);

    /// Sequence 2: single buffer
    auto r2 = tracker.insert(
        SequenceRange(SequenceNumber(2), SequenceNumber(3)),
        Timestamp(200));
    ASSERT_THAT(r2, CompletedWithWatermark(2, Timestamp(200)));

    /// Sequence 1: complete
    auto r1 = tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)),
        Timestamp(50));
    ASSERT_THAT(r1, CompletedWithWatermark(1, Timestamp(50)));
}

/// Watermark tracking: max watermark across all sub-ranges is returned
TEST_F(RangeCompletionTrackerTest, MaxWatermarkTracking)
{
    RangeCompletionTracker tracker;

    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(999)), std::nullopt);

    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(std::vector<size_t>{1, 2})),
        Timestamp(1)), std::nullopt);

    auto r = tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(2)),
        Timestamp(500));
    ASSERT_THAT(r, CompletedWithWatermark(1, Timestamp(999)));
}

/// Many sequential complete sequences
TEST_F(RangeCompletionTrackerTest, ManySequentialCompleteSequences)
{
    RangeCompletionTracker tracker;

    for (size_t i = 1; i <= 100; ++i)
    {
        auto r = tracker.insert(
            SequenceRange(SequenceNumber(i), SequenceNumber(i + 1)),
            Timestamp(i));
        ASSERT_THAT(r, CompletedWithWatermark(i, Timestamp(i)));
    }
}

/// Four-way split arriving in reverse order
TEST_F(RangeCompletionTrackerTest, FourWaySplitReverseOrder)
{
    RangeCompletionTracker tracker;

    /// [1.3, 2)
    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 3}), SequenceNumber(2)),
        Timestamp(40)), std::nullopt);

    /// [1.2, 1.3)
    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(std::vector<size_t>{1, 3})),
        Timestamp(30)), std::nullopt);

    /// [1.1, 1.2)
    EXPECT_EQ(tracker.insert(
        SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(std::vector<size_t>{1, 2})),
        Timestamp(20)), std::nullopt);

    /// [1, 1.1) — completes the sequence
    auto r = tracker.insert(
        SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})),
        Timestamp(10));
    ASSERT_THAT(r, CompletedWithWatermark(1, Timestamp(40)));
}

}
