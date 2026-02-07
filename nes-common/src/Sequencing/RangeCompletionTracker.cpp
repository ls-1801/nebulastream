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

namespace NES
{

std::optional<std::pair<size_t, Timestamp>> RangeCompletionTracker::insert(const SequenceRange& range, Timestamp watermark)
{
    auto rootSeq = range.rootSequence();
    auto& pending = pending_[rootSeq];

    pending.ranges.insert(range);
    pending.maxWatermark = std::max(pending.maxWatermark, watermark.getRawValue());

    mergeAdjacentRanges(pending.ranges);

    /// Check if the merged set contains a single range that covers [n, n+1)
    if (pending.ranges.size() == 1)
    {
        const auto& merged = *pending.ranges.begin();
        if (merged.isComplete())
        {
            auto result = std::make_pair(rootSeq, Timestamp(pending.maxWatermark));
            pending_.erase(rootSeq);
            return result;
        }
    }

    return std::nullopt;
}

void RangeCompletionTracker::mergeAdjacentRanges(std::set<SequenceRange>& ranges)
{
    if (ranges.size() <= 1)
    {
        return;
    }

    bool merged = true;
    while (merged)
    {
        merged = false;
        for (auto it = ranges.begin(); it != ranges.end(); ++it)
        {
            auto next = std::next(it);
            if (next == ranges.end())
            {
                break;
            }
            /// If current range's end equals next range's start, merge them
            if (it->end == next->start)
            {
                SequenceRange combined(it->start, next->end);
                ranges.erase(it, std::next(next));
                ranges.insert(combined);
                merged = true;
                break;
            }
        }
    }
}

}
