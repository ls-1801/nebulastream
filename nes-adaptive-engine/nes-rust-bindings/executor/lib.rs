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

//! CXX bridge for NebulaStream integration with adaptive-engine.
//!
//! This module provides wrapper types and FFI functions to integrate the
//! adaptive-engine executor with NebulaStream's pipeline execution model.

use adaptive_engine::ffi::buffer::BufferHandleOpaque;
use adaptive_engine::ffi::execution_context::ExecutionContextOpaque;
use adaptive_engine::ffi::handles::{
    BufferVecHandle, ExecutorHandleOpaque, ExecutorOpaqueHandle, GraphBuilderOpaque,
};
use std::pin::Pin;

#[cxx::bridge]
pub mod ffi {
    /// Result codes for executor operations
    enum ExecutorResult {
        Ok,
        AlreadyStarted,
        NotStarted,
        InvalidHandle,
        BufferError,
        GraphError,
        Unknown,
    }

    /// Metadata for a TupleBuffer, matching NebulaStream's buffer format
    struct TupleBufferMetadata {
        sequence_number: u64,
        origin_id: u64,
        watermark: u64,
        number_of_tuples: u64,
        chunk_number: i64,
        last_chunk: bool,
    }

    unsafe extern "C++" {
        include!("AdaptiveEngineBindings.hpp");

        /// C++ wrapper for NebulaStream TupleBuffer
        type NESTupleBufferBuilder;

        /// Get pointer to buffer data
        #[allow(non_snake_case)]
        unsafe fn getDataPtr(self: Pin<&mut NESTupleBufferBuilder>) -> *const u8;

        /// Get buffer size in bytes
        #[allow(non_snake_case)]
        fn getSize(self: &NESTupleBufferBuilder) -> usize;

        /// Get buffer metadata
        #[allow(non_snake_case)]
        fn getMetadata(self: &NESTupleBufferBuilder) -> TupleBufferMetadata;

        /// Set buffer metadata (for output buffers)
        #[allow(non_snake_case)]
        fn setMetadata(self: Pin<&mut NESTupleBufferBuilder>, meta: &TupleBufferMetadata);

        /// Set buffer data (copies data into the buffer)
        #[allow(non_snake_case)]
        fn setData(self: Pin<&mut NESTupleBufferBuilder>, data: &[u8]);

        /// C++ execution context wrapper
        type NESExecutionContext;

        /// Emit a buffer to downstream pipelines
        #[allow(non_snake_case)]
        fn emitBuffer(self: Pin<&mut NESExecutionContext>, builder: Pin<&mut NESTupleBufferBuilder>) -> bool;

        /// Allocate a new tuple buffer
        #[allow(non_snake_case)]
        fn allocateTupleBuffer(self: Pin<&mut NESExecutionContext>) -> *mut NESTupleBufferBuilder;

        /// Get worker thread ID
        #[allow(non_snake_case)]
        fn getWorkerId(self: &NESExecutionContext) -> u64;

        /// Get number of worker threads
        #[allow(non_snake_case)]
        fn getWorkerCount(self: &NESExecutionContext) -> u64;
    }

    extern "Rust" {
        /// Opaque handle to the adaptive-engine executor
        type AdaptiveExecutor;

        /// Opaque handle for submitting work to the executor
        type AdaptiveExecutorHandle;

        /// Opaque handle for building pipeline graphs
        type AdaptiveGraphBuilder;

        /// Opaque handle for a data buffer
        type AdaptiveBuffer;

        /// Opaque handle for a vector of buffers
        type AdaptiveBufferVec;

        // ============================================================
        // Executor lifecycle functions
        // ============================================================

        /// Create a new executor (not started)
        fn adaptive_executor_new() -> Box<AdaptiveExecutor>;

        /// Start the executor thread
        fn adaptive_executor_start(executor: &mut AdaptiveExecutor) -> ExecutorResult;

        /// Shutdown the executor and wait for thread to finish
        fn adaptive_executor_shutdown(executor: &mut AdaptiveExecutor) -> ExecutorResult;

        /// Get a handle for submitting work
        fn adaptive_executor_get_handle(executor: &AdaptiveExecutor) -> Box<AdaptiveExecutorHandle>;

        /// Free the executor
        fn adaptive_executor_free(executor: Box<AdaptiveExecutor>);

        // ============================================================
        // Handle functions
        // ============================================================

        /// Clone the executor handle
        fn adaptive_handle_clone(handle: &AdaptiveExecutorHandle) -> Box<AdaptiveExecutorHandle>;

        /// Emit a buffer to a pipeline
        fn adaptive_handle_emit(
            handle: &AdaptiveExecutorHandle,
            pipeline_id: &str,
            buffer: Box<AdaptiveBuffer>,
        ) -> ExecutorResult;

        /// Free the executor handle
        fn adaptive_handle_free(handle: Box<AdaptiveExecutorHandle>);

        // ============================================================
        // Buffer functions
        // ============================================================

        /// Create a buffer from a NES TupleBuffer (copies data)
        unsafe fn adaptive_buffer_from_tuple_buffer(
            builder: Pin<&mut NESTupleBufferBuilder>,
        ) -> Box<AdaptiveBuffer>;

        /// Write buffer data back to a NES TupleBuffer
        unsafe fn adaptive_buffer_to_tuple_buffer(
            buffer: &AdaptiveBuffer,
            builder: Pin<&mut NESTupleBufferBuilder>,
        );

        /// Get buffer size
        fn adaptive_buffer_get_size(buffer: &AdaptiveBuffer) -> usize;

        /// Get buffer sequence number
        fn adaptive_buffer_get_sequence(buffer: &AdaptiveBuffer) -> u64;

        /// Create a buffer from raw data with metadata
        unsafe fn adaptive_buffer_create_from_raw(
            data_ptr: *const u8,
            size: usize,
            sequence_number: u64,
            origin_id: u64,
            watermark: u64,
            number_of_tuples: u64,
            chunk_number: i64,
            last_chunk: bool,
        ) -> Box<AdaptiveBuffer>;

        /// Free a buffer
        fn adaptive_buffer_free(buffer: Box<AdaptiveBuffer>);

        // ============================================================
        // Buffer vector functions
        // ============================================================

        /// Create a new buffer vector
        fn adaptive_buffer_vec_new() -> Box<AdaptiveBufferVec>;

        /// Push a buffer into the vector
        fn adaptive_buffer_vec_push(vec: &mut AdaptiveBufferVec, buffer: Box<AdaptiveBuffer>);

        /// Get vector length
        fn adaptive_buffer_vec_len(vec: &AdaptiveBufferVec) -> usize;

        /// Free the buffer vector
        fn adaptive_buffer_vec_free(vec: Box<AdaptiveBufferVec>);

        // ============================================================
        // Graph builder functions
        // ============================================================

        /// Create a new graph builder
        fn adaptive_graph_builder_new() -> Box<AdaptiveGraphBuilder>;

        /// Add a pipeline to the graph
        fn adaptive_graph_builder_add_pipeline(
            builder: &mut AdaptiveGraphBuilder,
            pipeline_id: &str,
            context: usize,
        ) -> ExecutorResult;

        /// Connect two pipelines
        fn adaptive_graph_builder_connect(
            builder: &mut AdaptiveGraphBuilder,
            source_id: &str,
            sink_id: &str,
        ) -> ExecutorResult;

        /// Deploy the graph to the executor
        fn adaptive_graph_builder_deploy(
            builder: Box<AdaptiveGraphBuilder>,
            handle: &AdaptiveExecutorHandle,
        ) -> ExecutorResult;

        /// Free the graph builder
        fn adaptive_graph_builder_free(builder: Box<AdaptiveGraphBuilder>);

        // ============================================================
        // Execution context callback functions (for C++ to call Rust)
        // ============================================================

        /// Emit a buffer to successor pipelines via Rust execution context.
        ///
        /// # Arguments
        ///
        /// * `context_handle` - Raw pointer to ExecutionContextOpaque (as uintptr_t)
        /// * `data_ptr` - Pointer to buffer data
        /// * `size` - Size of buffer data
        /// * `sequence_number` - Buffer sequence number
        /// * `origin_id` - Buffer origin ID
        /// * `watermark` - Buffer watermark
        /// * `number_of_tuples` - Number of tuples in buffer
        /// * `chunk_number` - Chunk number (-1 if not chunked)
        /// * `last_chunk` - Whether this is the last chunk
        ///
        /// # Returns
        ///
        /// 1 if successful, 0 on failure
        ///
        /// # Safety
        ///
        /// The context_handle must be a valid ExecutionContextOpaque pointer.
        /// The data_ptr must point to valid memory of at least `size` bytes.
        unsafe fn rust_exec_context_emit_buffer(
            context_handle: usize,
            data_ptr: *const u8,
            size: usize,
            sequence_number: u64,
            origin_id: u64,
            watermark: u64,
            number_of_tuples: u64,
            chunk_number: i64,
            last_chunk: bool,
        ) -> i32;

        /// Get the number of worker threads from the Rust execution context.
        ///
        /// # Arguments
        ///
        /// * `context_handle` - Raw pointer to ExecutionContextOpaque (as uintptr_t)
        ///
        /// # Returns
        ///
        /// The number of worker threads
        ///
        /// # Safety
        ///
        /// The context_handle must be a valid ExecutionContextOpaque pointer.
        unsafe fn rust_exec_context_get_worker_count(context_handle: usize) -> u64;

        /// Schedule re-execution of a buffer after a delay via Rust execution context.
        ///
        /// # Arguments
        ///
        /// * `context_handle` - Raw pointer to ExecutionContextOpaque (as uintptr_t)
        /// * `data_ptr` - Pointer to buffer data
        /// * `size` - Size of buffer data
        /// * `sequence_number` - Buffer sequence number
        /// * `origin_id` - Buffer origin ID
        /// * `watermark` - Buffer watermark
        /// * `number_of_tuples` - Number of tuples in buffer
        /// * `chunk_number` - Chunk number (-1 if not chunked)
        /// * `last_chunk` - Whether this is the last chunk
        /// * `delay_ms` - Delay in milliseconds before re-execution
        ///
        /// # Returns
        ///
        /// 1 if successful, 0 on failure
        ///
        /// # Safety
        ///
        /// The context_handle must be a valid ExecutionContextOpaque pointer.
        /// The data_ptr must point to valid memory of at least `size` bytes.
        unsafe fn rust_exec_context_repeat_task(
            context_handle: usize,
            data_ptr: *const u8,
            size: usize,
            sequence_number: u64,
            origin_id: u64,
            watermark: u64,
            number_of_tuples: u64,
            chunk_number: i64,
            last_chunk: bool,
            delay_ms: u64,
        ) -> i32;
    }
}

// ============================================================
// Wrapper types
// ============================================================

/// Wrapper around ExecutorOpaqueHandle
pub struct AdaptiveExecutor {
    inner: Box<ExecutorOpaqueHandle>,
}

/// Wrapper around ExecutorHandleOpaque
pub struct AdaptiveExecutorHandle {
    inner: Box<ExecutorHandleOpaque>,
}

/// Wrapper around GraphBuilderOpaque
pub struct AdaptiveGraphBuilder {
    inner: Box<GraphBuilderOpaque>,
}

/// Wrapper around BufferHandleOpaque
pub struct AdaptiveBuffer {
    inner: Box<BufferHandleOpaque>,
}

/// Wrapper around BufferVecHandle
pub struct AdaptiveBufferVec {
    inner: Box<BufferVecHandle>,
}

// ============================================================
// Executor implementation
// ============================================================

fn adaptive_executor_new() -> Box<AdaptiveExecutor> {
    let inner = adaptive_engine::ffi::executor::executor_new();
    Box::new(AdaptiveExecutor { inner })
}

fn adaptive_executor_start(executor: &mut AdaptiveExecutor) -> ffi::ExecutorResult {
    let result = adaptive_engine::ffi::executor::executor_start(&mut executor.inner);
    error_code_to_result(result)
}

fn adaptive_executor_shutdown(executor: &mut AdaptiveExecutor) -> ffi::ExecutorResult {
    let result = adaptive_engine::ffi::executor::executor_shutdown(&mut executor.inner);
    error_code_to_result(result)
}

fn adaptive_executor_get_handle(executor: &AdaptiveExecutor) -> Box<AdaptiveExecutorHandle> {
    let inner = adaptive_engine::ffi::executor::executor_get_handle(&executor.inner);
    Box::new(AdaptiveExecutorHandle { inner })
}

fn adaptive_executor_free(_executor: Box<AdaptiveExecutor>) {
    // Dropped automatically
}

// ============================================================
// Handle implementation
// ============================================================

fn adaptive_handle_clone(handle: &AdaptiveExecutorHandle) -> Box<AdaptiveExecutorHandle> {
    Box::new(AdaptiveExecutorHandle {
        inner: Box::new(ExecutorHandleOpaque::new(handle.inner.handle.clone())),
    })
}

fn adaptive_handle_emit(
    handle: &AdaptiveExecutorHandle,
    pipeline_id: &str,
    buffer: Box<AdaptiveBuffer>,
) -> ffi::ExecutorResult {
    let result =
        adaptive_engine::ffi::executor::executor_handle_emit(&handle.inner, pipeline_id, buffer.inner);
    error_code_to_result(result)
}

fn adaptive_handle_free(_handle: Box<AdaptiveExecutorHandle>) {
    // Dropped automatically
}

// ============================================================
// Buffer implementation
// ============================================================

/// Create an AdaptiveBuffer from a NES TupleBuffer
///
/// # Safety
///
/// The builder must point to a valid NESTupleBufferBuilder with valid data.
unsafe fn adaptive_buffer_from_tuple_buffer(
    mut builder: Pin<&mut ffi::NESTupleBufferBuilder>,
) -> Box<AdaptiveBuffer> {
    let meta = builder.getMetadata();
    let size = builder.getSize();
    let data_ptr = builder.as_mut().getDataPtr();

    let inner = adaptive_engine::ffi::buffer::buffer_create_from_tuple_buffer(
        data_ptr,
        size,
        meta.sequence_number,
        meta.origin_id,
        meta.watermark,
        meta.number_of_tuples,
        meta.chunk_number,
        meta.last_chunk,
    );

    Box::new(AdaptiveBuffer { inner })
}

/// Write buffer data back to a NES TupleBuffer
///
/// # Safety
///
/// The builder must point to a valid NESTupleBufferBuilder with sufficient capacity.
unsafe fn adaptive_buffer_to_tuple_buffer(
    buffer: &AdaptiveBuffer,
    mut builder: Pin<&mut ffi::NESTupleBufferBuilder>,
) {
    // Get data from buffer
    let size = adaptive_engine::ffi::buffer::buffer_get_size(&buffer.inner);
    let data_ptr = adaptive_engine::ffi::buffer::buffer_get_data(&buffer.inner);

    // Create slice from raw parts
    let data = if size > 0 {
        std::slice::from_raw_parts(data_ptr, size)
    } else {
        &[]
    };

    // Set metadata
    let meta = ffi::TupleBufferMetadata {
        sequence_number: adaptive_engine::ffi::buffer::buffer_get_sequence(&buffer.inner),
        origin_id: adaptive_engine::ffi::buffer::buffer_get_origin_id(&buffer.inner),
        watermark: adaptive_engine::ffi::buffer::buffer_get_watermark(&buffer.inner),
        number_of_tuples: adaptive_engine::ffi::buffer::buffer_get_tuple_count(&buffer.inner),
        chunk_number: adaptive_engine::ffi::buffer::buffer_get_chunk_number(&buffer.inner),
        last_chunk: adaptive_engine::ffi::buffer::buffer_is_last_chunk(&buffer.inner),
    };

    builder.as_mut().setMetadata(&meta);
    builder.as_mut().setData(data);
}

fn adaptive_buffer_get_size(buffer: &AdaptiveBuffer) -> usize {
    adaptive_engine::ffi::buffer::buffer_get_size(&buffer.inner)
}

fn adaptive_buffer_get_sequence(buffer: &AdaptiveBuffer) -> u64 {
    adaptive_engine::ffi::buffer::buffer_get_sequence(&buffer.inner)
}

/// Create an AdaptiveBuffer from raw data with metadata
///
/// # Safety
///
/// The data_ptr must point to valid memory of at least `size` bytes.
unsafe fn adaptive_buffer_create_from_raw(
    data_ptr: *const u8,
    size: usize,
    sequence_number: u64,
    origin_id: u64,
    watermark: u64,
    number_of_tuples: u64,
    chunk_number: i64,
    last_chunk: bool,
) -> Box<AdaptiveBuffer> {
    let inner = adaptive_engine::ffi::buffer::buffer_create_from_tuple_buffer(
        data_ptr,
        size,
        sequence_number,
        origin_id,
        watermark,
        number_of_tuples,
        chunk_number,
        last_chunk,
    );

    Box::new(AdaptiveBuffer { inner })
}

fn adaptive_buffer_free(_buffer: Box<AdaptiveBuffer>) {
    // Dropped automatically
}

// ============================================================
// Buffer vector implementation
// ============================================================

fn adaptive_buffer_vec_new() -> Box<AdaptiveBufferVec> {
    let inner = adaptive_engine::ffi::buffer::buffer_vec_new();
    Box::new(AdaptiveBufferVec { inner })
}

fn adaptive_buffer_vec_push(vec: &mut AdaptiveBufferVec, buffer: Box<AdaptiveBuffer>) {
    adaptive_engine::ffi::buffer::buffer_vec_push(&mut vec.inner, buffer.inner);
}

fn adaptive_buffer_vec_len(vec: &AdaptiveBufferVec) -> usize {
    adaptive_engine::ffi::buffer::buffer_vec_len(&vec.inner)
}

fn adaptive_buffer_vec_free(_vec: Box<AdaptiveBufferVec>) {
    // Dropped automatically
}

// ============================================================
// Graph builder implementation
// ============================================================

fn adaptive_graph_builder_new() -> Box<AdaptiveGraphBuilder> {
    let inner = adaptive_engine::ffi::graph::graph_builder_new();
    Box::new(AdaptiveGraphBuilder { inner })
}

fn adaptive_graph_builder_add_pipeline(
    builder: &mut AdaptiveGraphBuilder,
    pipeline_id: &str,
    context: usize,
) -> ffi::ExecutorResult {
    let result =
        adaptive_engine::ffi::graph::graph_builder_add_pipeline(&mut builder.inner, pipeline_id, context);
    error_code_to_result(result)
}

fn adaptive_graph_builder_connect(
    builder: &mut AdaptiveGraphBuilder,
    source_id: &str,
    sink_id: &str,
) -> ffi::ExecutorResult {
    let result =
        adaptive_engine::ffi::graph::graph_builder_connect(&mut builder.inner, source_id, sink_id);
    error_code_to_result(result)
}

fn adaptive_graph_builder_deploy(
    builder: Box<AdaptiveGraphBuilder>,
    handle: &AdaptiveExecutorHandle,
) -> ffi::ExecutorResult {
    let result =
        adaptive_engine::ffi::graph::graph_builder_deploy(builder.inner, &handle.inner);
    error_code_to_result(result)
}

fn adaptive_graph_builder_free(_builder: Box<AdaptiveGraphBuilder>) {
    // Dropped automatically
}

// ============================================================
// Helper functions
// ============================================================

fn error_code_to_result(code: i32) -> ffi::ExecutorResult {
    match code {
        0 => ffi::ExecutorResult::Ok,
        1 => ffi::ExecutorResult::AlreadyStarted,
        2 => ffi::ExecutorResult::NotStarted,
        4 => ffi::ExecutorResult::InvalidHandle,
        10 => ffi::ExecutorResult::BufferError,
        5 => ffi::ExecutorResult::GraphError,
        _ => ffi::ExecutorResult::Unknown,
    }
}

// ============================================================
// Execution context callback implementation
// ============================================================

/// Emit a buffer to successor pipelines via Rust execution context.
///
/// This function is called from C++ RustBridgeExecutionContext::emitBuffer
/// to route buffer emissions back through the Rust adaptive engine.
///
/// # Safety
///
/// - `context_handle` must be a valid pointer to an ExecutionContextOpaque
/// - `data_ptr` must point to valid memory of at least `size` bytes
unsafe fn rust_exec_context_emit_buffer(
    context_handle: usize,
    data_ptr: *const u8,
    size: usize,
    sequence_number: u64,
    origin_id: u64,
    watermark: u64,
    number_of_tuples: u64,
    chunk_number: i64,
    last_chunk: bool,
) -> i32 {
    // Convert the context handle back to an ExecutionContextOpaque reference
    let context_ptr = context_handle as *const ExecutionContextOpaque;
    if context_ptr.is_null() {
        return 0; // Failure - null context
    }
    let context = &*context_ptr;

    // Create a BufferHandleOpaque from the raw data
    let buffer_handle = adaptive_engine::ffi::buffer::buffer_create_from_tuple_buffer(
        data_ptr,
        size,
        sequence_number,
        origin_id,
        watermark,
        number_of_tuples,
        chunk_number,
        last_chunk,
    );

    // Call the Rust exec_context_emit_buffer function
    adaptive_engine::ffi::execution_context::exec_context_emit_buffer(context, buffer_handle)
}

/// Get the number of worker threads from the Rust execution context.
///
/// This function is called from C++ RustBridgeExecutionContext::getNumberOfWorkerThreads
/// to retrieve the worker count from the Rust adaptive engine.
///
/// # Safety
///
/// - `context_handle` must be a valid pointer to an ExecutionContextOpaque
unsafe fn rust_exec_context_get_worker_count(context_handle: usize) -> u64 {
    // Convert the context handle back to an ExecutionContextOpaque reference
    let context_ptr = context_handle as *const ExecutionContextOpaque;
    if context_ptr.is_null() {
        return 1; // Default to 1 worker on null context
    }
    let context = &*context_ptr;

    // Call the Rust exec_context_get_worker_count function
    adaptive_engine::ffi::execution_context::exec_context_get_worker_count(context) as u64
}

/// Schedule re-execution of a buffer after a delay via Rust execution context.
///
/// This function is called from C++ RustBridgeExecutionContext::repeatTask
/// to schedule re-execution of the current pipeline with a buffer after a delay.
///
/// # Safety
///
/// - `context_handle` must be a valid pointer to an ExecutionContextOpaque
/// - `data_ptr` must point to valid memory of at least `size` bytes
#[allow(clippy::too_many_arguments)]
unsafe fn rust_exec_context_repeat_task(
    context_handle: usize,
    data_ptr: *const u8,
    size: usize,
    sequence_number: u64,
    origin_id: u64,
    watermark: u64,
    number_of_tuples: u64,
    chunk_number: i64,
    last_chunk: bool,
    delay_ms: u64,
) -> i32 {
    // Convert the context handle back to an ExecutionContextOpaque reference
    let context_ptr = context_handle as *const ExecutionContextOpaque;
    if context_ptr.is_null() {
        return 0; // Failure - null context
    }
    let context = &*context_ptr;

    // Create a BufferHandleOpaque from the raw data
    let buffer_handle = adaptive_engine::ffi::buffer::buffer_create_from_tuple_buffer(
        data_ptr,
        size,
        sequence_number,
        origin_id,
        watermark,
        number_of_tuples,
        chunk_number,
        last_chunk,
    );

    // Call the Rust exec_context_repeat_task function
    adaptive_engine::ffi::execution_context::exec_context_repeat_task(context, buffer_handle, delay_ms)
}
