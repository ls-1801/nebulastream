// FfiCallbacks.cpp - C++ callback implementations for Rust FFI
//
// These functions are called by Rust code when it needs to interact with
// C++ PipelineStage, SourceHandle, and BufferProvider objects.
//
// IMPORTANT: These are extern "C" functions declared in the Rust code
// (src/ffi/callbacks.rs) and must have matching signatures.

#include "adaptive_engine/PipelineStage.hpp"
#include "adaptive_engine/SourceHandle.hpp"
#include "adaptive_engine/Buffer.hpp"
#include "adaptive_engine/ExecutionContext.hpp"

#include <cstdint>
#include <cstddef>
#include <vector>

namespace {

/// Data for a buffer emitted via ExecutionContext::emit_buffer during stage execution.
/// Stores the opaque handle directly to preserve child buffers for variable-sized data.
struct EmittedBufferData {
    /// The opaque handle value (e.g., NesBufferWrapper*).
    /// Ownership is transferred to Rust via stage_get_emitted_opaque_handle().
    uintptr_t opaque_handle;
    /// Cached metadata extracted at emit time.
    adaptive_engine::BufferMetadata metadata;
    /// Cached data pointer from the handle at emit time.
    uintptr_t data_ptr;
    /// Cached data size from the handle at emit time.
    size_t data_size;
};

/// Thread-local storage for emitted buffers collected during stage_execute.
/// Safe because the executor is single-threaded.
thread_local std::vector<EmittedBufferData> g_emitted_buffer_data;

/// Thread-local flag for repeat_task requests during stage execution.
thread_local bool g_repeat_requested = false;

/// FFI execution context that collects emitted buffers in thread-local storage.
/// Used by stage_start/execute/stop and source_open/next_buffer/close callbacks.
class FfiExecutionContext : public adaptive_engine::ExecutionContext {
public:
    explicit FfiExecutionContext(adaptive_engine::BufferProvider* provider)
        : provider_(provider) {}

    void emit_buffer(adaptive_engine::BufferHandle handle) override {
        // Store the opaque handle directly to preserve child buffers.
        // Cache data pointer, size, and metadata for Rust access.
        EmittedBufferData ebd;
        ebd.opaque_handle = reinterpret_cast<uintptr_t>(handle.opaque);
        ebd.data_ptr = reinterpret_cast<uintptr_t>(provider_->get_data(handle));
        ebd.data_size = provider_->get_size(handle);
        ebd.metadata = provider_->get_metadata(handle);
        g_emitted_buffer_data.push_back(ebd);
    }

    void repeat_task() override {
        g_repeat_requested = true;
    }

    adaptive_engine::BufferHandle allocate_buffer(size_t size) override {
        return provider_->allocate(size);
    }

    uint32_t get_worker_id() const override { return 0; }
    uint64_t get_pipeline_id() const override { return 0; }
    adaptive_engine::BufferProvider* get_buffer_provider() override { return provider_; }
    void* get_user_data() override { return nullptr; }

private:
    adaptive_engine::BufferProvider* provider_;
};

}  // namespace

extern "C" {

// =============================================================================
// Pipeline Stage callbacks
// =============================================================================

/// Call C++ PipelineStage::start()
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param provider_ptr Pointer to C++ BufferProvider
/// @return 1 on success, 0 on failure
int32_t stage_start(uintptr_t stage_ptr, uintptr_t provider_ptr) {
    if (stage_ptr == 0) {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    try {
        FfiExecutionContext ctx(provider);
        stage->start(ctx);
        return 1;
    } catch (...) {
        return 0;
    }
}

/// Call C++ PipelineStage::execute() with proper buffer wrapping.
///
/// Creates a BufferHandle from raw data via the provider, executes the stage,
/// and collects emitted buffers in thread-local storage for retrieval by Rust.
///
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param provider_ptr Pointer to C++ BufferProvider
/// @param data_ptr Pointer to input buffer data
/// @param data_size Size of input buffer in bytes
/// @param sequence_number Buffer sequence number
/// @param origin_id Buffer origin ID
/// @param watermark Buffer watermark
/// @param num_tuples Number of tuples in buffer
/// @param chunk_number Chunk number
/// @param last_chunk Whether this is the last chunk
/// @return 1 on success, 0 on failure
int32_t stage_execute(
    uintptr_t stage_ptr,
    uintptr_t provider_ptr,
    uintptr_t data_ptr,
    size_t data_size,
    uint64_t sequence_number,
    uint64_t origin_id,
    uint64_t watermark,
    uint64_t num_tuples,
    uint32_t chunk_number,
    bool last_chunk
) {
    if (stage_ptr == 0 || data_ptr == 0) {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);

    // Wrap raw data into a proper BufferHandle via the provider
    adaptive_engine::BufferMetadata metadata{
        sequence_number, origin_id, watermark, num_tuples, chunk_number, last_chunk
    };
    adaptive_engine::BufferHandle input = provider->wrap(
        reinterpret_cast<void*>(data_ptr), data_size, metadata);

    if (input.opaque == nullptr) {
        return 0;
    }

    try {
        // Clear TLS state for this execution
        g_emitted_buffer_data.clear();
        g_repeat_requested = false;

        FfiExecutionContext ctx(provider);
        stage->execute(ctx, input);

        // Release the input buffer
        provider->release(input);

        return 1;
    } catch (...) {
        // Release the input buffer even on failure to prevent leaks
        provider->release(input);
        return 0;
    }
}

/// Call C++ PipelineStage::execute() with an opaque buffer handle.
///
/// This variant takes an opaque handle directly (e.g., NesBufferWrapper*),
/// preserving child buffers for variable-sized data. The handle is passed
/// directly to the stage without copying or re-wrapping.
///
/// NOTE: This function does NOT release the input buffer. The Rust side
/// manages buffer lifetime via Arc reference counting and will release
/// the buffer when the last reference is dropped.
///
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param provider_ptr Pointer to C++ BufferProvider
/// @param opaque_handle The opaque buffer handle to pass to the stage
/// @return 1 on success, 0 on failure
int32_t stage_execute_with_handle(
    uintptr_t stage_ptr,
    uintptr_t provider_ptr,
    uintptr_t opaque_handle
) {
    if (stage_ptr == 0 || opaque_handle == 0) {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);

    adaptive_engine::BufferHandle input{reinterpret_cast<void*>(opaque_handle)};

    try {
        // Clear TLS state for this execution
        g_emitted_buffer_data.clear();
        g_repeat_requested = false;

        FfiExecutionContext ctx(provider);
        stage->execute(ctx, input);

        // Do NOT release the input buffer - Rust manages lifetime via Arc
        return 1;
    } catch (...) {
        // Do NOT release - Rust manages lifetime via Arc
        return 0;
    }
}

/// Call C++ PipelineStage::stop()
/// @param stage_ptr Pointer to C++ PipelineStage
/// @param provider_ptr Pointer to C++ BufferProvider
/// @return 1 on success, 0 on failure
int32_t stage_stop(uintptr_t stage_ptr, uintptr_t provider_ptr) {
    if (stage_ptr == 0) {
        return 0;
    }
    auto* stage = reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    try {
        // Clear TLS state before stop
        g_emitted_buffer_data.clear();
        g_repeat_requested = false;

        FfiExecutionContext ctx(provider);
        stage->stop(ctx);
        return 1;
    } catch (...) {
        return 0;
    }
}

// =============================================================================
// Emitted buffer retrieval (thread-local storage access)
// =============================================================================

/// Get the number of buffers emitted during the last stage_execute call.
size_t stage_get_emitted_count() {
    return g_emitted_buffer_data.size();
}

/// Get the data pointer for an emitted buffer (cached at emit time).
/// @param index Index into the emitted buffer list
/// @return Pointer to buffer data, or 0 if index is out of bounds
uintptr_t stage_get_emitted_data_ptr(size_t index) {
    if (index >= g_emitted_buffer_data.size()) return 0;
    return g_emitted_buffer_data[index].data_ptr;
}

/// Get the data size for an emitted buffer (cached at emit time).
/// @param index Index into the emitted buffer list
/// @return Size of buffer data, or 0 if index is out of bounds
size_t stage_get_emitted_data_size(size_t index) {
    if (index >= g_emitted_buffer_data.size()) return 0;
    return g_emitted_buffer_data[index].data_size;
}

/// Get the opaque handle for an emitted buffer.
/// @param index Index into the emitted buffer list
/// @return The opaque handle value, or 0 if index is out of bounds
uintptr_t stage_get_emitted_opaque_handle(size_t index) {
    if (index >= g_emitted_buffer_data.size()) return 0;
    return g_emitted_buffer_data[index].opaque_handle;
}

/// Get metadata for an emitted buffer via out parameters.
/// @param index Index into the emitted buffer list
void stage_get_emitted_metadata(
    size_t index,
    uint64_t* seq_out,
    uint64_t* origin_out,
    uint64_t* watermark_out,
    uint64_t* num_tuples_out,
    uint32_t* chunk_number_out,
    bool* last_chunk_out
) {
    if (index >= g_emitted_buffer_data.size()) return;
    const auto& meta = g_emitted_buffer_data[index].metadata;
    if (seq_out) *seq_out = meta.sequence_number;
    if (origin_out) *origin_out = meta.origin_id;
    if (watermark_out) *watermark_out = meta.watermark;
    if (num_tuples_out) *num_tuples_out = meta.num_tuples;
    if (chunk_number_out) *chunk_number_out = meta.chunk_number;
    if (last_chunk_out) *last_chunk_out = meta.last_chunk;
}

/// Check if repeat_task was requested during the last stage execution.
bool stage_get_repeat_requested() {
    return g_repeat_requested;
}

// =============================================================================
// Source Handle callbacks
// =============================================================================

/// Call C++ SourceHandle::open()
/// @param source_ptr Pointer to C++ SourceHandle
/// @param provider_ptr Pointer to C++ BufferProvider
/// @return 1 on success, 0 on failure
int32_t source_open(uintptr_t source_ptr, uintptr_t provider_ptr) {
    if (source_ptr == 0) {
        return 0;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    try {
        FfiExecutionContext ctx(provider);
        source->open(ctx);
        return 1;
    } catch (...) {
        return 0;
    }
}

/// Call C++ SourceHandle::next_buffer()
/// @param source_ptr Pointer to C++ SourceHandle
/// @param provider_ptr Pointer to C++ BufferProvider
/// @return Buffer handle (opaque pointer), 0 if source is exhausted (EOS),
///         or SIZE_MAX if an error occurred (exception thrown)
uintptr_t source_next_buffer(uintptr_t source_ptr, uintptr_t provider_ptr) {
    if (source_ptr == 0) {
        return 0;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    try {
        FfiExecutionContext ctx(provider);
        auto result = source->next_buffer(ctx);
        if (result.has_value()) {
            return reinterpret_cast<uintptr_t>(result.value().opaque);
        }
        return 0;  // EOS (source exhausted)
    } catch (...) {
        return SIZE_MAX;  // Error sentinel - distinguishes error from EOS
    }
}

/// Call C++ SourceHandle::request_stop() to signal the source to stop.
/// Unlike source_close(), this does NOT release resources - it only signals a
/// stop token so that a concurrent next_buffer() call returns promptly.
/// Safe to call from any thread while next_buffer() is in progress.
/// @param source_ptr Pointer to C++ SourceHandle
void source_request_stop(uintptr_t source_ptr) {
    if (source_ptr == 0) {
        return;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    try {
        source->request_stop();
    } catch (...) {
        // Swallow exceptions - request_stop is best-effort
    }
}

/// Call C++ SourceHandle::close()
/// @param source_ptr Pointer to C++ SourceHandle
/// @param provider_ptr Pointer to C++ BufferProvider
/// @return 1 on success, 0 on failure
int32_t source_close(uintptr_t source_ptr, uintptr_t provider_ptr) {
    if (source_ptr == 0) {
        return 0;
    }
    auto* source = reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    try {
        FfiExecutionContext ctx(provider);
        source->close(ctx);
        return 1;
    } catch (...) {
        return 0;
    }
}

// =============================================================================
// Buffer Provider callbacks
// =============================================================================

/// Get data pointer from a buffer handle
/// @param provider_ptr Pointer to C++ BufferProvider
/// @param handle Buffer handle (opaque pointer)
/// @return Pointer to buffer data
uintptr_t buffer_provider_get_data(uintptr_t provider_ptr, uintptr_t handle) {
    if (provider_ptr == 0) {
        return 0;
    }
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    adaptive_engine::BufferHandle bh;
    bh.opaque = reinterpret_cast<void*>(handle);
    return reinterpret_cast<uintptr_t>(provider->get_data(bh));
}

/// Get size of a buffer
/// @param provider_ptr Pointer to C++ BufferProvider
/// @param handle Buffer handle (opaque pointer)
/// @return Size of buffer in bytes
size_t buffer_provider_get_size(uintptr_t provider_ptr, uintptr_t handle) {
    if (provider_ptr == 0) {
        return 0;
    }
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    adaptive_engine::BufferHandle bh;
    bh.opaque = reinterpret_cast<void*>(handle);
    return provider->get_size(bh);
}

/// Get metadata from a buffer handle via out parameters.
/// @param provider_ptr Pointer to C++ BufferProvider
/// @param handle Buffer handle (opaque pointer)
void buffer_provider_get_metadata(
    uintptr_t provider_ptr,
    uintptr_t handle,
    uint64_t* seq_out,
    uint64_t* origin_out,
    uint64_t* watermark_out,
    uint64_t* num_tuples_out,
    uint32_t* chunk_number_out,
    bool* last_chunk_out
) {
    if (provider_ptr == 0) return;
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    adaptive_engine::BufferHandle bh;
    bh.opaque = reinterpret_cast<void*>(handle);
    const auto& meta = provider->get_metadata(bh);
    if (seq_out) *seq_out = meta.sequence_number;
    if (origin_out) *origin_out = meta.origin_id;
    if (watermark_out) *watermark_out = meta.watermark;
    if (num_tuples_out) *num_tuples_out = meta.num_tuples;
    if (chunk_number_out) *chunk_number_out = meta.chunk_number;
    if (last_chunk_out) *last_chunk_out = meta.last_chunk;
}

/// Release a buffer handle
/// @param provider_ptr Pointer to C++ BufferProvider
/// @param handle Buffer handle to release
void buffer_provider_release(uintptr_t provider_ptr, uintptr_t handle) {
    if (provider_ptr == 0) {
        return;
    }
    auto* provider = reinterpret_cast<adaptive_engine::BufferProvider*>(provider_ptr);
    adaptive_engine::BufferHandle bh;
    bh.opaque = reinterpret_cast<void*>(handle);
    provider->release(bh);
}

// =============================================================================
// Object lifecycle callbacks
// =============================================================================

/// Destroy a C++ SourceHandle object.
/// Called by Rust when CppSourceHandle is dropped.
/// @param source_ptr Pointer to C++ SourceHandle to delete
void source_destroy(uintptr_t source_ptr) {
    if (source_ptr != 0) {
        delete reinterpret_cast<adaptive_engine::SourceHandle*>(source_ptr);
    }
}

/// Destroy a C++ PipelineStage object.
/// Called by Rust when CppPipelineStage is dropped.
/// @param stage_ptr Pointer to C++ PipelineStage to delete
void stage_destroy(uintptr_t stage_ptr) {
    if (stage_ptr != 0) {
        delete reinterpret_cast<adaptive_engine::PipelineStage*>(stage_ptr);
    }
}

}  // extern "C"
