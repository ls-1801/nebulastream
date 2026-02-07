#pragma once

#include "Buffer.hpp"

#include <cstdint>

namespace adaptive_engine {

/// Abstract execution context passed to pipeline stages during execution
///
/// Provides methods for stages to emit buffers, request re-execution,
/// allocate buffers, and access execution metadata.
class ExecutionContext {
public:
    virtual ~ExecutionContext() = default;

    /// Emit a buffer to downstream stages
    /// @param handle The buffer to emit
    virtual void emit_buffer(BufferHandle handle) = 0;

    /// Request that this task be repeated after the current execution
    /// Used for sources that have more data to produce
    virtual void repeat_task() = 0;

    /// Allocate a new buffer using the buffer provider
    /// @param size Requested size in bytes
    /// @return Handle to the newly allocated buffer
    virtual BufferHandle allocate_buffer(size_t size) = 0;

    /// Get the worker ID executing this context
    /// @return Worker identifier (0-based)
    virtual uint32_t get_worker_id() const = 0;

    /// Get the pipeline ID this context belongs to
    /// @return Pipeline identifier
    virtual uint64_t get_pipeline_id() const = 0;

    /// Get the buffer provider for this context
    /// @return Pointer to the buffer provider
    virtual BufferProvider* get_buffer_provider() = 0;

    /// Get user-defined data associated with this query
    /// @return Opaque pointer to user data
    virtual void* get_user_data() = 0;
};

}  // namespace adaptive_engine
