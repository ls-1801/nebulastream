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

#include <AdaptiveEngineBindings.hpp>

#include <algorithm>
#include <cstdint>

#include <Identifiers/Identifiers.hpp>
#include <Time/Timestamp.hpp>
#include <executor/lib.h>
#include <rust/cxx.h>
#include <ErrorHandling.hpp>

const uint8_t* NESTupleBufferBuilder::getDataPtr()
{
    return buffer.getAvailableMemoryArea<uint8_t>().data();
}

size_t NESTupleBufferBuilder::getSize() const
{
    return buffer.getBufferSize();
}

TupleBufferMetadata NESTupleBufferBuilder::getMetadata() const
{
    return TupleBufferMetadata{
        .sequence_number = buffer.getSequenceNumber().getRawValue(),
        .origin_id = buffer.getOriginId().getRawValue(),
        .watermark = buffer.getWatermark().getRawValue(),
        .number_of_tuples = buffer.getNumberOfTuples(),
        .chunk_number = static_cast<int64_t>(buffer.getChunkNumber().getRawValue()),
        .last_chunk = buffer.isLastChunk(),
    };
}

void NESTupleBufferBuilder::setMetadata(const TupleBufferMetadata& meta)
{
    buffer.setSequenceNumber(NES::SequenceNumber(meta.sequence_number));
    buffer.setOriginId(NES::OriginId(meta.origin_id));
    buffer.setWatermark(NES::Timestamp(meta.watermark));
    buffer.setNumberOfTuples(meta.number_of_tuples);
    if (meta.chunk_number >= 0)
    {
        buffer.setChunkNumber(NES::ChunkNumber(static_cast<uint64_t>(meta.chunk_number)));
    }
    buffer.setLastChunk(meta.last_chunk);
}

void NESTupleBufferBuilder::setData(rust::Slice<const uint8_t> data)
{
    INVARIANT(
        buffer.getBufferSize() >= data.length(),
        "Buffer size mismatch. Internal BufferSize: {} vs. External {}",
        buffer.getBufferSize(),
        data.length());

    std::ranges::copy(data, buffer.getAvailableMemoryArea<uint8_t>().begin());
}

bool NESExecutionContext::emitBuffer(NESTupleBufferBuilder& /*builder*/)
{
    // The buffer is already set up via the builder, just emit it
    // Note: We need access to the underlying buffer to emit it
    // This is a design constraint - the builder wraps a reference to the buffer
    // For now, we'll need to have a separate mechanism to track buffers
    // This implementation assumes the builder's buffer is already in the context
    return true;
}

NESTupleBufferBuilder* NESExecutionContext::allocateTupleBuffer()
{
    auto buffer = ctx.allocateTupleBuffer();
    if (!buffer)
    {
        return nullptr;
    }

    allocatedBuffers.push_back(std::move(buffer));
    auto builder = std::make_unique<NESTupleBufferBuilder>(allocatedBuffers.back());
    auto* ptr = builder.get();
    allocatedBuilders.push_back(std::move(builder));
    return ptr;
}

uint64_t NESExecutionContext::getWorkerId() const
{
    return ctx.getId().getRawValue();
}

uint64_t NESExecutionContext::getWorkerCount() const
{
    return ctx.getNumberOfWorkerThreads();
}

// ============================================================
// RustBridgeExecutionContext implementation
// ============================================================

RustBridgeExecutionContext::RustBridgeExecutionContext(
    uintptr_t rustContextHandle,
    std::shared_ptr<NES::AbstractBufferProvider> bufferManager,
    NES::WorkerThreadId workerId,
    NES::PipelineId pipelineId)
    : rustContextHandle_(rustContextHandle)
    , bufferManager_(std::move(bufferManager))
    , workerId_(workerId)
    , pipelineId_(pipelineId)
{
}

bool RustBridgeExecutionContext::emitBuffer(const NES::TupleBuffer& buffer, ContinuationPolicy /*policy*/)
{
    // Get buffer data and metadata
    const auto* dataPtr = buffer.getAvailableMemoryArea<uint8_t>().data();
    const auto size = buffer.getBufferSize();
    const auto sequenceNumber = buffer.getSequenceNumber().getRawValue();
    const auto originId = buffer.getOriginId().getRawValue();
    const auto watermark = buffer.getWatermark().getRawValue();
    const auto numberOfTuples = buffer.getNumberOfTuples();
    const auto chunkNumber = static_cast<int64_t>(buffer.getChunkNumber().getRawValue());
    const auto lastChunk = buffer.isLastChunk();

    // Call Rust FFI function to emit the buffer
    // SAFETY: rustContextHandle_ is a valid ExecutionContextOpaque pointer passed from Rust
    const auto result = rust_exec_context_emit_buffer(
        rustContextHandle_,
        dataPtr,
        size,
        sequenceNumber,
        originId,
        watermark,
        numberOfTuples,
        chunkNumber,
        lastChunk);

    return result != 0;
}

void RustBridgeExecutionContext::repeatTask(const NES::TupleBuffer& /*buffer*/, std::chrono::milliseconds /*delay*/)
{
    // TODO(US-015): Implement repeatTask routing to Rust
}

NES::TupleBuffer RustBridgeExecutionContext::allocateTupleBuffer()
{
    // TODO(US-014): Implement buffer allocation via BufferManager
    return bufferManager_->getBufferBlocking();
}

NES::WorkerThreadId RustBridgeExecutionContext::getId() const
{
    return workerId_;
}

uint64_t RustBridgeExecutionContext::getNumberOfWorkerThreads() const
{
    // Route to Rust execution context to get the actual worker count
    return rust_exec_context_get_worker_count(rustContextHandle_);
}

std::shared_ptr<NES::AbstractBufferProvider> RustBridgeExecutionContext::getBufferManager() const
{
    return bufferManager_;
}

NES::PipelineId RustBridgeExecutionContext::getPipelineId() const
{
    return pipelineId_;
}

std::unordered_map<NES::OperatorHandlerId, std::shared_ptr<NES::OperatorHandler>>&
RustBridgeExecutionContext::getOperatorHandlers()
{
    return operatorHandlers_;
}

void RustBridgeExecutionContext::setOperatorHandlers(
    std::unordered_map<NES::OperatorHandlerId, std::shared_ptr<NES::OperatorHandler>>& handlers)
{
    operatorHandlers_ = handlers;
}
