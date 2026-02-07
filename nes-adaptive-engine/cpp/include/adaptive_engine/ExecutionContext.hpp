#pragma once

#include "Buffer.hpp"

#include <cstdint>

namespace adaptive_engine {

/// Abstract execution context passed to pipeline stages during execution
///
/// Provides methods for stages to emit buffers, request re-execution,
/// and access execution metadata.
class ExecutionContext {
public:
    virtual ~ExecutionContext() = default;

    /// Emit a buffer to downstream stages
    /// @param handle The buffer to emit
    virtual void emit_buffer(BufferHandle handle) = 0;

    /// Request that this task be repeated with the given buffer.
    /// The engine re-enqueues the buffer as-is without copying or modifying it.
    /// @param handle The buffer to re-execute with
    virtual void repeat_task(BufferHandle handle) = 0;

    /// Get the worker ID executing this context
    /// @return Worker identifier (0-based)
    virtual uint32_t get_worker_id() const = 0;

    /// Get the pipeline ID this context belongs to
    /// @return Pipeline identifier
    virtual uint64_t get_pipeline_id() const = 0;

    /// Get user-defined data associated with this query
    /// @return Opaque pointer to user data
    virtual void* get_user_data() = 0;
};

}  // namespace adaptive_engine
