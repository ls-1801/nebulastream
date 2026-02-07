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
#include <ostream>
#include <Sequencing/SequenceNumber.hpp>
#include <Util/Logger/Formatter.hpp>

namespace NES
{

/// SequenceData wraps a SequenceRange for buffer tracking.
/// It provides the root sequence number and completeness information.
struct SequenceData
{
    explicit SequenceData(SequenceRange range);
    explicit SequenceData();

    friend std::ostream& operator<<(std::ostream& os, const SequenceData& obj);

    friend auto operator<=>(const SequenceData& lhs, const SequenceData& rhs) = default;

    SequenceRange range;
};

}

FMT_OSTREAM(NES::SequenceData);
