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

#include <Sequencing/SequenceNumber.hpp>

#include <ostream>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace NES
{

SequenceNumber::SequenceNumber() = default;

SequenceNumber::SequenceNumber(size_t root) : components_({root})
{
}

SequenceNumber::SequenceNumber(std::vector<size_t> components) : components_(std::move(components))
{
}

SequenceNumber SequenceNumber::child(size_t offset) const
{
    auto childComponents = components_;
    childComponents.push_back(offset);
    return SequenceNumber(std::move(childComponents));
}

size_t SequenceNumber::depth() const
{
    return components_.size();
}

const std::vector<size_t>& SequenceNumber::components() const
{
    return components_;
}

size_t SequenceNumber::root() const
{
    return components_.empty() ? 0 : components_[0];
}

bool SequenceNumber::isValid() const
{
    return !components_.empty();
}

std::ostream& operator<<(std::ostream& os, const SequenceNumber& seq)
{
    if (seq.components_.empty())
    {
        return os << "INVALID";
    }
    for (size_t i = 0; i < seq.components_.size(); ++i)
    {
        if (i > 0)
        {
            os << '.';
        }
        os << seq.components_[i];
    }
    return os;
}

std::string SequenceNumber::toString() const
{
    std::ostringstream oss;
    oss << *this;
    return oss.str();
}

SequenceRange::SequenceRange(SequenceNumber start, SequenceNumber end) : start(std::move(start)), end(std::move(end))
{
}

bool SequenceRange::isValid() const
{
    return start.isValid() && end.isValid();
}

bool SequenceRange::isComplete() const
{
    /// A range is "complete" if it covers exactly [n, n+1) for some integer n
    /// That means: start has depth 1 (root-level), end has depth 1, and end.root() == start.root() + 1
    return start.isValid() && end.isValid() && start.depth() == 1 && end.depth() == 1 && end.root() == start.root() + 1;
}

size_t SequenceRange::rootSequence() const
{
    return start.root();
}

std::ostream& operator<<(std::ostream& os, const SequenceRange& range)
{
    return os << "[" << range.start << ", " << range.end << ")";
}

std::string SequenceRange::toString() const
{
    std::ostringstream oss;
    oss << *this;
    return oss.str();
}

}
