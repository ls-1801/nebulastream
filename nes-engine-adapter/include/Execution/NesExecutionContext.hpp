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

#include <cstdint>
#include <functional>
#include <adaptive_engine/ExecutionContext.hpp>
#include <BufferManagement/NesBufferProvider.hpp>

namespace NES
{

/// Callback type for emitting buffers to downstream stages.
/// The callback receives the BufferHandle that should be sent to successors.
using EmitBufferCallback = std::function<void(adaptive_engine::BufferHandle)>;

/// Callback type for requesting task re-execution.
/// The callback is invoked when a stage needs to be re-executed (e.g., a source has more data).
using RepeatTaskCallback = std::function<void()>;

/// ExecutionContext implementation that bridges NES runtime to the adaptive_engine interface.
///
/// This class implements the adaptive_engine::ExecutionContext interface and provides
/// NES-specific functionality for pipeline execution. It wraps a NesBufferProvider for
/// buffer management and uses callbacks to route buffers and schedule task re-execution.
class NesExecutionContext final : public adaptive_engine::ExecutionContext
{
public:
    /// Construct a NesExecutionContext.
    /// @param bufferProvider The NesBufferProvider for buffer allocation and management
    /// @param workerId The worker thread ID executing this context
    /// @param pipelineId The pipeline ID this context belongs to
    /// @param userData Opaque pointer to user-defined data (e.g., OperatorHandlers)
    /// @param emitCallback Callback invoked when emit_buffer() is called
    /// @param repeatCallback Callback invoked when repeat_task() is called
    NesExecutionContext(
        NesBufferProvider* bufferProvider,
        uint32_t workerId,
        uint64_t pipelineId,
        void* userData,
        EmitBufferCallback emitCallback,
        RepeatTaskCallback repeatCallback);

    ~NesExecutionContext() override = default;

    // Non-copyable, non-movable (context is tied to a specific execution)
    NesExecutionContext(const NesExecutionContext&) = delete;
    NesExecutionContext& operator=(const NesExecutionContext&) = delete;
    NesExecutionContext(NesExecutionContext&&) = delete;
    NesExecutionContext& operator=(NesExecutionContext&&) = delete;

    /// Emit a buffer to downstream stages.
    /// @param handle The buffer to emit
    void emit_buffer(adaptive_engine::BufferHandle handle) override;

    /// Request that this task be repeated with the given buffer.
    /// Used for sources that have more data to produce.
    void repeat_task(adaptive_engine::BufferHandle handle) override;

    /// Get the worker ID executing this context.
    /// @return Worker identifier (0-based)
    [[nodiscard]] uint32_t get_worker_id() const override;

    /// Get the pipeline ID this context belongs to.
    /// @return Pipeline identifier
    [[nodiscard]] uint64_t get_pipeline_id() const override;

    /// Get user-defined data associated with this query.
    /// @return Opaque pointer to user data (e.g., OperatorHandlers map)
    void* get_user_data() override;

private:
    NesBufferProvider* bufferProvider_;
    uint32_t workerId_;
    uint64_t pipelineId_;
    void* userData_;
    EmitBufferCallback emitCallback_;
    RepeatTaskCallback repeatCallback_;
};

}  // namespace NES
