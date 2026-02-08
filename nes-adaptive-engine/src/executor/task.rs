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

//! Task types for the execution engine.
//!
//! Defines the task enum variants that can be submitted to the executor's
//! task queue for processing by the execution threads.

use crate::graph::{PipelineGraph, PipelineNode};
use crate::pipeline::{Buffer, PipelineId};
use std::sync::Weak;

/// Task variants for the execution engine.
///
/// Tasks are submitted to the executor's thread-safe queue and processed
/// by execution threads in FIFO order.
pub enum Task {
    /// Execute a pipeline with the given buffer.
    ///
    /// This is the primary work task that processes buffers through pipelines.
    WorkTask {
        /// ID of the pipeline to execute
        pipeline_id: PipelineId,
        /// Direct weak reference to the pipeline node (avoids global metadata lookup)
        node: Weak<PipelineNode>,
        /// Buffer to process
        buffer: Buffer,
    },

    /// Deploy a new pipeline graph.
    ///
    /// Merges the new graph into the executor's single graph. This is used for
    /// deploying new pipelines into the running system.
    DeployGraph {
        /// The new graph to deploy
        graph: PipelineGraph,
    },

    /// Start a source node in the pipeline graph.
    ///
    /// This task is enqueued after graph deployment to asynchronously start
    /// source nodes. Sources begin emitting data only after all successor
    /// pipelines have been set up and are ready to receive buffers.
    StartSource {
        /// ID of the source to start
        source_id: PipelineId,
    },

    /// Signal that a source has finished emitting to a pipeline.
    ///
    /// The executor tracks how many sources feed each pipeline. When all
    /// sources signal end-of-stream, the pipeline can be gracefully stopped
    /// after processing all pending buffers.
    EndOfStream {
        /// ID of the source that finished emitting
        source_id: PipelineId,
        /// ID of the pipeline that will receive no more buffers from this source
        pipeline_id: PipelineId,
    },

    /// Stop a specific pipeline and flush its buffers.
    ///
    /// This task triggers cascading shutdown for a single pipeline:
    /// 1. Calls flush() to emit final buffers
    /// 2. Routes flushed buffers to successors
    /// 3. Calls teardown() to clean up resources
    /// 4. Enqueues EndOfStream to all successors
    StopPipelineTask {
        /// ID of the pipeline to stop
        pipeline_id: PipelineId,
    },

    /// Report a source error to the executor.
    ///
    /// When a source encounters an error (e.g., C++ source throws during next_buffer),
    /// it enqueues this task. The executor records the error and terminates execution.
    SourceError {
        /// ID of the source that encountered the error
        source_id: PipelineId,
        /// Error message
        error: String,
    },

    /// Shutdown the execution engine.
    ///
    /// Signals the execution thread to stop processing tasks and return.
    Shutdown,
}
