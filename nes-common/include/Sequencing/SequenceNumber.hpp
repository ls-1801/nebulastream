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
#include <functional>
#include <ostream>
#include <string>
#include <vector>
#include <Util/Logger/Formatter.hpp>

namespace NES
{

/// Hierarchical sequence number for tracking buffer lineage.
/// Sequence numbers are represented as a series of components,
/// e.g., {1} = "1", {1,1} = "1.1", {1,0,1} = "1.0.1".
/// This matches the Rust SequenceNumber type in nes-adaptive-engine/src/sequence.rs.
class SequenceNumber
{
    std::vector<size_t> components_;

public:
    /// Default constructor creates an INVALID (empty) sequence number
    SequenceNumber();

    /// Create a root-level sequence number, e.g., SequenceNumber(1) = {1}
    explicit SequenceNumber(size_t root);

    /// Create from explicit components, e.g., {1,2,3} = "1.2.3"
    explicit SequenceNumber(std::vector<size_t> components);

    /// Create a child sequence number by appending a component
    [[nodiscard]] SequenceNumber child(size_t offset) const;

    /// Get the depth (number of components)
    [[nodiscard]] size_t depth() const;

    /// Get a reference to the components
    [[nodiscard]] const std::vector<size_t>& components() const;

    /// Get the first (root) component. Returns 0 if empty.
    [[nodiscard]] size_t root() const;

    /// Check if this is a valid (non-empty) sequence number
    [[nodiscard]] bool isValid() const;

    /// Lexicographic comparison (matches Rust Vec<u64> Ord)
    friend auto operator<=>(const SequenceNumber& lhs, const SequenceNumber& rhs) = default;
    friend bool operator==(const SequenceNumber& lhs, const SequenceNumber& rhs) = default;

    /// Display
    friend std::ostream& operator<<(std::ostream& os, const SequenceNumber& seq);
    [[nodiscard]] std::string toString() const;
};

/// Global constants replacing the old constexpr values
inline const SequenceNumber INVALID_SEQ_NUMBER{};
inline const SequenceNumber INITIAL_SEQ_NUMBER{size_t(1)};

/// A half-open range [start, end) of hierarchical sequence numbers.
/// When a source produces buffer N, it assigns range [N, N+1).
/// Splitting a buffer subdivides its range.
struct SequenceRange
{
    SequenceNumber start; /// inclusive
    SequenceNumber end; /// exclusive

    SequenceRange() = default;
    SequenceRange(SequenceNumber start, SequenceNumber end);

    /// Check if both start and end are valid
    [[nodiscard]] bool isValid() const;

    /// Check if this range covers exactly one integer interval [n, n+1)
    [[nodiscard]] bool isComplete() const;

    /// Get the root sequence number from the start
    [[nodiscard]] size_t rootSequence() const;

    /// Comparison operators for ordered containers
    friend auto operator<=>(const SequenceRange& lhs, const SequenceRange& rhs) = default;
    friend bool operator==(const SequenceRange& lhs, const SequenceRange& rhs) = default;

    friend std::ostream& operator<<(std::ostream& os, const SequenceRange& range);
    [[nodiscard]] std::string toString() const;
};

}

FMT_OSTREAM(NES::SequenceNumber);
FMT_OSTREAM(NES::SequenceRange);

namespace std
{
template <>
struct hash<NES::SequenceNumber>
{
    size_t operator()(const NES::SequenceNumber& seq) const
    {
        size_t seed = 0;
        for (const auto& component : seq.components())
        {
            /// hash_combine from boost
            seed ^= std::hash<size_t>()(component) + 0x9e3779b9 + (seed << 6) + (seed >> 2);
        }
        return seed;
    }
};
}
