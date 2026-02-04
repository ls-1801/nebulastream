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

#include <memory>
#include <adaptive_engine/Buffer.hpp>
#include <Runtime/TupleBuffer.hpp>

namespace NES
{

class AbstractBufferProvider;

/// Internal wrapper that holds a TupleBuffer and its cached metadata.
/// This is stored as the opaque pointer in BufferHandle.
struct NesBufferWrapper
{
    TupleBuffer buffer;
    adaptive_engine::BufferMetadata metadata;

    explicit NesBufferWrapper(TupleBuffer buf);
};

/// BufferProvider implementation that wraps NES TupleBuffer instances.
///
/// This class bridges the adaptive_engine::BufferProvider interface to NES's
/// buffer management system. It wraps TupleBuffer objects and manages their
/// lifecycle through reference counting.
class NesBufferProvider final : public adaptive_engine::BufferProvider
{
public:
    /// Create a NesBufferProvider backed by the given NES buffer provider.
    /// @param nesProvider The underlying NES buffer provider (e.g., BufferManager)
    explicit NesBufferProvider(std::shared_ptr<AbstractBufferProvider> nesProvider);

    ~NesBufferProvider() override = default;

    /// Wrap an existing TupleBuffer pointer.
    /// @param data Pointer to a TupleBuffer* (NOT the data pointer)
    /// @param size Ignored (size comes from TupleBuffer)
    /// @param metadata Ignored (metadata comes from TupleBuffer)
    /// @return Handle to the wrapped buffer
    /// @note The TupleBuffer's reference count is incremented via retain()
    adaptive_engine::BufferHandle wrap(void* data, size_t size, const adaptive_engine::BufferMetadata& metadata) override;

    /// Release a buffer handle, decrementing the TupleBuffer's reference count.
    /// @param handle The buffer handle to release
    void release(adaptive_engine::BufferHandle handle) override;

    /// Get the data pointer for a buffer.
    /// @param handle The buffer handle
    /// @return Pointer to the TupleBuffer's data region
    void* get_data(adaptive_engine::BufferHandle handle) override;

    /// Get the size of a buffer.
    /// @param handle The buffer handle
    /// @return Size of the buffer in bytes
    size_t get_size(adaptive_engine::BufferHandle handle) override;

    /// Get the metadata for a buffer.
    /// @param handle The buffer handle
    /// @return Reference to the buffer's metadata
    const adaptive_engine::BufferMetadata& get_metadata(adaptive_engine::BufferHandle handle) override;

    /// Allocate a new buffer of the specified size.
    /// @param size Requested size in bytes (pooled buffers if <= pool size, unpooled otherwise)
    /// @return Handle to the newly allocated buffer
    adaptive_engine::BufferHandle allocate(size_t size) override;

private:
    std::shared_ptr<AbstractBufferProvider> nesProvider_;
};

}  // namespace NES
