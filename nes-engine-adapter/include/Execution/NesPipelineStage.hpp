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

#include <string>
#include <Execution/NesStageContext.hpp>
#include <Runtime/TupleBuffer.hpp>

namespace NES
{

/// Abstract base class for NES pipeline stages.
///
/// Subclasses must implement:
/// - doExecute(ctx, buffer) -- process a TupleBuffer
/// - getId()               -- return stage identifier
///
/// start() and stop() have default empty implementations since many stages
/// (especially sinks) don't need them.
class NesPipelineStage
{
public:
    virtual ~NesPipelineStage() = default;

    /// Called once when the pipeline starts. Default is no-op.
    virtual void start(NesStageContext& /*ctx*/) { }

    /// Process a TupleBuffer. Implemented by concrete NES pipeline stages and sinks.
    /// @param ctx Execution context for this invocation
    /// @param buffer The TupleBuffer to process
    virtual void doExecute(NesStageContext& ctx, TupleBuffer& buffer) = 0;

    /// Called once when the pipeline stops. Default is no-op.
    virtual void stop(NesStageContext& /*ctx*/) { }

    /// Get the unique identifier for this stage.
    /// @return Stage identifier string
    [[nodiscard]] virtual std::string getId() const = 0;
};

} /// namespace NES
