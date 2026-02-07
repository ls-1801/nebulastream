#pragma once

#include <cstddef>
#include <cstdint>

namespace adaptive_engine {

/// Metadata associated with a buffer for stream processing
struct BufferMetadata {
    uint64_t sequence_number;  ///< Sequence number for ordering
    uint64_t origin_id;        ///< Identifier for the data source
    uint64_t watermark;        ///< Watermark timestamp for event-time processing
    uint64_t num_tuples;       ///< Number of tuples in the buffer
    uint32_t chunk_number;     ///< Current chunk number in a multi-chunk transfer
    bool last_chunk;           ///< True if this is the last chunk
};

/// Base class for buffer handle objects. Enables provider-free clone/release
/// via virtual dispatch through the FFI boundary.
struct BufferHandleBase {
    virtual ~BufferHandleBase() = default;
    /// Clone this handle (e.g., copy-construct wrapper, increment refcount).
    virtual BufferHandleBase* do_clone() = 0;
    /// Release this handle. Default: delete this.
    virtual void do_release() { delete this; }
};

/// Opaque handle to a buffer managed via BufferHandleBase virtual dispatch
struct BufferHandle {
    BufferHandleBase* opaque{nullptr};
};

}  // namespace adaptive_engine
