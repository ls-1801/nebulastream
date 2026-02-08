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

//! Pipeline metadata for reference counting and lifecycle management.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Metadata for tracking pipeline execution state.
///
/// Each active pipeline has associated metadata that tracks:
/// - Pending tasks (reference counting)
/// - Termination flag (for graceful shutdown)
/// - Lifecycle state (setup success tracking)
/// - End-of-stream coordination (multi-source tracking)
/// - Source-specific state (for source nodes only)
#[derive(Debug)]
pub struct PipelineMetadata {
    /// Number of pending work tasks for this pipeline.
    ///
    /// Incremented when a work task is enqueued, decremented after execution.
    /// Used for reference counting to ensure graceful shutdown.
    pub pending_tasks: AtomicUsize,

    /// Whether this pipeline has been requested to terminate.
    ///
    /// Set to true when all expected sources signal end-of-stream. Once this is true
    /// and pending_tasks reaches zero, the pipeline is removed.
    pub requires_termination: AtomicBool,

    /// Whether the pipeline's setup() method completed successfully.
    ///
    /// Set to true after setup() succeeds. If false, buffers will not be
    /// processed by this pipeline. Used to prevent execution before initialization.
    pub setup_succeeded: AtomicBool,

    /// Number of sources expected to feed this pipeline.
    ///
    /// Set during graph deployment based on graph analysis. Used for
    /// end-of-stream coordination - when all sources signal EOS, the
    /// pipeline can be gracefully stopped.
    pub expected_sources: AtomicUsize,

    /// Number of sources that have signaled end-of-stream.
    ///
    /// Incremented when a source signals it will emit no more buffers.
    /// When this equals expected_sources, the pipeline can be stopped.
    pub eos_received: AtomicUsize,

    /// Whether the source has been started (source nodes only).
    ///
    /// Set to true after the source's start() method is called successfully.
    /// Only meaningful for source nodes; regular pipelines ignore this field.
    pub source_started: AtomicBool,

    /// Whether the source has been requested to stop (source nodes only).
    ///
    /// Wrapped in Arc so the flag can be shared with SourceEmitHandle.
    /// Set to true when the source should stop emitting data. Sources check
    /// this flag via SourceEmitHandle::should_stop(). Only meaningful for
    /// source nodes; regular pipelines ignore this field.
    pub source_stop_requested: Arc<AtomicBool>,
}

impl PipelineMetadata {
    /// Create new pipeline metadata with zero pending tasks.
    pub fn new() -> Self {
        Self {
            pending_tasks: AtomicUsize::new(0),
            requires_termination: AtomicBool::new(false),
            setup_succeeded: AtomicBool::new(false),
            expected_sources: AtomicUsize::new(0),
            eos_received: AtomicUsize::new(0),
            source_started: AtomicBool::new(false),
            source_stop_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Increment the pending task counter.
    pub fn increment_pending(&self) {
        self.pending_tasks.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrement the pending task counter and return the new value.
    /// Uses saturating subtraction to avoid overflow if called when already at 0.
    pub fn decrement_pending(&self) -> usize {
        let prev = self.pending_tasks.load(Ordering::SeqCst);
        if prev == 0 {
            return 0;
        }
        self.pending_tasks.fetch_sub(1, Ordering::SeqCst) - 1
    }

    /// Get the current number of pending tasks.
    pub fn get_pending(&self) -> usize {
        self.pending_tasks.load(Ordering::SeqCst)
    }

    /// Mark this pipeline as requiring termination.
    pub fn request_termination(&self) {
        self.requires_termination.store(true, Ordering::SeqCst);
    }

    /// Check if this pipeline requires termination.
    pub fn should_terminate(&self) -> bool {
        self.requires_termination.load(Ordering::SeqCst)
    }

    /// Mark that setup() completed successfully.
    pub fn mark_setup_succeeded(&self) {
        self.setup_succeeded.store(true, Ordering::SeqCst);
    }

    /// Check if setup() completed successfully.
    pub fn is_setup_succeeded(&self) -> bool {
        self.setup_succeeded.load(Ordering::SeqCst)
    }

    /// Set the number of expected sources for this pipeline.
    pub fn set_expected_sources(&self, count: usize) {
        self.expected_sources.store(count, Ordering::SeqCst);
    }

    /// Get the number of expected sources.
    pub fn get_expected_sources(&self) -> usize {
        self.expected_sources.load(Ordering::SeqCst)
    }

    /// Increment the end-of-stream counter and return the new value.
    pub fn increment_eos(&self) -> usize {
        self.eos_received.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Get the number of sources that have signaled end-of-stream.
    pub fn get_eos_received(&self) -> usize {
        self.eos_received.load(Ordering::SeqCst)
    }

    /// Check if all expected sources have signaled end-of-stream.
    pub fn all_sources_finished(&self) -> bool {
        let expected = self.expected_sources.load(Ordering::SeqCst);
        let received = self.eos_received.load(Ordering::SeqCst);
        expected > 0 && received >= expected
    }

    /// Mark that the source has been started (source nodes only).
    ///
    /// This is called after a source's start() method completes successfully.
    pub fn mark_source_started(&self) {
        self.source_started.store(true, Ordering::SeqCst);
    }

    /// Check if the source has been started (source nodes only).
    ///
    /// Returns true if the source's start() method has been called.
    pub fn is_source_started(&self) -> bool {
        self.source_started.load(Ordering::SeqCst)
    }

    /// Request that the source stop emitting data (source nodes only).
    ///
    /// This sets a flag that sources can check via SourceEmitHandle::should_stop().
    pub fn request_source_stop(&self) {
        self.source_stop_requested.store(true, Ordering::SeqCst);
    }

    /// Check if the source has been requested to stop (source nodes only).
    ///
    /// Returns true if the source should stop emitting data.
    pub fn is_source_stop_requested(&self) -> bool {
        self.source_stop_requested.load(Ordering::SeqCst)
    }

    /// Get a shared reference to the source stop flag (source nodes only).
    ///
    /// Returns an Arc clone of the stop_requested flag, suitable for passing
    /// to SourceEmitHandle so it can observe stop requests in real-time.
    pub fn get_source_stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.source_stop_requested)
    }
}

impl Default for PipelineMetadata {
    fn default() -> Self {
        Self::new()
    }
}
