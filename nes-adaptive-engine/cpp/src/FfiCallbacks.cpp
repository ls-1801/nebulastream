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

/// FfiCallbacks.cpp - C++ callback implementations for Rust FFI
///
/// These functions are called by Rust code when it needs to interact with
/// C++ PipelineStage, SourceHandle, and buffer handle objects.
///
/// IMPORTANT: These are extern "C" functions declared in the Rust code
/// (src/ffi/callbacks.rs) and must have matching signatures.

#include "adaptive_engine/Buffer.hpp"
#include "adaptive_engine/ExecutionContext.hpp"
#include "adaptive_engine/PipelineStage.hpp"
#include "adaptive_engine/SourceHandle.hpp"

#include <cstddef>
#include <cstdint>
#include <vector>
#include <cpptrace/from_current.hpp>

namespace
{

/// Data for a buffer emitted via ExecutionContext::emit_buffer during stage execution.
/// Only stores the opaque handle — the engine treats buffers as opaque.
struct EmittedBufferData
{
    /// The opaque handle value. Each emitted handle is independently owned
    /// (cloned at emit time) so Rust can release it independently.
    uintptr_t opaque_handle;
};

/// Thread-local storage for emitted buffers collected during stage_execute.
/// Safe because the executor is single-threaded.
thread_local std::vector<EmittedBufferData> g_emitted_buffer_data;

/// Thread-local flag for repeat_task requests during stage execution.
thread_local bool g_repeat_requested = false;

/// FFI execution context that collects emitted buffers in thread-local storage.
/// Used by stage_start/execute/stop and source_open/next_buffer/close callbacks.
class FfiExecutionContext : public adaptive_engine::ExecutionContext
{
public:
    FfiExecutionContext(void* context_ptr, uint32_t worker_id, uint64_t worker_count)
        : context_ptr_(context_ptr), worker_id_(worker_id), worker_count_(worker_count)
    {
    }

    void emit_buffer(adaptive_engine::BufferHandle handle) override
    {
        /// Clone the handle via virtual dispatch so each emitted buffer is independently owned.
        auto* cloned = handle.opaque->do_clone();
        EmittedBufferData ebd;
        ebd.opaque_handle = reinterpret_cast<uintptr_t>(cloned);
        g_emitted_buffer_data.push_back(ebd);
    }

    void repeat_task(adaptive_engine::BufferHandle /*handle*/) override
    {
        /// The handle is not stored on the C++ side. The Rust FFI bridge
        /// preserves the original buffer and passes it to the executor.
        g_repeat_requested = true;
    }

    uint32_t get_worker_id() const override { return worker_id_; }

    uint64_t get_worker_count() const override { return worker_count_; }

    uint64_t get_pipeline_id() const override { return 0; }

    void* get_user_data() override { return context_ptr_; }

private:
    void* context_ptr_;
    uint32_t worker_id_;
    uint64_t worker_count_;
};

} /// namespace

extern "C" {

/// =============================================================================
/// Pipeline Stage callbacks
/// =============================================================================

/// Call C++ PipelineStage::start()
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param context_ptr Opaque context pointer
/// @param worker_id Worker thread ID (0-based)
/// @param worker_count Total number of worker threads
/// @return 1 on success, 0 on failure
int32_t stage_start(uintptr_t stage_ptr, uintptr_t context_ptr, uint32_t worker_id, uint64_t worker_count)
{
    if (stage_ptr == 0)
    {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    CPPTRACE_TRY
    {
        FfiExecutionContext ctx(reinterpret_cast<void*>(context_ptr), worker_id, worker_count);
        stage->start(ctx);
        return 1;
    }
    CPPTRACE_CATCH(...)
    {
        return 0;
    }
    std::unreachable();
}

/// Call C++ PipelineStage::execute() with an opaque buffer handle.
///
/// Takes an opaque handle directly (e.g., NesBufferWrapper*),
/// preserving child buffers for variable-sized data. The handle is passed
/// directly to the stage without copying or re-wrapping.
///
/// NOTE: This function does NOT release the input buffer. The Rust side
/// manages buffer lifetime and will release it when dropped.
///
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param context_ptr Opaque context pointer
/// @param opaque_handle The opaque buffer handle to pass to the stage
/// @param worker_id Worker thread ID (0-based)
/// @param worker_count Total number of worker threads
/// @return 1 on success, 0 on failure
int32_t
stage_execute_with_handle(uintptr_t stage_ptr, uintptr_t context_ptr, uintptr_t opaque_handle, uint32_t worker_id, uint64_t worker_count)
{
    if (stage_ptr == 0 || opaque_handle == 0)
    {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);

    adaptive_engine::BufferHandle input{reinterpret_cast<adaptive_engine::BufferHandleBase*>(opaque_handle)};

    CPPTRACE_TRY
    {
        /// Clear TLS state for this execution
        g_emitted_buffer_data.clear();
        g_repeat_requested = false;

        FfiExecutionContext ctx(reinterpret_cast<void*>(context_ptr), worker_id, worker_count);
        stage->execute(ctx, input);

        /// Do NOT release the input buffer - Rust manages lifetime
        return 1;
    }
    CPPTRACE_CATCH(...)
    {
        /// Do NOT release - Rust manages lifetime
        return 0;
    }
    std::unreachable();
}

/// Call C++ PipelineStage::stop()
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param context_ptr Opaque context pointer
/// @param worker_id Worker thread ID (0-based)
/// @param worker_count Total number of worker threads
/// @return 1 on success, 0 on failure
int32_t stage_stop(uintptr_t stage_ptr, uintptr_t context_ptr, uint32_t worker_id, uint64_t worker_count)
{
    if (stage_ptr == 0)
    {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    CPPTRACE_TRY
    {
        /// Clear TLS state before stop
        g_emitted_buffer_data.clear();
        g_repeat_requested = false;

        FfiExecutionContext ctx(reinterpret_cast<void*>(context_ptr), worker_id, worker_count);
        stage->stop(ctx);
        return 1;
    }
    CPPTRACE_CATCH(...)
    {
        return 0;
    }
    std::unreachable();
}

/// =============================================================================
/// Emitted buffer retrieval (thread-local storage access)
/// =============================================================================

/// Get the number of buffers emitted during the last stage_execute call.
size_t stage_get_emitted_count()
{
    return g_emitted_buffer_data.size();
}

/// Get the opaque handle for an emitted buffer.
/// @param index Index into the emitted buffer list
/// @return The opaque handle value, or 0 if index is out of bounds
uintptr_t stage_get_emitted_opaque_handle(size_t index)
{
    if (index >= g_emitted_buffer_data.size())
        return 0;
    return g_emitted_buffer_data[index].opaque_handle;
}

/// Check if repeat_task was requested during the last stage execution.
bool stage_get_repeat_requested()
{
    return g_repeat_requested;
}

/// =============================================================================
/// Source Handle callbacks
/// =============================================================================

/// Call C++ SourceHandle::open()
/// @param source_ptr Pointer to C++ SourceHandle
/// @param context_ptr Opaque context pointer
/// @return 1 on success, 0 on failure
int32_t source_open(uintptr_t source_ptr, uintptr_t context_ptr)
{
    if (source_ptr == 0)
    {
        return 0;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    CPPTRACE_TRY
    {
        FfiExecutionContext ctx(reinterpret_cast<void*>(context_ptr), 0, 1);
        source->open(ctx);
        return 1;
    }
    CPPTRACE_CATCH(...)
    {
        return 0;
    }
    std::unreachable();
}

/// Call C++ SourceHandle::next_buffer()
/// @param source_ptr Pointer to C++ SourceHandle
/// @param context_ptr Opaque context pointer
/// @return Buffer handle (opaque pointer), 0 if source is exhausted (EOS),
///         or SIZE_MAX if an error occurred (exception thrown)
uintptr_t source_next_buffer(uintptr_t source_ptr, uintptr_t context_ptr)
{
    if (source_ptr == 0)
    {
        return 0;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    CPPTRACE_TRY
    {
        FfiExecutionContext ctx(reinterpret_cast<void*>(context_ptr), 0, 1);
        auto result = source->next_buffer(ctx);
        if (result.has_value())
        {
            return reinterpret_cast<uintptr_t>(result.value().opaque);
        }
        return 0; /// EOS (source exhausted)
    }
    CPPTRACE_CATCH(...)
    {
        return SIZE_MAX; /// Error sentinel - distinguishes error from EOS
    }
    std::unreachable();
}

/// Call C++ SourceHandle::request_stop() to signal the source to stop.
/// Unlike source_close(), this does NOT release resources - it only signals a
/// stop token so that a concurrent next_buffer() call returns promptly.
/// Safe to call from any thread while next_buffer() is in progress.
/// @param source_ptr Pointer to C++ SourceHandle
void source_request_stop(uintptr_t source_ptr)
{
    if (source_ptr == 0)
    {
        return;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    CPPTRACE_TRY
    {
        source->request_stop();
    }
    CPPTRACE_CATCH(...)
    {
        /// Swallow exceptions - request_stop is best-effort
    }
}

/// Call C++ SourceHandle::close()
/// @param source_ptr Pointer to C++ SourceHandle
/// @param context_ptr Opaque context pointer
/// @return 1 on success, 0 on failure
int32_t source_close(uintptr_t source_ptr, uintptr_t context_ptr)
{
    if (source_ptr == 0)
    {
        return 0;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    CPPTRACE_TRY
    {
        FfiExecutionContext ctx(reinterpret_cast<void*>(context_ptr), 0, 1);
        source->close(ctx);
        return 1;
    }
    CPPTRACE_CATCH(...)
    {
        return 0;
    }
    std::unreachable();
}

/// =============================================================================
/// Buffer handle callbacks (provider-free via virtual dispatch)
/// =============================================================================

/// Clone a buffer handle via virtual dispatch.
/// @param handle Buffer handle to clone
/// @return New independent handle (must be separately released)
uintptr_t buffer_handle_clone(uintptr_t handle)
{
    if (handle == 0)
    {
        return 0;
    }
    return reinterpret_cast<uintptr_t>(reinterpret_cast<adaptive_engine::BufferHandleBase*>(handle)->do_clone());
}

/// Release a buffer handle via virtual dispatch.
/// @param handle Buffer handle to release
void buffer_handle_release(uintptr_t handle)
{
    if (handle == 0)
    {
        return;
    }
    reinterpret_cast<adaptive_engine::BufferHandleBase*>(handle)->do_release();
}

/// =============================================================================
/// Object lifecycle callbacks
/// =============================================================================

/// Destroy a C++ SourceHandle object.
/// Called by Rust when CppSourceHandle is dropped.
/// @param source_ptr Pointer to C++ SourceHandle to delete
void source_destroy(uintptr_t source_ptr)
{
    if (source_ptr != 0)
    {
        delete reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    }
}

/// Destroy a C++ PipelineStage object.
/// Called by Rust when CppPipelineStage is dropped.
/// @param stage_ptr Pointer to C++ PipelineStage to delete
void stage_destroy(uintptr_t stage_ptr)
{
    if (stage_ptr != 0)
    {
        delete reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    }
}

} /// extern "C"
