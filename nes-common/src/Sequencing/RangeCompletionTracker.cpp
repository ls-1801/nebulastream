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

namespace NES
{

void RangeCompletionTrackerBase::insert(const SequenceRange& range)
{
    insertAndMerge(range);
}

size_t RangeCompletionTrackerBase::getCompletedUpTo() const
{
    if (ranges_.empty())
    {
        return 0;
    }
    const auto& first = *ranges_.begin();
    if (first.start != SequenceNumber(1))
    {
        return 0;
    }
    /// If the range is [1, N+1) where N+1 is a root-level number, sequences 1..N are complete.
    /// If the range is [1, {N, ...}) sequences 1..N-1 are complete (N is partial).
    /// In both cases: completedUpTo = end.root() - 1.
    return first.end.root() - 1;
}

SequenceNumber RangeCompletionTrackerBase::getHighestSeen() const
{
    return highestSeen_;
}

size_t RangeCompletionTrackerBase::insertAndMerge(const SequenceRange& range)
{
    /// Track the highest end of any inserted range
    if (!highestSeen_.isValid() || range.end > highestSeen_)
    {
        highestSeen_ = range.end;
    }

    auto [it, inserted] = ranges_.insert(range);
    if (!inserted)
    {
        return getCompletedUpTo();
    }

    /// Try to merge with predecessor
    if (it != ranges_.begin())
    {
        auto prev = std::prev(it);
        if (prev->end == it->start)
        {
            SequenceRange merged(prev->start, it->end);
            ranges_.erase(prev);
            ranges_.erase(it);
            auto [newIt, ok] = ranges_.insert(merged);
            it = newIt;
        }
    }

    /// Try to merge with successor
    auto next = std::next(it);
    if (next != ranges_.end() && it->end == next->start)
    {
        SequenceRange merged(it->start, next->end);
        ranges_.erase(next);
        ranges_.erase(it);
        ranges_.insert(merged);
    }

    return getCompletedUpTo();
}

}
