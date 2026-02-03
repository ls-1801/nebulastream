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

#include <chrono>
#include <cstdint>
#include <memory>
#include <unordered_map>
#include <rust/cxx.h>

#include <Identifiers/Identifiers.hpp>
#include <PipelineExecutionContext.hpp>
#include <Runtime/AbstractBufferProvider.hpp>
#include <Runtime/Execution/OperatorHandler.hpp>
#include <Runtime/TupleBuffer.hpp>

struct TupleBufferMetadata;

/// The NESTupleBufferBuilder is a wrapper around a NES TupleBuffer with methods exposed to Rust.
/// This allows Rust code to read/write data and metadata from/to a TupleBuffer without
/// directly exposing the TupleBuffer internals.
///
/// It is important that the underlying TupleBuffer outlives the NESTupleBufferBuilder.
class NESTupleBufferBuilder
{
public:
    explicit NESTupleBufferBuilder(NES::TupleBuffer& buffer) : buffer(buffer) {}

    /// Get pointer to buffer data
    /// Note: Non-const because CXX bridge uses Pin<&mut>
    const uint8_t* getDataPtr();

    /// Get buffer size in bytes
    size_t getSize() const;

    /// Get buffer metadata
    TupleBufferMetadata getMetadata() const;

    /// Set buffer metadata (for output buffers)
    void setMetadata(const TupleBufferMetadata& meta);

    /// Set buffer data (copies data into the buffer)
    void setData(rust::Slice<const uint8_t> data);

private:
    NES::TupleBuffer& buffer; ///NOLINT(cppcoreguidelines-avoid-const-or-ref-data-members)
};

/// The NESExecutionContext is a wrapper around NES::PipelineExecutionContext
/// that exposes methods to Rust for emitting buffers and allocation.
class NESExecutionContext
{
public:
    explicit NESExecutionContext(NES::PipelineExecutionContext& ctx) : ctx(ctx) {}

    /// Emit a buffer to downstream pipelines
    bool emitBuffer(NESTupleBufferBuilder& builder);

    /// Allocate a new tuple buffer and return a builder for it
    /// Note: Returns a raw pointer that the caller must manage
    NESTupleBufferBuilder* allocateTupleBuffer();

    /// Get worker thread ID
    uint64_t getWorkerId() const;

    /// Get number of worker threads
    uint64_t getWorkerCount() const;

private:
    NES::PipelineExecutionContext& ctx; ///NOLINT(cppcoreguidelines-avoid-const-or-ref-data-members)

    /// Storage for allocated buffers (keeps them alive while builders exist)
    std::vector<NES::TupleBuffer> allocatedBuffers;

    /// Storage for allocated builders
    std::vector<std::unique_ptr<NESTupleBufferBuilder>> allocatedBuilders;
};

/// RustBridgeExecutionContext implements NES::PipelineExecutionContext and routes
/// all calls back to the Rust adaptive engine via FFI.
///
/// This class is used when C++ ExecutablePipelineStage instances need to interact
/// with the Rust execution context. When a wrapped C++ stage calls emitBuffer(),
/// allocateTupleBuffer(), etc., those calls are forwarded to the Rust context.
///
/// # Ownership
///
/// - The Rust context handle is owned by Rust; this class holds a borrowed pointer
/// - The BufferManager pointer is owned by NES; this class holds a borrowed pointer
/// - The OperatorHandlers map is stored by value but may reference shared_ptrs
///
/// # Thread Safety
///
/// This class is designed to be used from a single worker thread at a time.
/// The underlying Rust context handles any necessary synchronization.
class RustBridgeExecutionContext : public NES::PipelineExecutionContext
{
public:
    /// Create a new RustBridgeExecutionContext.
    ///
    /// @param rustContextHandle Opaque handle to the Rust ExecutionContextOpaque
    /// @param bufferManager Pointer to the NES BufferManager for buffer allocation
    /// @param workerId The worker thread ID for this context
    /// @param pipelineId The pipeline ID this context is executing
    RustBridgeExecutionContext(
        uintptr_t rustContextHandle,
        std::shared_ptr<NES::AbstractBufferProvider> bufferManager,
        NES::WorkerThreadId workerId,
        NES::PipelineId pipelineId);

    ~RustBridgeExecutionContext() override = default;

    // Disable copy
    RustBridgeExecutionContext(const RustBridgeExecutionContext&) = delete;
    RustBridgeExecutionContext& operator=(const RustBridgeExecutionContext&) = delete;

    // Enable move
    RustBridgeExecutionContext(RustBridgeExecutionContext&&) = default;
    RustBridgeExecutionContext& operator=(RustBridgeExecutionContext&&) = default;

    // =========================================================================
    // PipelineExecutionContext interface implementation
    // =========================================================================

    /// Emit a buffer to downstream pipelines via Rust FFI.
    ///
    /// @param buffer The buffer to emit
    /// @param policy Whether immediate continuation is allowed
    /// @return true if the buffer was successfully emitted
    bool emitBuffer(const NES::TupleBuffer& buffer, ContinuationPolicy policy) override;

    /// Schedule re-execution of the current task after a delay.
    ///
    /// @param buffer The buffer to re-process
    /// @param delay How long to wait before re-execution
    void repeatTask(const NES::TupleBuffer& buffer, std::chrono::milliseconds delay) override;

    /// Allocate a new tuple buffer from the buffer manager.
    ///
    /// @return A new TupleBuffer, or an invalid buffer if allocation failed
    NES::TupleBuffer allocateTupleBuffer() override;

    /// Get the worker thread ID.
    [[nodiscard]] NES::WorkerThreadId getId() const override;

    /// Get the total number of worker threads.
    [[nodiscard]] uint64_t getNumberOfWorkerThreads() const override;

    /// Get the buffer manager.
    [[nodiscard]] std::shared_ptr<NES::AbstractBufferProvider> getBufferManager() const override;

    /// Get the pipeline ID.
    [[nodiscard]] NES::PipelineId getPipelineId() const override;

    /// Get the operator handlers map.
    std::unordered_map<NES::OperatorHandlerId, std::shared_ptr<NES::OperatorHandler>>& getOperatorHandlers() override;

    /// Set the operator handlers map.
    void setOperatorHandlers(
        std::unordered_map<NES::OperatorHandlerId, std::shared_ptr<NES::OperatorHandler>>& handlers) override;

private:
    /// Opaque handle to the Rust ExecutionContextOpaque
    /// This is passed through FFI calls to route back to Rust
    uintptr_t rustContextHandle_;

    /// Buffer manager for allocating TupleBuffers
    std::shared_ptr<NES::AbstractBufferProvider> bufferManager_;

    /// Operator handlers map
    std::unordered_map<NES::OperatorHandlerId, std::shared_ptr<NES::OperatorHandler>> operatorHandlers_;

    /// Worker thread ID
    NES::WorkerThreadId workerId_;

    /// Pipeline ID
    NES::PipelineId pipelineId_;
};
