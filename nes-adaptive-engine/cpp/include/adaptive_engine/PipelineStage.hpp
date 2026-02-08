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

#include "Buffer.hpp"
#include "ExecutionContext.hpp"

#include <cstdint>
#include <string>

namespace adaptive_engine
{

/// Abstract interface for a pipeline stage
///
/// Pipeline stages process buffers through the start/execute/stop lifecycle.
/// Implementations wrap user-defined operators (e.g., filter, map, aggregate).
class PipelineStage
{
public:
    virtual ~PipelineStage() = default;

    /// Called once when the pipeline starts
    /// @param ctx Execution context for this invocation
    virtual void start(ExecutionContext& ctx) = 0;

    /// Called for each input buffer to process
    /// @param ctx Execution context for this invocation
    /// @param input The input buffer to process
    virtual void execute(ExecutionContext& ctx, BufferHandle input) = 0;

    /// Called once when the pipeline stops
    /// @param ctx Execution context for this invocation
    virtual void stop(ExecutionContext& ctx) = 0;

    /// Get the unique identifier for this stage
    /// @return Stage identifier string
    virtual std::string get_id() const = 0;
};

} /// namespace adaptive_engine
