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

#pragma once

#include <cstddef>
#include <map>
#include <optional>
#include <set>
#include <utility>
#include <Sequencing/SequenceNumber.hpp>
#include <Time/Timestamp.hpp>

namespace NES
{

/// Tracks completeness of fractional sequence ranges. Accepts sub-ranges
/// that may arrive out of order and detects when a full integer interval
/// [n, n+1) is completely covered.
///
/// Single-threaded. Wrap in std::mutex or folly::Synchronized externally
/// if concurrent access is needed.
class RangeCompletionTracker
{
public:
    /// Insert a completed range. Returns the root sequence number and
    /// associated watermark if the full integer interval [n, n+1) is now covered.
    std::optional<std::pair<size_t, Timestamp>> insert(const SequenceRange& range, Timestamp watermark);

private:
    struct PendingSequence
    {
        std::set<SequenceRange> ranges;
        Timestamp::Underlying maxWatermark = 0;
    };

    /// Try to merge adjacent ranges in the set. Two ranges are adjacent
    /// if one's end equals the other's start.
    static void mergeAdjacentRanges(std::set<SequenceRange>& ranges);

    std::map<size_t, PendingSequence> pending_;
};

}
