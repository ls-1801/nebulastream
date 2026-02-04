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

#include <BufferManagement/NesBufferProvider.hpp>
#include <Runtime/AbstractBufferProvider.hpp>
#include <ErrorHandling.hpp>

namespace NES
{

NesBufferWrapper::NesBufferWrapper(TupleBuffer buf) : buffer(std::move(buf)), metadata{}
{
    // Extract metadata from the TupleBuffer
    metadata.sequence_number = buffer.getSequenceNumber().getRawValue();
    metadata.origin_id = buffer.getOriginId().getRawValue();
    metadata.watermark = buffer.getWatermark().getRawValue();
    metadata.num_tuples = buffer.getNumberOfTuples();
    metadata.chunk_number = static_cast<uint32_t>(buffer.getChunkNumber().getRawValue());
    metadata.last_chunk = buffer.isLastChunk();
}

NesBufferProvider::NesBufferProvider(std::shared_ptr<AbstractBufferProvider> nesProvider)
    : nesProvider_(std::move(nesProvider))
{
    PRECONDITION(nesProvider_ != nullptr, "NesBufferProvider requires a valid AbstractBufferProvider");
}

adaptive_engine::BufferHandle NesBufferProvider::wrap(void* data, size_t /*size*/, const adaptive_engine::BufferMetadata& /*metadata*/)
{
    PRECONDITION(data != nullptr, "Cannot wrap null TupleBuffer pointer");

    // data is expected to be a TupleBuffer*
    auto* tupleBufferPtr = static_cast<TupleBuffer*>(data);

    // Retain the buffer to increment reference count
    tupleBufferPtr->retain();

    // Create wrapper with a copy of the TupleBuffer (which also increments ref count)
    auto* wrapper = new NesBufferWrapper(*tupleBufferPtr);

    // Now release our retain() since the copy in wrapper holds the ref
    tupleBufferPtr->release();

    return adaptive_engine::BufferHandle{wrapper};
}

void NesBufferProvider::release(adaptive_engine::BufferHandle handle)
{
    if (handle.opaque == nullptr)
    {
        return;
    }

    auto* wrapper = static_cast<NesBufferWrapper*>(handle.opaque);
    // Destructor of NesBufferWrapper will release the TupleBuffer
    delete wrapper;
}

void* NesBufferProvider::get_data(adaptive_engine::BufferHandle handle)
{
    PRECONDITION(handle.opaque != nullptr, "Cannot get data from null handle");

    auto* wrapper = static_cast<NesBufferWrapper*>(handle.opaque);
    auto memoryArea = wrapper->buffer.getAvailableMemoryArea<uint8_t>();
    return memoryArea.data();
}

size_t NesBufferProvider::get_size(adaptive_engine::BufferHandle handle)
{
    PRECONDITION(handle.opaque != nullptr, "Cannot get size from null handle");

    auto* wrapper = static_cast<NesBufferWrapper*>(handle.opaque);
    return wrapper->buffer.getBufferSize();
}

const adaptive_engine::BufferMetadata& NesBufferProvider::get_metadata(adaptive_engine::BufferHandle handle)
{
    PRECONDITION(handle.opaque != nullptr, "Cannot get metadata from null handle");

    auto* wrapper = static_cast<NesBufferWrapper*>(handle.opaque);
    return wrapper->metadata;
}

adaptive_engine::BufferHandle NesBufferProvider::allocate(size_t size)
{
    PRECONDITION(nesProvider_ != nullptr, "Buffer provider not initialized");

    std::optional<TupleBuffer> buffer;

    // If requested size fits in pooled buffer, use pooled allocation
    if (size <= nesProvider_->getBufferSize())
    {
        buffer = nesProvider_->getBufferNoBlocking();
    }
    else
    {
        // Otherwise use unpooled buffer of exact size
        buffer = nesProvider_->getUnpooledBuffer(size);
    }

    if (!buffer.has_value())
    {
        // Allocation failed - return null handle
        return adaptive_engine::BufferHandle{nullptr};
    }

    // Create wrapper holding the new buffer
    auto* wrapper = new NesBufferWrapper(std::move(buffer.value()));

    return adaptive_engine::BufferHandle{wrapper};
}

}  // namespace NES
