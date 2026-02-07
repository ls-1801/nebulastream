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

#include <NesSourceHandle.hpp>

#include <chrono>
#include <optional>
#include <string>
#include <utility>
#include <adaptive_engine/Buffer.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include <BufferManagement/NesBufferProvider.hpp>
#include <Identifiers/Identifiers.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <Time/Timestamp.hpp>
#include <ErrorHandling.hpp>
#include <Util/Logger/Logger.hpp>
#include <fmt/format.h>

namespace NES
{


NesSourceHandle::NesSourceHandle(
    std::unique_ptr<Source> source,
    OriginId originId,
    std::shared_ptr<AbstractBufferProvider> bufferProvider)
    : source_(std::move(source)), originId_(originId), bufferProvider_(std::move(bufferProvider))
{
    PRECONDITION(source_ != nullptr, "NesSourceHandle requires a valid Source");
    PRECONDITION(bufferProvider_ != nullptr, "NesSourceHandle requires a valid buffer provider");
}

std::optional<adaptive_engine::BufferHandle> NesSourceHandle::next_buffer(adaptive_engine::ExecutionContext& /*ctx*/)
{
    PRECONDITION(opened_, "Source must be opened before calling next_buffer");

    // Get stop token for this source
    std::stop_token stopToken = stopSource_.get_token();
    if (stopToken.stop_requested())
    {
        return std::nullopt;
    }

    // Allocate a buffer from the buffer provider
    // Try to get a pooled buffer with timeout
    std::optional<TupleBuffer> buffer;
    while (!buffer && !stopToken.stop_requested())
    {
        buffer = bufferProvider_->getBufferWithTimeout(std::chrono::milliseconds(25));
    }

    if (stopToken.stop_requested() || !buffer.has_value())
    {
        return std::nullopt;
    }

    // Fill the buffer using the underlying Source
    Source::FillTupleBufferResult result = source_->fillTupleBuffer(*buffer, stopToken);

    if (result.isEoS())
    {
        // End of stream - source is exhausted
        NES_DEBUG("NesSourceHandle {}: End of stream", originId_);
        return std::nullopt;
    }

    // Set buffer metadata
    const bool requiresMetadata = !source_->addsMetadata();
    if (requiresMetadata)
    {
        buffer->setOriginId(originId_);
        buffer->setSequenceNumber(SequenceNumber(sequenceNumber_.fetch_add(1)));
        buffer->setCreationTimestampInMS(Timestamp(
            std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::high_resolution_clock::now().time_since_epoch())
                .count()));
        buffer->setChunkNumber(INITIAL_CHUNK_NUMBER);
        buffer->setLastChunk(true);
    }

    // Set the number of bytes read (Source uses this to communicate size)
    buffer->setNumberOfTuples(result.getNumberOfBytes());

    // Wrap the TupleBuffer into a BufferHandle
    auto* wrapper = new NesBufferWrapper(std::move(*buffer));

    return adaptive_engine::BufferHandle{wrapper};
}

void NesSourceHandle::open(adaptive_engine::ExecutionContext& /*ctx*/)
{
    PRECONDITION(!opened_, "Source is already opened");

    NES_DEBUG("NesSourceHandle {}: Opening source", originId_);
    source_->open(bufferProvider_);
    opened_ = true;
}

void NesSourceHandle::close(adaptive_engine::ExecutionContext& /*ctx*/)
{
    if (!opened_)
    {
        return;
    }

    NES_DEBUG("NesSourceHandle {}: Closing source", originId_);
    stopSource_.request_stop();
    source_->close();
    opened_ = false;
}

std::string NesSourceHandle::get_id() const
{
    return fmt::format("source-{}", originId_.getRawValue());
}

void NesSourceHandle::request_stop()
{
    stopSource_.request_stop();
}

}  // namespace NES
