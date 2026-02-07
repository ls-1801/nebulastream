#pragma once

#include "Buffer.hpp"
#include "ExecutionContext.hpp"

#include <optional>
#include <string>

namespace adaptive_engine {

/// Abstract interface for data sources
///
/// Sources produce buffers for pipeline processing. They are opened once,
/// produce buffers via next_buffer until exhausted, then closed.
class SourceHandle {
public:
    virtual ~SourceHandle() = default;

    /// Get the next buffer from the source
    /// @param ctx Execution context for this invocation
    /// @return Buffer handle if available, nullopt if source is exhausted
    virtual std::optional<BufferHandle> next_buffer(ExecutionContext& ctx) = 0;

    /// Open the source for reading
    /// @param ctx Execution context for this invocation
    virtual void open(ExecutionContext& ctx) = 0;

    /// Close the source
    /// @param ctx Execution context for this invocation
    virtual void close(ExecutionContext& ctx) = 0;

    /// Get the unique identifier for this source
    /// @return Source identifier string
    virtual std::string get_id() const = 0;

    /// Request the source to stop producing data.
    /// This should signal any internal stop mechanism (e.g., stop tokens) so that
    /// a concurrent next_buffer() call returns promptly. Unlike close(), this must
    /// NOT destroy or release any resources — it only signals the intent to stop.
    /// Default implementation does nothing (safe for sources with non-blocking next_buffer).
    virtual void request_stop() {}
};

}  // namespace adaptive_engine
