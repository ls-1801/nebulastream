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

//! Source nodes for pipeline graphs.
//!
//! This module defines the `Source` trait and related types for source nodes
//! that generate data and feed it into pipeline graphs. Sources are special
//! nodes with no predecessors that have their own runtime (threads/closures)
//! and lifecycle management.
//!
//! # Overview
//!
//! Sources differ from pipelines in several key ways:
//! - Sources **generate** data, pipelines **transform** data
//! - Sources have `start()` and `stop()` lifecycle methods
//! - Sources cannot have predecessors in the graph
//! - Sources run asynchronously (typically in separate threads)
//!
//! # Lifecycle
//!
//! Sources follow a four-phase lifecycle:
//!
//! 1. **Setup**: `setup()` - Initialize resources (connections, threads, etc.)
//! 2. **Start**: `start(emit_handle)` - Begin emitting data
//! 3. **Stop**: `stop()` - Signal the source to stop emitting
//! 4. **Teardown**: `teardown()` - Clean up resources
//!
//! # Examples
//!
//! ```no_run
//! use adaptive_engine::source::{Source, SourceEmitHandle, SourceError};
//! use adaptive_engine::pipeline::{PipelineId, Buffer};
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicBool, Ordering};
//!
//! struct SimpleSource {
//!     id: PipelineId,
//!     stopped: Arc<AtomicBool>,
//! }
//!
//! impl Source for SimpleSource {
//!     fn setup(&self) -> Result<(), SourceError> {
//!         // Initialize resources
//!         Ok(())
//!     }
//!
//!     fn start(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
//!         let stopped = self.stopped.clone();
//!
//!         std::thread::spawn(move || {
//!             let mut seq: u64 = 1;
//!             while !stopped.load(Ordering::Relaxed) {
//!                 let buffer = Buffer::new(vec![seq as u8]);
//!                 if let Err(_) = emit_handle.emit(buffer) {
//!                     break;
//!                 }
//!                 seq += 1;
//!                 std::thread::sleep(std::time::Duration::from_millis(100));
//!             }
//!         });
//!
//!         Ok(())
//!     }
//!
//!     fn stop(&self) -> Result<(), SourceError> {
//!         self.stopped.store(true, Ordering::Relaxed);
//!         Ok(())
//!     }
//!
//!     fn teardown(&self) -> Result<(), SourceError> {
//!         // Clean up resources
//!         Ok(())
//!     }
//!
//!     fn id(&self) -> &PipelineId {
//!         &self.id
//!     }
//! }
//! ```

pub mod generator_source;
pub mod test_source;
pub mod wrapper;

use crate::executor::queue::TaskQueue;
use crate::executor::task::Task;
use crate::pipeline::{Buffer, PipelineId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Error types for source operations.
#[derive(Error, Debug)]
pub enum SourceError {
    /// Source setup failed.
    #[error("Source setup failed: {0}")]
    SetupFailed(String),

    /// Source start failed.
    #[error("Source start failed: {0}")]
    StartFailed(String),

    /// Source stop failed.
    #[error("Source stop failed: {0}")]
    StopFailed(String),

    /// Source teardown failed.
    #[error("Source teardown failed: {0}")]
    TeardownFailed(String),

    /// Failed to emit buffer.
    #[error("Failed to emit buffer: {0}")]
    EmitFailed(String),

    /// Source not found in graph.
    #[error("Source not found: {0}")]
    NotFound(PipelineId),

    /// Invalid source configuration.
    #[error("Invalid source configuration: {0}")]
    InvalidConfiguration(String),
}

/// Trait for source nodes that generate data for pipeline graphs.
///
/// Sources are special graph nodes that have no predecessors and generate
/// data to feed into the pipeline. They have their own runtime (typically
/// a separate thread) and lifecycle management.
///
/// # Lifecycle
///
/// Sources follow a four-phase lifecycle:
///
/// 1. **Setup**: Initialize resources (connections, file handles, etc.)
/// 2. **Start**: Begin generating and emitting data
/// 3. **Stop**: Signal the source to stop generating data
/// 4. **Teardown**: Clean up resources
///
/// # Thread Safety
///
/// Sources must be `Send + Sync` because they are typically stored in
/// `Arc<dyn Source>` and accessed from multiple threads.
///
/// # Examples
///
/// See module-level documentation for a complete example.
pub trait Source: Send + Sync {
    /// Initialize the source.
    ///
    /// This method is called once during graph deployment, before `start()`.
    /// Use this to allocate resources, establish connections, or perform
    /// any initialization that might fail.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if setup succeeded, or an error if initialization failed.
    ///
    /// # Errors
    ///
    /// Should return `SourceError::SetupFailed` if initialization fails.
    fn setup(&self) -> Result<(), SourceError> {
        Ok(())
    }

    /// Start the source and begin emitting data.
    ///
    /// This method is called after all successor pipelines have been set up
    /// and are ready to receive data. The source should begin generating and
    /// emitting data using the provided `emit_handle`.
    ///
    /// Sources typically spawn a worker thread in this method that runs until
    /// `stop()` is called.
    ///
    /// # Arguments
    ///
    /// * `emit_handle` - Handle for emitting buffers to successor pipelines
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the source started successfully, or an error if
    /// starting failed.
    ///
    /// # Errors
    ///
    /// Should return `SourceError::StartFailed` if the source cannot start.
    fn start(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError>;

    /// Signal the source to stop emitting data.
    ///
    /// This method should signal any worker threads to stop. It does not need
    /// to wait for threads to finish (that's handled by `teardown()`).
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the stop signal was sent successfully.
    ///
    /// # Errors
    ///
    /// Should return `SourceError::StopFailed` if the source cannot be stopped.
    fn stop(&self) -> Result<(), SourceError>;

    /// Clean up source resources.
    ///
    /// This method is called after `stop()` to clean up any resources allocated
    /// by the source (file handles, connections, threads, etc.).
    ///
    /// The source should wait for any worker threads to finish in this method.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if teardown succeeded.
    ///
    /// # Errors
    ///
    /// Should return `SourceError::TeardownFailed` if cleanup fails.
    fn teardown(&self) -> Result<(), SourceError> {
        Ok(())
    }

    /// Get the unique identifier for this source.
    ///
    /// # Returns
    ///
    /// Returns a reference to this source's pipeline ID.
    fn id(&self) -> &PipelineId;
}

/// Handle for sources to emit buffers to the executor.
///
/// This is a restricted version of `ExecutorHandle` that only allows sources
/// to emit buffers and check if they should stop. It follows the principle of
/// least privilege by not exposing graph deployment or shutdown operations.
///
/// # Examples
///
/// ```no_run
/// use adaptive_engine::source::SourceEmitHandle;
/// use adaptive_engine::pipeline::Buffer;
///
/// fn emit_data(handle: SourceEmitHandle) {
///     let mut seq: u64 = 1;
///     while !handle.should_stop() {
///         let buffer = Buffer::new(vec![seq as u8]);
///         if let Err(_) = handle.emit(buffer) {
///             break;
///         }
///         seq += 1;
///     }
/// }
/// ```
#[derive(Clone)]
pub struct SourceEmitHandle {
    source_id: PipelineId,
    query_id: u64,
    task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
    stop_requested: Arc<AtomicBool>,
    task_available: Arc<std::sync::Condvar>,
}

impl SourceEmitHandle {
    /// Create a new source emit handle.
    ///
    /// This is an internal method used by the executor to create handles
    /// for sources.
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the source that will use this handle
    /// * `query_id` - ID of the query this source belongs to
    /// * `task_queue` - Task queue for submitting buffers
    /// * `stop_requested` - Shared flag indicating if the source should stop
    /// * `task_available` - Condvar to notify worker threads of new tasks
    pub(crate) fn new(
        source_id: PipelineId,
        query_id: u64,
        task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
        stop_requested: Arc<AtomicBool>,
        task_available: Arc<std::sync::Condvar>,
    ) -> Self {
        Self {
            source_id,
            query_id,
            task_queue,
            stop_requested,
            task_available,
        }
    }

    /// Emit a buffer to successor pipelines.
    ///
    /// The buffer will be enqueued as a work task and routed to all
    /// successor pipelines by the executor.
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer to emit
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the buffer was successfully enqueued, or an error
    /// if enqueueing failed.
    ///
    /// # Errors
    ///
    /// Returns `SourceError::EmitFailed` if the buffer cannot be enqueued.
    pub fn emit(&self, buffer: Buffer) -> Result<(), SourceError> {
        let task = Task::WorkTask {
            query_id: self.query_id,
            pipeline_id: self.source_id.clone(),
            buffer,
        };

        self.task_queue
            .lock()
            .map_err(|e| SourceError::EmitFailed(format!("Failed to lock task queue: {}", e)))?
            .push(task);

        self.task_available.notify_one();
        Ok(())
    }

    /// Check if the source should stop emitting data.
    ///
    /// Sources should periodically check this flag and stop their worker
    /// threads when it returns `true`.
    ///
    /// # Returns
    ///
    /// Returns `true` if the source should stop, `false` otherwise.
    pub fn should_stop(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    /// Signal that the source has finished emitting data (end of stream).
    ///
    /// This enqueues a StopPipelineTask for the source, which triggers
    /// cascading shutdown through the DAG: the source pipeline is torn down,
    /// then EndOfStream is sent to all successors.
    ///
    /// Call this when the source naturally exhausts (no more data to emit).
    pub fn end_of_stream(&self) -> Result<(), SourceError> {
        let task = Task::StopPipelineTask {
            pipeline_id: self.source_id.clone(),
        };

        self.task_queue
            .lock()
            .map_err(|e| SourceError::EmitFailed(format!("Failed to lock task queue: {}", e)))?
            .push(task);

        self.task_available.notify_one();
        Ok(())
    }

    /// Signal that the source encountered an error.
    ///
    /// This enqueues a SourceError task that causes the executor to record
    /// the error and terminate the query. Used when a C++ source throws
    /// during next_buffer().
    pub fn signal_error(&self, error: &str) -> Result<(), SourceError> {
        let task = Task::SourceError {
            query_id: self.query_id,
            source_id: self.source_id.clone(),
            error: error.to_string(),
        };

        self.task_queue
            .lock()
            .map_err(|e| SourceError::EmitFailed(format!("Failed to lock task queue: {}", e)))?
            .push(task);

        self.task_available.notify_one();
        Ok(())
    }
}

// Re-export key types for convenience
pub use generator_source::{GeneratorConfig, GeneratorSource};
pub use test_source::{TestSource, TestSourceHandle};
pub use wrapper::SourcePipeline;
