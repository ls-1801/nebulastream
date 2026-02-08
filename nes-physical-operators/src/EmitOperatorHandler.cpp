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

#include <EmitOperatorHandler.hpp>

#include <Identifiers/Identifiers.hpp>
#include <Runtime/QueryTerminationType.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <Sequencing/SequenceNumber.hpp>
#include <Util/Logger/Logger.hpp>
#include <ErrorHandling.hpp>
#include <PipelineExecutionContext.hpp>

namespace NES
{

void EmitOperatorHandler::assignRange(const SequenceRange* inputRange, bool closesChunk, OriginId originId, TupleBuffer& outputBuffer)
{
    PRECONDITION(inputRange != nullptr, "Expects a valid input range pointer");
    PRECONDITION(inputRange->isValid(), "Expects a valid input range");

    RangeForOriginId key{*inputRange, originId};
    const auto lock = rangeStates.wlock();

    auto& state = (*lock)[key];

    if (closesChunk)
    {
        if (state.nextChild == 1)
        {
            /// Only one buffer emitted for this input range — pass through unchanged
            outputBuffer.setSequenceRange(*inputRange);
        }
        else
        {
            /// Final buffer for this input range: close up to the original end
            /// Range is [start.child(nextChild-1), end)
            auto subStart = inputRange->start.child(state.nextChild - 1);
            outputBuffer.setSequenceRange(SequenceRange(std::move(subStart), inputRange->end));
        }
        lock->erase(key);
    }
    else
    {
        /// Non-final buffer: [start.child(nextChild-1), start.child(nextChild))
        auto subStart = inputRange->start.child(state.nextChild - 1);
        auto subEnd = inputRange->start.child(state.nextChild);
        state.nextChild++;
        outputBuffer.setSequenceRange(SequenceRange(std::move(subStart), std::move(subEnd)));
    }
}

void EmitOperatorHandler::start(PipelineExecutionContext&, uint32_t)
{
}

void EmitOperatorHandler::stop(QueryTerminationType, PipelineExecutionContext&)
{
}

}
