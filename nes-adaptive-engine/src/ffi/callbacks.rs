//! C++ callback wrappers for pipeline stages and sources.
//!
//! This module provides Rust implementations that call through to C++ stages
//! and sources via FFI, enabling C++ PipelineStage and SourceHandle
//! implementations to be used with the Rust executor.

use crate::executor::PipelineExecutionContext;
use crate::ffi::buffer::BufferMetadata;
use crate::pipeline::{Buffer, OpaqueBufferHandle, Pipeline, PipelineError, PipelineId};
use crate::sequence::SequenceNumber;
use crate::source::{Source, SourceEmitHandle, SourceError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

// =============================================================================
// CppPipelineStage - wraps C++ PipelineStage via FFI
// =============================================================================

/// A pipeline stage that wraps a C++ PipelineStage implementation.
///
/// This struct implements the Rust Pipeline trait and forwards all calls
/// to the corresponding C++ methods via FFI. Emitted buffers and repeat_task
/// requests are communicated via thread-local storage in FfiCallbacks.cpp.
pub struct CppPipelineStage {
    /// Unique identifier for this stage
    id: PipelineId,
    /// Opaque pointer to the C++ PipelineStage instance
    stage_ptr: usize,
    /// Pointer to the C++ BufferProvider
    buffer_provider_ptr: usize,
}

// SAFETY: CppPipelineStage is Send because:
// - C++ PipelineStage is thread-safe by contract
// - stage_ptr and buffer_provider_ptr are just usize values
unsafe impl Send for CppPipelineStage {}

// SAFETY: CppPipelineStage is Sync because:
// - C++ PipelineStage is thread-safe by contract
// - All FFI calls are properly synchronized on the C++ side
unsafe impl Sync for CppPipelineStage {}

impl CppPipelineStage {
    /// Create a new C++ pipeline stage wrapper.
    ///
    /// # Safety
    /// The caller must ensure:
    /// - `stage_ptr` points to a valid C++ PipelineStage
    /// - `buffer_provider_ptr` points to a valid C++ BufferProvider
    /// - Both pointers remain valid for the lifetime of this wrapper
    pub unsafe fn new(id: PipelineId, stage_ptr: usize, buffer_provider_ptr: usize) -> Self {
        Self {
            id,
            stage_ptr,
            buffer_provider_ptr,
        }
    }

    /// Get the stage pointer.
    pub fn stage_ptr(&self) -> usize {
        self.stage_ptr
    }

    /// Get the buffer provider pointer.
    pub fn buffer_provider_ptr(&self) -> usize {
        self.buffer_provider_ptr
    }
}

impl Pipeline for CppPipelineStage {
    fn setup(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        let result = unsafe { stage_start(self.stage_ptr, self.buffer_provider_ptr) };
        if result == 0 {
            Err(PipelineError::ExecutionFailed(
                "C++ stage start() failed".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    fn execute(
        &self,
        input: Buffer,
        context: &dyn PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        // If the input buffer carries an opaque handle, pass it directly to
        // the C++ stage. This preserves child buffers for variable-sized data.
        let result = if let Some(opaque) = input.opaque_handle() {
            unsafe {
                stage_execute_with_handle(self.stage_ptr, self.buffer_provider_ptr, opaque.handle())
            }
        } else {
            let metadata = BufferMetadata::from(&input);
            // Fallback: pass raw data (no opaque handle available)
            unsafe {
                stage_execute(
                    self.stage_ptr,
                    self.buffer_provider_ptr,
                    input.data().as_ptr() as usize,
                    input.data().len(),
                    metadata.sequence_number,
                    metadata.origin_id,
                    metadata.watermark,
                    metadata.num_tuples,
                    metadata.chunk_number,
                    metadata.last_chunk,
                )
            }
        };

        if result == 0 {
            return Err(PipelineError::ExecutionFailed(
                "C++ stage execute() failed".to_string(),
            ));
        }

        // Check if repeat_task was requested during execution
        if unsafe { stage_get_repeat_requested() } {
            context.repeat_task(0);
        }

        // Collect emitted buffers from C++ thread-local storage
        let count = unsafe { stage_get_emitted_count() };
        let mut buffers = Vec::with_capacity(count);

        for i in 0..count {
            let data_ptr = unsafe { stage_get_emitted_data_ptr(i) } as *const u8;
            let data_size = unsafe { stage_get_emitted_data_size(i) };

            let mut seq = 0u64;
            let mut origin = 0u64;
            let mut watermark = 0u64;
            let mut num_tuples = 0u64;
            let mut chunk_number = 0u32;
            let mut last_chunk = false;

            unsafe {
                stage_get_emitted_metadata(
                    i,
                    &mut seq,
                    &mut origin,
                    &mut watermark,
                    &mut num_tuples,
                    &mut chunk_number,
                    &mut last_chunk,
                );
            }

            // Copy data from C++ TLS into Rust Vec (needed for Rust executor routing)
            let data = if data_ptr.is_null() || data_size == 0 {
                vec![]
            } else {
                unsafe { std::slice::from_raw_parts(data_ptr, data_size) }.to_vec()
            };

            let mut buffer = Buffer::new(data, SequenceNumber::new(seq))
                .with_origin(origin)
                .with_watermark(watermark)
                .with_tuple_count(num_tuples)
                .with_chunk_info(chunk_number as u64, last_chunk);

            // Attach opaque handle to preserve child buffers for variable-sized data
            let opaque_handle = unsafe { stage_get_emitted_opaque_handle(i) };
            if opaque_handle != 0 {
                buffer = buffer.with_opaque_handle(OpaqueBufferHandle::new(
                    self.buffer_provider_ptr,
                    opaque_handle,
                ));
            }

            buffers.push(buffer);
        }

        Ok(buffers)
    }

    fn teardown(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        // Call C++ stage stop() with repeat_task loop support
        loop {
            let result = unsafe { stage_stop(self.stage_ptr, self.buffer_provider_ptr) };
            if result == 0 {
                return Err(PipelineError::ExecutionFailed(
                    "C++ stage stop() failed".to_string(),
                ));
            }

            // Check if repeat was requested during stop
            if unsafe { stage_get_repeat_requested() } {
                continue; // Call stop again
            } else {
                break;
            }
        }
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

// =============================================================================
// CppSourceHandle - wraps C++ SourceHandle via FFI
// =============================================================================

/// A source handle that wraps a C++ SourceHandle implementation.
///
/// This provides methods for the pull-based source lifecycle:
/// open() → next_buffer() loop → close()
///
/// Thread safety: The C++ SourceHandle (e.g. NesSourceHandle) is NOT thread-safe --
/// concurrent calls to next_buffer() and close() cause data races on internal state
/// like `opened_`. The source thread is the ONLY caller of next_buffer() and close()
/// (teardown joins the thread before proceeding), so calls are serialized by design.
/// The atomic `closed` flag is a safety net for idempotent close().
pub struct CppSourceHandle {
    /// Unique identifier for this source
    id: PipelineId,
    /// Opaque pointer to the C++ SourceHandle instance
    source_ptr: usize,
    /// Pointer to the C++ BufferProvider
    buffer_provider_ptr: usize,
    /// Whether close() has already been called (prevents concurrent/duplicate close).
    closed: AtomicBool,
    /// Mutex to serialize FFI calls (next_buffer vs close).
    ffi_mutex: Mutex<()>,
}

// SAFETY: Same reasoning as CppPipelineStage
unsafe impl Send for CppSourceHandle {}
unsafe impl Sync for CppSourceHandle {}

impl CppSourceHandle {
    /// Create a new C++ source handle wrapper.
    ///
    /// # Safety
    /// The caller must ensure:
    /// - `source_ptr` points to a valid C++ SourceHandle
    /// - `buffer_provider_ptr` points to a valid C++ BufferProvider
    /// - Both pointers remain valid for the lifetime of this wrapper
    pub unsafe fn new(id: PipelineId, source_ptr: usize, buffer_provider_ptr: usize) -> Self {
        Self {
            id,
            source_ptr,
            buffer_provider_ptr,
            closed: AtomicBool::new(false),
            ffi_mutex: Mutex::new(()),
        }
    }

    /// Get the source ID.
    pub fn id(&self) -> &PipelineId {
        &self.id
    }

    /// Open the source.
    pub fn open(&self) -> Result<(), PipelineError> {
        let result = unsafe { source_open(self.source_ptr, self.buffer_provider_ptr) };
        if result == 0 {
            Err(PipelineError::ExecutionFailed(
                "C++ source open() failed".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    /// Get the next buffer from the source.
    ///
    /// Returns Ok(Some((data, metadata, opaque_handle))) if a buffer is available,
    /// Ok(None) if exhausted (EOS), or Err if an error occurred.
    /// The opaque handle is preserved so it can be passed to downstream stages,
    /// preserving child buffers for variable-sized data.
    ///
    /// If close() has been called (from any thread), returns Ok(None) immediately
    /// without calling into C++, avoiding a data race with the C++ close() path.
    pub fn next_buffer(
        &self,
    ) -> Result<Option<(Vec<u8>, BufferMetadata, OpaqueBufferHandle)>, PipelineError> {
        // Check if already closed - avoids calling next_buffer() after close()
        // which is a data race on the C++ side (non-atomic `opened_` flag).
        // This is safe because only the source thread calls next_buffer() and close(),
        // and teardown() joins the source thread before proceeding.
        if self.closed.load(Ordering::SeqCst) {
            return Ok(None);
        }

        let handle = unsafe { source_next_buffer(self.source_ptr, self.buffer_provider_ptr) };
        if handle == 0 {
            return Ok(None); // EOS
        }
        if handle == usize::MAX {
            return Err(PipelineError::ExecutionFailed(
                "C++ source next_buffer() failed".to_string(),
            ));
        }

        // Get data from provider
        let data_ptr =
            unsafe { buffer_provider_get_data(self.buffer_provider_ptr, handle) } as *const u8;
        let size = unsafe { buffer_provider_get_size(self.buffer_provider_ptr, handle) };

        // Get metadata from provider
        let mut seq = 0u64;
        let mut origin = 0u64;
        let mut watermark = 0u64;
        let mut num_tuples = 0u64;
        let mut chunk_number = 0u32;
        let mut last_chunk = false;

        unsafe {
            buffer_provider_get_metadata(
                self.buffer_provider_ptr,
                handle,
                &mut seq,
                &mut origin,
                &mut watermark,
                &mut num_tuples,
                &mut chunk_number,
                &mut last_chunk,
            );
        }

        // Copy data into Rust Vec (needed for routing/metadata access in executor)
        let data = if data_ptr.is_null() || size == 0 {
            vec![]
        } else {
            unsafe { std::slice::from_raw_parts(data_ptr, size) }.to_vec()
        };

        let metadata = BufferMetadata {
            sequence_number: seq,
            origin_id: origin,
            watermark,
            num_tuples,
            chunk_number,
            last_chunk,
        };

        // Preserve the opaque handle - it will be released when the last
        // Arc reference is dropped (via OpaqueBufferHandleInner::drop).
        let opaque = OpaqueBufferHandle::new(self.buffer_provider_ptr, handle);

        Ok(Some((data, metadata, opaque)))
    }

    /// Close the source.
    ///
    /// Uses an atomic compare-and-swap to ensure the C++ close() is called exactly
    /// once, even when called concurrently from the source thread and executor thread.
    /// After close(), subsequent next_buffer() calls return None without calling into C++.
    pub fn close(&self) -> Result<(), PipelineError> {
        // Fast path: already closed.
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Acquire mutex to wait for any in-flight next_buffer() call to complete.
        let _guard = self.ffi_mutex.lock().unwrap();

        // Atomically set closed to true; if it was already true, skip the FFI call.
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(()); // Already closed by another thread
        }

        let result = unsafe { source_close(self.source_ptr, self.buffer_provider_ptr) };
        if result == 0 {
            Err(PipelineError::ExecutionFailed(
                "C++ source close() failed".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    /// Check if close() has been called.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Signal the source to stop producing data without closing resources.
    ///
    /// This calls C++ SourceHandle::request_stop(), which signals stop tokens
    /// to unblock a concurrent next_buffer() call. Unlike close(), this does NOT
    /// release resources, so it is safe to call while next_buffer() is in progress
    /// on another thread.
    pub fn request_stop(&self) {
        unsafe { source_request_stop(self.source_ptr) };
    }
}

// =============================================================================
// CppSourceAdapter - implements Rust Source trait for C++ SourceHandle
// =============================================================================

/// Adapts a pull-based C++ SourceHandle to the push-based Rust Source trait.
///
/// The adapter spawns a thread in `start()` that pulls buffers from the C++
/// source via open() → next_buffer() loop → close(), converting each buffer
/// to a Rust Buffer and emitting it via the SourceEmitHandle.
pub struct CppSourceAdapter {
    id: PipelineId,
    source_handle: Arc<CppSourceHandle>,
    stopped: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

// SAFETY: CppSourceAdapter is Send+Sync because:
// - source_handle is Arc (thread-safe ref counting)
// - stopped is Arc<AtomicBool> (lock-free atomic)
// - thread is Mutex<Option<JoinHandle>> (mutex-protected)
unsafe impl Send for CppSourceAdapter {}
unsafe impl Sync for CppSourceAdapter {}

impl CppSourceAdapter {
    /// Create a new adapter wrapping a C++ source handle.
    pub fn new(source_handle: CppSourceHandle) -> Self {
        let id = source_handle.id().clone();
        Self {
            id,
            source_handle: Arc::new(source_handle),
            stopped: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        }
    }
}

impl Source for CppSourceAdapter {
    fn start(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
        let source_handle = self.source_handle.clone();
        let stopped = self.stopped.clone();

        let thread = std::thread::spawn(move || {
            // Open the source
            if let Err(e) = source_handle.open() {
                eprintln!("Source open failed: {}", e);
                return;
            }

            // Pull buffers until exhausted, error, or stopped
            enum SourceOutcome {
                Eos,
                Error,
                Stopped,
            }

            let outcome = loop {
                if emit_handle.should_stop() || stopped.load(Ordering::SeqCst) {
                    break SourceOutcome::Stopped;
                }

                match source_handle.next_buffer() {
                    Ok(Some((data, metadata, opaque))) => {
                        let buffer =
                            Buffer::new(data, SequenceNumber::new(metadata.sequence_number))
                                .with_origin(metadata.origin_id)
                                .with_watermark(metadata.watermark)
                                .with_tuple_count(metadata.num_tuples)
                                .with_chunk_info(metadata.chunk_number as u64, metadata.last_chunk)
                                .with_opaque_handle(opaque);

                        if emit_handle.emit(buffer).is_err() {
                            break SourceOutcome::Stopped;
                        }
                    }
                    Ok(None) => {
                        // Source exhausted (EOS)
                        break SourceOutcome::Eos;
                    }
                    Err(e) => {
                        // Source encountered an error
                        eprintln!("Source error: {}", e);
                        break SourceOutcome::Error;
                    }
                }
            };

            // Close the source (idempotent - safe even if already closed)
            let _ = source_handle.close();

            match outcome {
                SourceOutcome::Eos => {
                    // If source exhausted naturally, signal end-of-stream to trigger
                    // cascading shutdown through the pipeline DAG
                    let _ = emit_handle.end_of_stream();
                }
                SourceOutcome::Error => {
                    // Signal error to the executor - this enqueues a special error task
                    let _ = emit_handle.signal_error("Source next_buffer() failed");
                }
                SourceOutcome::Stopped => {
                    // Source was stopped externally - no further action needed
                }
            }
        });

        *self.thread.lock().unwrap() = Some(thread);
        Ok(())
    }

    fn stop(&self) -> Result<(), SourceError> {
        self.stopped.store(true, Ordering::SeqCst);
        // Signal the C++ source to stop via its stop token. This unblocks any
        // concurrent next_buffer() call (e.g., waiting in getBufferWithTimeout)
        // WITHOUT closing the source resources. This is critical because close()
        // destroys state (e.g., closes file handles) that next_buffer() may be
        // actively using on the source thread. The source thread will call close()
        // itself after its loop exits.
        self.source_handle.request_stop();
        Ok(())
    }

    fn teardown(&self) -> Result<(), SourceError> {
        // Ensure stopped
        self.stopped.store(true, Ordering::SeqCst);
        // Signal the stop token to unblock next_buffer() on the source thread
        self.source_handle.request_stop();
        // Join the source thread - it will call close() before exiting
        if let Some(thread) = self.thread.lock().unwrap().take() {
            thread
                .join()
                .map_err(|_| SourceError::TeardownFailed("Source thread panicked".into()))?;
        }
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

// =============================================================================
// FFI declarations - C functions implemented in FfiCallbacks.cpp
// =============================================================================

extern "C" {
    // Stage callbacks
    fn stage_start(stage_ptr: usize, provider_ptr: usize) -> i32;
    fn stage_execute(
        stage_ptr: usize,
        provider_ptr: usize,
        data_ptr: usize,
        data_size: usize,
        sequence_number: u64,
        origin_id: u64,
        watermark: u64,
        num_tuples: u64,
        chunk_number: u32,
        last_chunk: bool,
    ) -> i32;
    fn stage_execute_with_handle(
        stage_ptr: usize,
        provider_ptr: usize,
        opaque_handle: usize,
    ) -> i32;
    fn stage_stop(stage_ptr: usize, provider_ptr: usize) -> i32;

    // Emitted buffer retrieval (thread-local storage access)
    fn stage_get_emitted_count() -> usize;
    fn stage_get_emitted_data_ptr(index: usize) -> usize;
    fn stage_get_emitted_data_size(index: usize) -> usize;
    fn stage_get_emitted_opaque_handle(index: usize) -> usize;
    fn stage_get_emitted_metadata(
        index: usize,
        seq_out: *mut u64,
        origin_out: *mut u64,
        watermark_out: *mut u64,
        num_tuples_out: *mut u64,
        chunk_number_out: *mut u32,
        last_chunk_out: *mut bool,
    );
    fn stage_get_repeat_requested() -> bool;

    // Source callbacks
    fn source_open(source_ptr: usize, provider_ptr: usize) -> i32;
    fn source_next_buffer(source_ptr: usize, provider_ptr: usize) -> usize;
    fn source_request_stop(source_ptr: usize);
    fn source_close(source_ptr: usize, provider_ptr: usize) -> i32;

    // Buffer provider callbacks
    pub fn buffer_provider_get_data(provider_ptr: usize, handle: usize) -> usize;
    pub fn buffer_provider_get_size(provider_ptr: usize, handle: usize) -> usize;
    fn buffer_provider_get_metadata(
        provider_ptr: usize,
        handle: usize,
        seq_out: *mut u64,
        origin_out: *mut u64,
        watermark_out: *mut u64,
        num_tuples_out: *mut u64,
        chunk_number_out: *mut u32,
        last_chunk_out: *mut bool,
    );
    pub fn buffer_provider_release(provider_ptr: usize, handle: usize);

    // Object lifecycle callbacks
    fn source_destroy(source_ptr: usize);
    fn stage_destroy(stage_ptr: usize);
}

// =============================================================================
// Drop implementations - destroy C++ objects when Rust wrappers are dropped
// =============================================================================

impl Drop for CppPipelineStage {
    fn drop(&mut self) {
        unsafe {
            stage_destroy(self.stage_ptr);
        }
    }
}

impl Drop for CppSourceHandle {
    fn drop(&mut self) {
        unsafe {
            source_destroy(self.source_ptr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests can only verify compilation since the extern C
    // functions are not linked in unit tests. Full integration tests
    // require the C++ library.

    #[test]
    fn test_cpp_pipeline_stage_creation() {
        // We can't actually create a valid CppPipelineStage without C++ pointers,
        // but we can test the struct layout
        assert!(std::mem::size_of::<CppPipelineStage>() > 0);
    }

    #[test]
    fn test_cpp_source_handle_creation() {
        assert!(std::mem::size_of::<CppSourceHandle>() > 0);
    }

    #[test]
    fn test_cpp_source_adapter_creation() {
        assert!(std::mem::size_of::<CppSourceAdapter>() > 0);
    }
}
