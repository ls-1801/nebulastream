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

#include <algorithm>
#include <cstddef>
#include <map>
#include <optional>
#include <set>
#include <Sequencing/SequenceNumber.hpp>

namespace NES
{

/// Default combiner: std::max
struct MaxCombine
{
    template <typename T>
    T operator()(const T& a, const T& b) const
    {
        return std::max(a, b);
    }
};

/// Base class: pure range tracking (no associated values).
///
/// Maintains a single sorted set of non-overlapping ranges. On each insert,
/// the new range is merged with its neighbors. The completed frontier is
/// derived from the first range: if it starts at SequenceNumber(1) and
/// extends to SequenceNumber(N+1), then sequences 1..N are complete.
///
/// Single-threaded. Wrap in std::mutex or folly::Synchronized externally
/// if concurrent access is needed.
class RangeCompletionTrackerBase
{
public:
    /// Insert a range fragment and merge with adjacent ranges.
    void insert(const SequenceRange& range);

    /// Highest root sequence N where all [1,2), [2,3), ..., [N, N+1) are complete.
    /// Returns 0 when nothing is complete.
    [[nodiscard]] size_t getCompletedUpTo() const;

    /// Highest end of any range fragment ever inserted.
    [[nodiscard]] SequenceNumber getHighestSeen() const;

protected:
    /// Insert a range, merge with neighbors, and return the new completed frontier.
    size_t insertAndMerge(const SequenceRange& range);

    std::set<SequenceRange> ranges_;
    SequenceNumber highestSeen_;
};

/// Forward declaration of the primary template
template <typename T = void, typename Combine = MaxCombine>
class RangeCompletionTracker;

/// Void specialization: pure range tracking with no associated values.
template <>
class RangeCompletionTracker<void, MaxCombine> : public RangeCompletionTrackerBase
{
public:
    using RangeCompletionTrackerBase::insert;
    using RangeCompletionTrackerBase::getCompletedUpTo;
    using RangeCompletionTrackerBase::getHighestSeen;
};

/// Valued specialization: associates a value T with each range fragment.
/// Values are aggregated per root sequence using the Combine functor.
template <typename T, typename Combine>
class RangeCompletionTracker : public RangeCompletionTrackerBase
{
public:
    /// Insert a range fragment with an associated value.
    void insert(const SequenceRange& range, T value)
    {
        auto rootSeq = range.rootSequence();

        /// Combine the value for this root sequence
        auto it = values_.find(rootSeq);
        if (it != values_.end())
        {
            it->second = combine_(it->second, value);
        }
        else
        {
            values_.emplace(rootSeq, value);
        }

        /// Insert range and get new frontier
        auto prevFrontier = getCompletedUpTo();
        insertAndMerge(range);
        auto newFrontier = getCompletedUpTo();

        /// Clean up values below the new frontier
        if (newFrontier > prevFrontier)
        {
            auto keepFrom = values_.lower_bound(newFrontier);
            values_.erase(values_.begin(), keepFrom);
        }
    }

    /// Value at the completed frontier sequence (combined across all fragments).
    /// Returns nullopt if completedUpTo == 0.
    [[nodiscard]] std::optional<T> getCompletedValue() const
    {
        auto frontier = getCompletedUpTo();
        if (frontier == 0)
        {
            return std::nullopt;
        }
        auto it = values_.find(frontier);
        if (it != values_.end())
        {
            return it->second;
        }
        return std::nullopt;
    }

private:
    Combine combine_{};
    std::map<size_t, T> values_; /// root seq -> combined value
};

}
