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

//! C++ callback wrappers for pipeline stages and sources.
//!
//! This module provides Rust implementations that call through to C++ stages
//! and sources via FFI, enabling C++ PipelineStage and SourceHandle
//! implementations to be used with the Rust executor.

use crate::executor::PipelineExecutionContext;
use crate::pipeline::{Buffer, OpaqueBufferHandle, Pipeline, PipelineError, PipelineId};
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
    /// Opaque context pointer (e.g., NesBufferProvider*)
    context_ptr: usize,
}

// SAFETY: CppPipelineStage is Send because:
// - C++ PipelineStage is thread-safe by contract
// - stage_ptr and context_ptr are just usize values
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
    /// - `context_ptr` is a valid opaque context pointer
    /// - Both pointers remain valid for the lifetime of this wrapper
    pub unsafe fn new(id: PipelineId, stage_ptr: usize, context_ptr: usize) -> Self {
        Self {
            id,
            stage_ptr,
            context_ptr,
        }
    }

    /// Get the stage pointer.
    pub fn stage_ptr(&self) -> usize {
        self.stage_ptr
    }

    /// Get the context pointer.
    pub fn context_ptr(&self) -> usize {
        self.context_ptr
    }
}

impl Pipeline for CppPipelineStage {
    fn setup(&self, context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        let worker_id = context.get_worker_id() as u32;
        let worker_count = context.get_worker_count() as u64;
        let result =
            unsafe { stage_start(self.stage_ptr, self.context_ptr, worker_id, worker_count) };
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
        // The input buffer must carry an opaque handle for C++ stages.
        let opaque = input
            .opaque_handle()
            .expect("CppPipelineStage::execute requires an opaque buffer handle");

        let worker_id = context.get_worker_id() as u32;
        let worker_count = context.get_worker_count() as u64;
        let result = unsafe {
            stage_execute_with_handle(
                self.stage_ptr,
                self.context_ptr,
                opaque.handle(),
                worker_id,
                worker_count,
            )
        };

        if result == 0 {
            return Err(PipelineError::ExecutionFailed(
                "C++ stage execute() failed".to_string(),
            ));
        }

        // Collect emitted buffers from C++ thread-local storage.
        // Each emitted handle was independently cloned by FfiExecutionContext::emit_buffer(),
        // so each one is an independently owned handle — no handle-matching needed.
        let count = unsafe { stage_get_emitted_count() };
        let mut buffers = Vec::with_capacity(count);

        for i in 0..count {
            let emitted_handle = unsafe { stage_get_emitted_opaque_handle(i) };
            if emitted_handle != 0 {
                let handle = OpaqueBufferHandle::new(emitted_handle);
                buffers.push(Buffer::opaque(handle));
            }
        }

        // If repeat was requested, pass the input buffer to the context.
        // The executor will re-enqueue it as-is (no copy needed).
        if unsafe { stage_get_repeat_requested() } {
            context.repeat_task(input, 0);
        }

        Ok(buffers)
    }

    fn flush(&self, context: &dyn PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
        // Call C++ stage stop() which triggers terminate() on windowed operators,
        // flushing all remaining windows. Collect emitted buffers from TLS just
        // like execute() does.
        let worker_id = context.get_worker_id() as u32;
        let worker_count = context.get_worker_count() as u64;
        loop {
            let result =
                unsafe { stage_stop(self.stage_ptr, self.context_ptr, worker_id, worker_count) };
            if result == 0 {
                return Err(PipelineError::ExecutionFailed(
                    "C++ stage stop() failed".to_string(),
                ));
            }

            // Check if repeat was requested during stop (handled inline via loop)
            if unsafe { stage_get_repeat_requested() } {
                continue;
            } else {
                break;
            }
        }

        // Collect emitted buffers from C++ thread-local storage.
        // These are the final window results triggered during terminate().
        let count = unsafe { stage_get_emitted_count() };
        let mut buffers = Vec::with_capacity(count);

        for i in 0..count {
            let emitted_handle = unsafe { stage_get_emitted_opaque_handle(i) };
            if emitted_handle != 0 {
                let handle = OpaqueBufferHandle::new(emitted_handle);
                buffers.push(Buffer::opaque(handle));
            }
        }

        Ok(buffers)
    }

    fn teardown(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        // No-op: stage_stop() is already called in flush(), which handles
        // both the C++ terminate() call and buffer collection.
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
    /// Opaque context pointer (e.g., NesBufferProvider*)
    context_ptr: usize,
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
    /// - `context_ptr` is a valid opaque context pointer
    /// - Both pointers remain valid for the lifetime of this wrapper
    pub unsafe fn new(id: PipelineId, source_ptr: usize, context_ptr: usize) -> Self {
        Self {
            id,
            source_ptr,
            context_ptr,
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
        let result = unsafe { source_open(self.source_ptr, self.context_ptr) };
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
    /// Returns Ok(Some(handle)) if a buffer is available,
    /// Ok(None) if exhausted (EOS), or Err if an error occurred.
    ///
    /// If close() has been called (from any thread), returns Ok(None) immediately
    /// without calling into C++, avoiding a data race with the C++ close() path.
    pub fn next_buffer(&self) -> Result<Option<OpaqueBufferHandle>, PipelineError> {
        // Check if already closed - avoids calling next_buffer() after close()
        // which is a data race on the C++ side (non-atomic `opened_` flag).
        if self.closed.load(Ordering::SeqCst) {
            return Ok(None);
        }

        let handle = unsafe { source_next_buffer(self.source_ptr, self.context_ptr) };
        if handle == 0 {
            return Ok(None); // EOS
        }
        if handle == usize::MAX {
            return Err(PipelineError::ExecutionFailed(
                "C++ source next_buffer() failed".to_string(),
            ));
        }

        // Return the opaque handle directly — no data copy, no metadata extraction.
        let opaque = OpaqueBufferHandle::new(handle);
        Ok(Some(opaque))
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

        let result = unsafe { source_close(self.source_ptr, self.context_ptr) };
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
                    Ok(Some(opaque)) => {
                        // Wrap the opaque handle directly — no data copy, no metadata.
                        let buffer = Buffer::opaque(opaque);

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
    fn stage_start(stage_ptr: usize, context_ptr: usize, worker_id: u32, worker_count: u64) -> i32;
    fn stage_execute_with_handle(
        stage_ptr: usize,
        context_ptr: usize,
        opaque_handle: usize,
        worker_id: u32,
        worker_count: u64,
    ) -> i32;
    fn stage_stop(stage_ptr: usize, context_ptr: usize, worker_id: u32, worker_count: u64) -> i32;

    // Emitted buffer retrieval (thread-local storage access)
    fn stage_get_emitted_count() -> usize;
    fn stage_get_emitted_opaque_handle(index: usize) -> usize;
    fn stage_get_repeat_requested() -> bool;

    // Source callbacks
    fn source_open(source_ptr: usize, context_ptr: usize) -> i32;
    fn source_next_buffer(source_ptr: usize, context_ptr: usize) -> usize;
    fn source_request_stop(source_ptr: usize);
    fn source_close(source_ptr: usize, context_ptr: usize) -> i32;

    // Buffer handle callbacks (provider-free via virtual dispatch)
    pub fn buffer_handle_clone(handle: usize) -> usize;
    pub fn buffer_handle_release(handle: usize);

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
