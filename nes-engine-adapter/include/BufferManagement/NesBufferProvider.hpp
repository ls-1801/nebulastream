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

/// Internal wrapper that holds a TupleBuffer.
/// Inherits BufferHandleBase so clone/release work via virtual dispatch
/// without requiring a BufferProvider.
struct NesBufferWrapper : adaptive_engine::BufferHandleBase
{
    TupleBuffer buffer;

    explicit NesBufferWrapper(TupleBuffer buf);

    adaptive_engine::BufferHandleBase* do_clone() override { return new NesBufferWrapper(buffer); }
    // do_release() default (delete this) is correct
};

/// NES buffer management utility for allocating and wrapping buffers.
///
/// This class bridges NES's buffer management system to the adaptive engine.
/// It is NOT an adaptive_engine::BufferProvider — buffer clone/release are
/// handled by BufferHandleBase virtual dispatch.
class NesBufferProvider
{
public:
    /// Create a NesBufferProvider backed by the given NES buffer provider.
    /// @param nesProvider The underlying NES buffer provider (e.g., BufferManager)
    explicit NesBufferProvider(std::shared_ptr<AbstractBufferProvider> nesProvider);

    ~NesBufferProvider() = default;

    /// Get the data pointer for a buffer.
    /// @param handle The buffer handle
    /// @return Pointer to the TupleBuffer's data region
    void* get_data(adaptive_engine::BufferHandle handle);

    /// Get the size of a buffer.
    /// @param handle The buffer handle
    /// @return Size of the buffer in bytes
    size_t get_size(adaptive_engine::BufferHandle handle);

    /// Allocate a new buffer of the specified size.
    /// @param size Requested size in bytes (pooled buffers if <= pool size, unpooled otherwise)
    /// @return Handle to the newly allocated buffer
    adaptive_engine::BufferHandle allocate(size_t size);

    /// Get the underlying NES buffer provider.
    /// @return Shared pointer to the underlying AbstractBufferProvider
    [[nodiscard]] std::shared_ptr<AbstractBufferProvider> getUnderlyingProvider() const { return nesProvider_; }

private:
    std::shared_ptr<AbstractBufferProvider> nesProvider_;
};

}  // namespace NES
