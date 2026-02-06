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

#include <stdexcept>
#include <adaptive_engine/Buffer.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include <adaptive_engine/PipelineStage.hpp>
#include <BufferManagement/NesBufferProvider.hpp>
#include <Runtime/TupleBuffer.hpp>

namespace NES
{

/// Abstract base class that bridges adaptive_engine::PipelineStage to NES's TupleBuffer-based execution model.
///
/// This class handles the BufferHandle → TupleBuffer conversion in one place, so concrete NES pipeline stages
/// and sinks only need to implement doExecute() with a TupleBuffer reference. The TupleBuffer is extracted from
/// the NesBufferWrapper stored in the BufferHandle's opaque pointer. Reference counting is handled automatically
/// via RAII — when the TupleBuffer copy goes out of scope after doExecute() returns, the ref count decrements.
///
/// Subclasses must implement:
/// - doExecute(ctx, buffer) — process a TupleBuffer
/// - get_id() — return stage identifier
///
/// start() and stop() have default empty implementations since many stages (especially sinks) don't need them.
class NesPipelineStage : public adaptive_engine::PipelineStage
{
public:
    ~NesPipelineStage() override = default;

    /// Called once when the pipeline starts. Default is no-op.
    void start(adaptive_engine::ExecutionContext& /*ctx*/) override { }

    /// Extracts TupleBuffer from BufferHandle and delegates to doExecute().
    void execute(adaptive_engine::ExecutionContext& ctx, adaptive_engine::BufferHandle input) override
    {
        if (input.opaque == nullptr)
        {
            throw std::runtime_error("Cannot execute with null buffer handle");
        }

        auto* wrapper = static_cast<NesBufferWrapper*>(input.opaque);
        TupleBuffer buffer = wrapper->buffer;  // copies TupleBuffer (increments ref count)
        doExecute(ctx, buffer);
    }  // buffer destructor decrements ref count

    /// Called once when the pipeline stops. Default is no-op.
    void stop(adaptive_engine::ExecutionContext& /*ctx*/) override { }

protected:
    /// Process a TupleBuffer. Implemented by concrete NES pipeline stages and sinks.
    /// @param ctx Execution context for this invocation
    /// @param buffer The TupleBuffer to process
    virtual void doExecute(adaptive_engine::ExecutionContext& ctx, TupleBuffer& buffer) = 0;
};

}  // namespace NES
