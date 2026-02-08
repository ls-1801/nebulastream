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

//! Pipeline execution context for NebulaStream compatibility.
//!
//! Provides the `PipelineExecutionContext` trait and implementation that gives
//! pipelines access to executor services during execution, matching NebulaStream's
//! PipelineExecutionContext interface.

use crate::executor::QueryId;
use crate::pipeline::{Buffer, PipelineError, PipelineId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// Execution context provided to pipelines during execution.
///
/// Mirrors NebulaStream's PipelineExecutionContext capabilities, providing
/// pipelines with access to buffer emission, allocation, and worker information.
///
/// # NebulaStream Compatibility
///
/// This trait matches the core methods of NebulaStream's PipelineExecutionContext:
/// - `emit_buffer()` - corresponds to `emitBuffer()`
/// - `allocate_buffer()` - corresponds to buffer pool allocation
/// - `get_worker_id()` / `get_worker_count()` - worker thread information
///
/// # Examples
///
/// ```no_run
/// use adaptive_engine::executor::PipelineExecutionContext;
/// use adaptive_engine::pipeline::{Buffer, PipelineError};
///
/// fn process_with_context(
///     input: Buffer,
///     context: &dyn PipelineExecutionContext
/// ) -> Result<Vec<Buffer>, PipelineError> {
///     // Allocate a new buffer from the pool
///     let mut output = context.allocate_buffer()?;
///
///     // Process data...
///
///     // Emit via context (NebulaStream style)
///     context.emit_buffer(output);
///
///     // Return empty vec when using emit
///     Ok(vec![])
/// }
/// ```
pub trait PipelineExecutionContext: Send + Sync {
    /// Emit a buffer to successor pipelines.
    fn emit_buffer(&self, buffer: Buffer) -> bool;

    /// Allocate a new buffer from the executor's buffer pool.
    fn allocate_buffer(&self) -> Result<Buffer, PipelineError>;

    /// Get the worker thread ID.
    fn get_worker_id(&self) -> usize;

    /// Get the total number of worker threads.
    fn get_worker_count(&self) -> usize;

    /// Get the current pipeline ID.
    fn get_pipeline_id(&self) -> &PipelineId;

    /// Request re-execution of the current task with the given buffer.
    ///
    /// The caller provides the buffer to re-execute with. The executor
    /// re-enqueues this buffer as-is without copying or modifying it.
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer to re-execute with.
    /// * `delay_ms` - The delay in milliseconds before re-queuing the task.
    ///   If 0, the task is re-queued immediately.
    fn repeat_task(&self, buffer: Buffer, delay_ms: u64);
}

/// Default execution context implementation backed by the executor.
///
/// # Repeat Task Mechanism
///
/// When `repeat_task(buffer, delay_ms)` is called during pipeline execution,
/// the context stores the buffer and delay value. After `pipeline.execute()`
/// returns, `execute_work_task()` in the executor takes the stored buffer
/// and re-enqueues it as-is.
///
/// # Thread Safety
///
/// `ExecutorContext` is `Send + Sync` because:
/// - `pipeline_id` is immutable after creation
/// - `worker_id` and `worker_count` are immutable
/// - `emit_tx` is a `Sender` which is `Send + Sync`
/// - `repeat_buffer` is behind a Mutex, `repeat_delay_ms` is atomic
pub struct ExecutorContext {
    /// The ID of the pipeline being executed
    pipeline_id: PipelineId,

    /// The ID of the query this pipeline belongs to
    #[allow(dead_code)]
    query_id: QueryId,

    /// The worker thread ID (0 for single-threaded executor)
    worker_id: usize,

    /// The total number of worker threads (1 for single-threaded executor)
    worker_count: usize,

    /// Channel for emitting buffers to the executor
    emit_tx: Sender<(PipelineId, Buffer)>,

    /// Buffer to re-enqueue, set by repeat_task during execution.
    repeat_buffer: Mutex<Option<Buffer>>,

    /// Delay value set by repeat_task (in milliseconds).
    repeat_delay_ms: AtomicU64,
}

impl ExecutorContext {
    /// Create a new executor context.
    pub fn new(
        pipeline_id: PipelineId,
        worker_id: usize,
        worker_count: usize,
        emit_tx: Sender<(PipelineId, Buffer)>,
    ) -> Self {
        Self {
            pipeline_id,
            query_id: 0,
            worker_id,
            worker_count,
            emit_tx,
            repeat_buffer: Mutex::new(None),
            repeat_delay_ms: AtomicU64::new(0),
        }
    }

    /// Create a new executor context with a specific query_id.
    pub fn with_query_id(
        pipeline_id: PipelineId,
        query_id: QueryId,
        worker_id: usize,
        worker_count: usize,
        emit_tx: Sender<(PipelineId, Buffer)>,
    ) -> Self {
        Self {
            pipeline_id,
            query_id,
            worker_id,
            worker_count,
            emit_tx,
            repeat_buffer: Mutex::new(None),
            repeat_delay_ms: AtomicU64::new(0),
        }
    }

    /// Take the repeat buffer, if one was stored by repeat_task.
    pub fn take_repeat_buffer(&self) -> Option<Buffer> {
        self.repeat_buffer.lock().unwrap().take()
    }

    /// Get the delay value set by repeat_task.
    pub fn get_repeat_delay_ms(&self) -> u64 {
        self.repeat_delay_ms.load(Ordering::SeqCst)
    }
}

impl PipelineExecutionContext for ExecutorContext {
    fn emit_buffer(&self, buffer: Buffer) -> bool {
        self.emit_tx
            .send((self.pipeline_id.clone(), buffer))
            .is_ok()
    }

    fn allocate_buffer(&self) -> Result<Buffer, PipelineError> {
        Ok(Buffer::new(vec![]))
    }

    fn get_worker_id(&self) -> usize {
        self.worker_id
    }

    fn get_worker_count(&self) -> usize {
        self.worker_count
    }

    fn get_pipeline_id(&self) -> &PipelineId {
        &self.pipeline_id
    }

    fn repeat_task(&self, buffer: Buffer, delay_ms: u64) {
        // Store the buffer and delay value.
        // The executor will take the buffer after pipeline.execute() returns
        // and re-enqueue it as-is.
        *self.repeat_buffer.lock().unwrap() = Some(buffer);
        self.repeat_delay_ms.store(delay_ms, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn test_context_creation() {
        let (tx, _rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);

        assert_eq!(context.get_worker_id(), 0);
        assert_eq!(context.get_worker_count(), 1);
        assert_eq!(context.get_pipeline_id().as_str(), "test");
    }

    #[test]
    fn test_context_emit_buffer() {
        let (tx, rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);

        let buffer = Buffer::new(vec![1, 2, 3]);
        assert!(context.emit_buffer(buffer.clone()));

        let (pipeline_id, received_buffer) = rx.recv().unwrap();
        assert_eq!(pipeline_id.as_str(), "test");
        assert_eq!(received_buffer.data(), &[1, 2, 3]);
    }

    #[test]
    fn test_context_allocate_buffer() {
        let (tx, _rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);

        let buffer = context.allocate_buffer().unwrap();
        assert_eq!(buffer.data().len(), 0);
    }

    #[test]
    fn test_context_emit_fails_when_channel_closed() {
        let (tx, rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);

        drop(rx);

        let buffer = Buffer::new(vec![1, 2, 3]);
        assert!(!context.emit_buffer(buffer));
    }
}
