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
#include <ostream>
#include <Identifiers/Identifiers.hpp>
#include <Runtime/Execution/OperatorHandler.hpp>
#include <Runtime/QueryTerminationType.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <Sequencing/SequenceNumber.hpp>
#include <Util/Logger/Formatter.hpp>
#include <folly/Synchronized.h>

namespace NES
{

/// Key identifying a unique input range per origin
struct RangeForOriginId
{
    SequenceRange inputRange;
    OriginId originId = INVALID_ORIGIN_ID;

    auto operator<=>(const RangeForOriginId&) const = default;

    friend std::ostream& operator<<(std::ostream& os, const RangeForOriginId& obj)
    {
        return os << "{ range = " << obj.inputRange << ", originId = " << obj.originId << "}";
    }
};

/// Tracks sub-range assignment for a single input range
struct RangeState
{
    size_t nextChild = 1; /// counter for child offsets
};

class EmitOperatorHandler final : public OperatorHandler
{
public:
    /// Assign a sub-range to the output buffer based on the input range.
    /// @param inputRange pointer to the SequenceRange of the input buffer (from ExecutionContext)
    /// @param closesChunk true if this is the last buffer emitted for this input range
    /// @param originId the origin of the buffer
    /// @param outputBuffer the buffer being emitted, will have its SequenceRange set
    void assignRange(const SequenceRange* inputRange, bool closesChunk, OriginId originId, TupleBuffer& outputBuffer);

    void start(PipelineExecutionContext& pipelineExecutionContext, uint32_t localStateVariableId) override;
    void stop(QueryTerminationType terminationType, PipelineExecutionContext& pipelineExecutionContext) override;

    folly::Synchronized<std::map<RangeForOriginId, RangeState>> rangeStates;
};
}

FMT_OSTREAM(NES::RangeForOriginId);
