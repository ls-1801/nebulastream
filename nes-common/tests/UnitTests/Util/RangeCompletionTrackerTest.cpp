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
#include <cstdint>
#include <Sequencing/SequenceNumber.hpp>
#include <gtest/gtest.h>

namespace NES
{

class RangeCompletionTrackerTest : public ::testing::Test
{
};

/// Single buffer with no splitting: insert [1, 2) -> immediately complete
TEST_F(RangeCompletionTrackerTest, SingleBufferNoSplit)
{
    RangeCompletionTracker<> tracker;
    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
    EXPECT_EQ(tracker.getHighestSeen(), SequenceNumber(2));
}

/// Two-way split: insert [1, 1.1) then [1.1, 2) -> complete after second insert
TEST_F(RangeCompletionTrackerTest, TwoWaySplitInOrder)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
}

/// Out-of-order: insert [1.1, 2) first, then [1, 1.1) -> complete after merge
TEST_F(RangeCompletionTrackerTest, TwoWaySplitOutOfOrder)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
}

/// Three-way split: [1, 1.1), [1.1, 1.2), [1.2, 2)
TEST_F(RangeCompletionTrackerTest, ThreeWaySplit)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})));
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(std::vector<size_t>{1, 2})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
}

/// Multi-level fracturing: [1, 1.1) splits into [1, 1.0.1) and [1.0.1, 1.1)
TEST_F(RangeCompletionTrackerTest, MultiLevelFracturing)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 0, 1})));
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 0, 1}), SequenceNumber(std::vector<size_t>{1, 1})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
}

/// Hole: insert non-adjacent ranges, verify NOT complete
TEST_F(RangeCompletionTrackerTest, HoleNotComplete)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})));
    /// Skip [1.1, 1.2) — there's a hole
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(2)));

    EXPECT_EQ(tracker.getCompletedUpTo(), 0);
}

/// Multiple concurrent sequences tracked independently
TEST_F(RangeCompletionTrackerTest, MultipleConcurrentSequences)
{
    RangeCompletionTracker<> tracker;

    /// Sequence 1: two-way split (first half only)
    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    /// Sequence 2: single buffer — completes seq 2 but frontier stays at 0 (seq 1 incomplete)
    tracker.insert(SequenceRange(SequenceNumber(2), SequenceNumber(3)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    /// Sequence 1: complete — frontier advances to 2 (both 1 and 2 are now done)
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 2);
}

/// Many sequential complete sequences
TEST_F(RangeCompletionTrackerTest, ManySequentialCompleteSequences)
{
    RangeCompletionTracker<> tracker;

    for (size_t i = 1; i <= 100; ++i)
    {
        tracker.insert(SequenceRange(SequenceNumber(i), SequenceNumber(i + 1)));
        EXPECT_EQ(tracker.getCompletedUpTo(), i);
    }
}

/// Four-way split arriving in reverse order
TEST_F(RangeCompletionTrackerTest, FourWaySplitReverseOrder)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 3}), SequenceNumber(2)));
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(std::vector<size_t>{1, 3})));
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(std::vector<size_t>{1, 2})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
}

/// getHighestSeen tracks the max end of any inserted range
TEST_F(RangeCompletionTrackerTest, HighestSeenTracking)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)));
    EXPECT_EQ(tracker.getHighestSeen(), SequenceNumber(2));

    /// Insert a sub-range of sequence 7: [7, 7.1)
    tracker.insert(SequenceRange(SequenceNumber(7), SequenceNumber(std::vector<size_t>{7, 1})));
    EXPECT_EQ(tracker.getHighestSeen(), SequenceNumber(std::vector<size_t>{7, 1}));

    /// Insert a range with a lower end — highestSeen should not decrease
    tracker.insert(SequenceRange(SequenceNumber(3), SequenceNumber(4)));
    EXPECT_EQ(tracker.getHighestSeen(), SequenceNumber(std::vector<size_t>{7, 1}));
}

/// Frontier advancement with out-of-order completion
TEST_F(RangeCompletionTrackerTest, FrontierAdvancementOutOfOrder)
{
    RangeCompletionTracker<> tracker;

    /// Complete sequences 3, 2 (out of order, missing 1)
    tracker.insert(SequenceRange(SequenceNumber(3), SequenceNumber(4)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    tracker.insert(SequenceRange(SequenceNumber(2), SequenceNumber(3)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);

    /// Complete sequence 1 — frontier should jump to 3
    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 3);
}

/// Frontier with gap: 1,2,3 complete, then 5 complete, then 4 completes -> frontier = 5
TEST_F(RangeCompletionTrackerTest, FrontierGapFilling)
{
    RangeCompletionTracker<> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)));
    tracker.insert(SequenceRange(SequenceNumber(2), SequenceNumber(3)));
    tracker.insert(SequenceRange(SequenceNumber(3), SequenceNumber(4)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 3);

    tracker.insert(SequenceRange(SequenceNumber(5), SequenceNumber(6)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 3);

    tracker.insert(SequenceRange(SequenceNumber(4), SequenceNumber(5)));
    EXPECT_EQ(tracker.getCompletedUpTo(), 5);
}

/// Empty tracker: completedUpTo is 0, highestSeen is invalid
TEST_F(RangeCompletionTrackerTest, EmptyTracker)
{
    RangeCompletionTracker<> tracker;
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);
    EXPECT_FALSE(tracker.getHighestSeen().isValid());
}

/// --- Valued specialization tests ---

TEST_F(RangeCompletionTrackerTest, ValuedSingleInsert)
{
    RangeCompletionTracker<uint64_t> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)), uint64_t(100));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(100));
}

TEST_F(RangeCompletionTrackerTest, ValuedMaxCombine)
{
    RangeCompletionTracker<uint64_t> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(std::vector<size_t>{1, 1})), uint64_t(999));
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 1}), SequenceNumber(std::vector<size_t>{1, 2})), uint64_t(1));
    tracker.insert(SequenceRange(SequenceNumber(std::vector<size_t>{1, 2}), SequenceNumber(2)), uint64_t(500));
    EXPECT_EQ(tracker.getCompletedUpTo(), 1);
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(999));
}

TEST_F(RangeCompletionTrackerTest, ValuedNoCompletedValue)
{
    RangeCompletionTracker<uint64_t> tracker;
    EXPECT_EQ(tracker.getCompletedValue(), std::nullopt);
}

TEST_F(RangeCompletionTrackerTest, ValuedFrontierAdvancement)
{
    RangeCompletionTracker<uint64_t> tracker;

    /// Complete seq 2 first (out of order)
    tracker.insert(SequenceRange(SequenceNumber(2), SequenceNumber(3)), uint64_t(200));
    EXPECT_EQ(tracker.getCompletedUpTo(), 0);
    EXPECT_EQ(tracker.getCompletedValue(), std::nullopt);

    /// Complete seq 1 — frontier advances to 2
    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)), uint64_t(100));
    EXPECT_EQ(tracker.getCompletedUpTo(), 2);
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(200));
}

TEST_F(RangeCompletionTrackerTest, ValuedSequentialProgress)
{
    RangeCompletionTracker<uint64_t> tracker;

    tracker.insert(SequenceRange(SequenceNumber(1), SequenceNumber(2)), uint64_t(31));
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(31));

    tracker.insert(SequenceRange(SequenceNumber(2), SequenceNumber(3)), uint64_t(63));
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(63));

    /// Skip seq 3, insert seq 4 — frontier stays at 2
    tracker.insert(SequenceRange(SequenceNumber(4), SequenceNumber(5)), uint64_t(99));
    EXPECT_EQ(tracker.getCompletedUpTo(), 2);
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(63));

    /// Insert seq 3 — frontier jumps to 4
    tracker.insert(SequenceRange(SequenceNumber(3), SequenceNumber(4)), uint64_t(80));
    EXPECT_EQ(tracker.getCompletedUpTo(), 4);
    EXPECT_EQ(tracker.getCompletedValue(), uint64_t(99));
}

}
