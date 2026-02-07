#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>

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

/// Opaque handle to a buffer managed by the BufferProvider
struct BufferHandle {
    void* opaque;  ///< Opaque pointer to provider-specific buffer data
};

/// Abstract interface for buffer memory management
///
/// Implementations manage buffer allocation, lifecycle, and data access.
/// The engine uses this interface to work with various buffer backends
/// (e.g., NebulaStream TupleBuffer, custom memory pools).
class BufferProvider {
public:
    virtual ~BufferProvider() = default;

    /// Wrap an existing buffer with the provider
    /// @param data Pointer to raw buffer data
    /// @param size Size of the buffer in bytes
    /// @param metadata Metadata to associate with the buffer
    /// @return Handle to the wrapped buffer
    virtual BufferHandle wrap(void* data, size_t size, const BufferMetadata& metadata) = 0;

    /// Release a buffer handle, freeing associated resources
    /// @param handle The buffer handle to release
    virtual void release(BufferHandle handle) = 0;

    /// Get the data pointer for a buffer
    /// @param handle The buffer handle
    /// @return Pointer to the buffer's data
    virtual void* get_data(BufferHandle handle) = 0;

    /// Get the size of a buffer
    /// @param handle The buffer handle
    /// @return Size of the buffer in bytes
    virtual size_t get_size(BufferHandle handle) = 0;

    /// Get the metadata for a buffer
    /// @param handle The buffer handle
    /// @return Reference to the buffer's metadata
    virtual const BufferMetadata& get_metadata(BufferHandle handle) = 0;

    /// Allocate a new buffer of the specified size
    /// @param size Requested size in bytes
    /// @return Handle to the newly allocated buffer
    virtual BufferHandle allocate(size_t size) = 0;
};

}  // namespace adaptive_engine
