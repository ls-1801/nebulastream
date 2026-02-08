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

//! Mock pipeline implementations for testing.
//!
//! This module provides mock implementations of various pipeline types
//! for use in testing. These include filter, multi-buffer, occasional
//! emission, and stateful pipelines.
//!
//! # Examples
//!
//! ```
//! use adaptive_engine::pipeline::mocks::{FilterPipeline, MultibufferPipeline};
//! use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
//!
//! // Create a filter pipeline that only passes buffers longer than 10 bytes
//! let filter = FilterPipeline::new(
//!     PipelineId::new("filter"),
//!     |b| b.data().len() > 10
//! );
//!
//! // Create a multi-buffer pipeline that emits 3 buffers for each input
//! let fanout = MultibufferPipeline::new(PipelineId::new("fanout"), 3);
//! ```

use crate::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Filter pipeline that conditionally passes buffers based on a predicate.
///
/// This pipeline evaluates a predicate function for each input buffer and
/// only emits the buffer if the predicate returns `true`.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::mocks::FilterPipeline;
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
///
/// let filter = FilterPipeline::new(
///     PipelineId::new("size-filter"),
///     |b| b.data().len() > 5
/// );
///
/// // Note: For testing, you would need to create a mock context
/// // In practice, the executor provides the context
/// ```
pub struct FilterPipeline {
    id: PipelineId,
    predicate: Box<dyn Fn(&Buffer) -> bool + Send + Sync>,
}

impl FilterPipeline {
    /// Create a new filter pipeline with a predicate function.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this pipeline
    /// * `predicate` - Function that returns `true` if the buffer should be passed through
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::FilterPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let filter = FilterPipeline::new(
    ///     PipelineId::new("filter"),
    ///     |b| b.data().len() > 10
    /// );
    /// ```
    pub fn new<F>(id: PipelineId, predicate: F) -> Self
    where
        F: Fn(&Buffer) -> bool + Send + Sync + 'static,
    {
        Self {
            id,
            predicate: Box::new(predicate),
        }
    }
}

impl Pipeline for FilterPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        if (self.predicate)(&input) {
            Ok(vec![input])
        } else {
            Ok(vec![])
        }
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Multi-buffer pipeline that emits multiple buffers for each input.
///
/// This pipeline simulates fanout scenarios by creating N child buffers
/// for each input buffer, where N is the fanout factor.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::mocks::MultibufferPipeline;
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
///
/// let fanout = MultibufferPipeline::new(PipelineId::new("fanout"), 3);
///
/// // Note: For testing, you would need to create a mock context
/// // In practice, the executor provides the context
/// ```
pub struct MultibufferPipeline {
    id: PipelineId,
    fanout: usize,
}

impl MultibufferPipeline {
    /// Create a new multi-buffer pipeline.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this pipeline
    /// * `fanout` - Number of output buffers to emit for each input
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::MultibufferPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let fanout = MultibufferPipeline::new(PipelineId::new("fanout"), 5);
    /// ```
    pub fn new(id: PipelineId, fanout: usize) -> Self {
        Self { id, fanout }
    }

    /// Get the fanout factor for this pipeline.
    pub fn fanout(&self) -> usize {
        self.fanout
    }
}

impl Pipeline for MultibufferPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let mut outputs = Vec::with_capacity(self.fanout);

        for _i in 0..self.fanout {
            let child_data = input.data().to_vec();
            outputs.push(Buffer::new(child_data));
        }

        Ok(outputs)
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Pipeline that emits buffers occasionally, simulating windowing behavior.
///
/// This pipeline collects buffers and only emits every Nth buffer, where N
/// is the emission interval. This simulates windowing or aggregation scenarios.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::mocks::OccasionalEmissionPipeline;
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
///
/// let windowing = OccasionalEmissionPipeline::new(PipelineId::new("window"), 3);
///
/// // Note: For testing, you would need to create a mock context
/// // In practice, the executor provides the context
/// ```
pub struct OccasionalEmissionPipeline {
    id: PipelineId,
    emission_interval: usize,
    counter: AtomicUsize,
}

impl OccasionalEmissionPipeline {
    /// Create a new occasional emission pipeline.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this pipeline
    /// * `emission_interval` - Emit a buffer every N inputs
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::OccasionalEmissionPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let windowing = OccasionalEmissionPipeline::new(PipelineId::new("window"), 10);
    /// ```
    pub fn new(id: PipelineId, emission_interval: usize) -> Self {
        Self {
            id,
            emission_interval,
            counter: AtomicUsize::new(0),
        }
    }

    /// Get the emission interval for this pipeline.
    pub fn emission_interval(&self) -> usize {
        self.emission_interval
    }

    /// Reset the internal counter to zero.
    ///
    /// This is useful for testing scenarios where you want to reset
    /// the pipeline state.
    pub fn reset_counter(&self) {
        self.counter.store(0, Ordering::SeqCst);
    }

    /// Get the current counter value.
    pub fn counter(&self) -> usize {
        self.counter.load(Ordering::SeqCst)
    }
}

impl Pipeline for OccasionalEmissionPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let count = self.counter.fetch_add(1, Ordering::SeqCst) + 1;

        if count.is_multiple_of(self.emission_interval) {
            Ok(vec![input])
        } else {
            Ok(vec![])
        }
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Stateful pipeline for testing purposes only.
///
/// This mock pipeline maintains simple key-value state. It's not intended
/// for production use, only for testing pipeline graph construction and
/// validating that stateful pipelines can be integrated into the system.
///
/// **Note:** This uses a `Mutex` for interior mutability, which is acceptable
/// for testing but not for production. Real stateful pipelines would use
/// proper state management with MVCC and snapshots.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::mocks::StatefulPipeline;
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
///
/// let stateful = StatefulPipeline::new(PipelineId::new("state"));
///
/// // Store some state
/// stateful.set_state("count".to_string(), vec![0, 0, 0, 5]);
///
/// // Retrieve state
/// let value = stateful.get_state("count").unwrap();
/// assert_eq!(value, vec![0, 0, 0, 5]);
/// ```
pub struct StatefulPipeline {
    id: PipelineId,
    state: Mutex<HashMap<String, Vec<u8>>>,
}

impl StatefulPipeline {
    /// Create a new stateful pipeline.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this pipeline
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::StatefulPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let stateful = StatefulPipeline::new(PipelineId::new("state"));
    /// ```
    pub fn new(id: PipelineId) -> Self {
        Self {
            id,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Set a state value.
    ///
    /// # Arguments
    ///
    /// * `key` - The state key
    /// * `value` - The state value
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::StatefulPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let stateful = StatefulPipeline::new(PipelineId::new("state"));
    /// stateful.set_state("key".to_string(), vec![1, 2, 3]);
    /// ```
    pub fn set_state(&self, key: String, value: Vec<u8>) {
        let mut state = self.state.lock().unwrap();
        state.insert(key, value);
    }

    /// Get a state value.
    ///
    /// # Arguments
    ///
    /// * `key` - The state key to retrieve
    ///
    /// # Returns
    ///
    /// The state value if it exists, or `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::StatefulPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let stateful = StatefulPipeline::new(PipelineId::new("state"));
    /// stateful.set_state("key".to_string(), vec![1, 2, 3]);
    ///
    /// let value = stateful.get_state("key").unwrap();
    /// assert_eq!(value, vec![1, 2, 3]);
    /// ```
    pub fn get_state(&self, key: &str) -> Option<Vec<u8>> {
        let state = self.state.lock().unwrap();
        state.get(key).cloned()
    }

    /// Clear all state.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::StatefulPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let stateful = StatefulPipeline::new(PipelineId::new("state"));
    /// stateful.set_state("key".to_string(), vec![1, 2, 3]);
    /// stateful.clear_state();
    ///
    /// assert!(stateful.get_state("key").is_none());
    /// ```
    pub fn clear_state(&self) {
        let mut state = self.state.lock().unwrap();
        state.clear();
    }

    /// Get the number of state entries.
    pub fn state_len(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.len()
    }
}

impl Pipeline for StatefulPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        // For this mock, we simply pass through the buffer
        // Real stateful pipelines would interact with state during execution
        Ok(vec![input])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Sink pipeline that accepts and stores buffers without emitting outputs.
///
/// This mock pipeline is useful for testing as a terminal node in a pipeline
/// graph. It accepts all input buffers and stores them internally without
/// emitting any outputs.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::mocks::SinkPipeline;
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
///
/// let sink = SinkPipeline::new("sink");
///
/// // Note: For testing, you would need to create a mock context
/// // In practice, the executor provides the context
/// assert_eq!(sink.buffer_count(), 0); // Initially no buffers received
/// ```
pub struct SinkPipeline {
    id: PipelineId,
    received_buffers: AtomicUsize,
}

impl SinkPipeline {
    /// Create a new sink pipeline.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this pipeline (can be a string)
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::mocks::SinkPipeline;
    ///
    /// let sink = SinkPipeline::new("my-sink");
    /// ```
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: PipelineId::new(id),
            received_buffers: AtomicUsize::new(0),
        }
    }

    /// Get the number of buffers received by this sink.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::executor::ExecutorContext;
    /// use adaptive_engine::pipeline::mocks::SinkPipeline;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId};
    /// use std::sync::mpsc::channel;
    ///
    /// let sink = SinkPipeline::new("sink");
    /// assert_eq!(sink.buffer_count(), 0);
    ///
    /// let input = Buffer::new(vec![1, 2, 3]);
    /// let (tx, _rx) = channel();
    /// let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);
    /// sink.execute(input, &context).unwrap();
    ///
    /// assert_eq!(sink.buffer_count(), 1);
    /// ```
    pub fn buffer_count(&self) -> usize {
        self.received_buffers.load(Ordering::SeqCst)
    }

    /// Reset the buffer counter to zero.
    pub fn reset_counter(&self) {
        self.received_buffers.store(0, Ordering::SeqCst);
    }
}

impl Pipeline for SinkPipeline {
    fn execute(
        &self,
        _input: Buffer,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        self.received_buffers.fetch_add(1, Ordering::SeqCst);
        // Sinks don't emit any outputs
        Ok(vec![])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    // Helper function to create a mock context for testing
    fn create_test_context() -> crate::executor::ExecutorContext {
        let (tx, _rx) = channel();
        crate::executor::ExecutorContext::new(PipelineId::new("test"), 0, 1, tx)
    }

    #[test]
    fn test_filter_pipeline_pass() {
        let filter = FilterPipeline::new(PipelineId::new("filter"), |b| b.data().len() > 5);
        let context = create_test_context();

        let input = Buffer::new(vec![1, 2, 3, 4, 5, 6]);
        let output = filter.execute(input, &context).unwrap();

        assert_eq!(output.len(), 1);
    }

    #[test]
    fn test_filter_pipeline_block() {
        let filter = FilterPipeline::new(PipelineId::new("filter"), |b| b.data().len() > 5);
        let context = create_test_context();

        let input = Buffer::new(vec![1, 2]);
        let output = filter.execute(input, &context).unwrap();

        assert_eq!(output.len(), 0);
    }

    #[test]
    fn test_multibuffer_pipeline() {
        let fanout = MultibufferPipeline::new(PipelineId::new("fanout"), 3);
        let context = create_test_context();

        let input = Buffer::new(vec![1, 2, 3]);
        let outputs = fanout.execute(input, &context).unwrap();

        assert_eq!(outputs.len(), 3);

        // All outputs should have the same data
        for output in &outputs {
            assert_eq!(output.data(), &[1, 2, 3]);
        }
    }

    #[test]
    fn test_occasional_emission_pipeline() {
        let windowing = OccasionalEmissionPipeline::new(PipelineId::new("window"), 3);
        let context = create_test_context();

        let b1 = Buffer::new(vec![1]);
        let b2 = Buffer::new(vec![2]);
        let b3 = Buffer::new(vec![3]);
        let b4 = Buffer::new(vec![4]);

        assert_eq!(windowing.execute(b1, &context).unwrap().len(), 0);
        assert_eq!(windowing.execute(b2, &context).unwrap().len(), 0);
        assert_eq!(windowing.execute(b3, &context).unwrap().len(), 1);
        assert_eq!(windowing.execute(b4, &context).unwrap().len(), 0);
    }

    #[test]
    fn test_occasional_emission_reset() {
        let windowing = OccasionalEmissionPipeline::new(PipelineId::new("window"), 2);
        let context = create_test_context();

        let b1 = Buffer::new(vec![1]);
        let b2 = Buffer::new(vec![2]);

        assert_eq!(windowing.execute(b1, &context).unwrap().len(), 0);
        assert_eq!(windowing.counter(), 1);

        windowing.reset_counter();
        assert_eq!(windowing.counter(), 0);

        assert_eq!(windowing.execute(b2, &context).unwrap().len(), 0);
        assert_eq!(windowing.counter(), 1);
    }

    #[test]
    fn test_stateful_pipeline() {
        let stateful = StatefulPipeline::new(PipelineId::new("state"));
        let context = create_test_context();

        // Set and get state
        stateful.set_state("count".to_string(), vec![0, 0, 0, 5]);
        let value = stateful.get_state("count").unwrap();
        assert_eq!(value, vec![0, 0, 0, 5]);

        // Test execution passes through
        let input = Buffer::new(vec![1, 2, 3]);
        let output = stateful.execute(input, &context).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].data(), &[1, 2, 3]);

        // Test state_len
        assert_eq!(stateful.state_len(), 1);

        // Test clear
        stateful.clear_state();
        assert!(stateful.get_state("count").is_none());
        assert_eq!(stateful.state_len(), 0);
    }
}
