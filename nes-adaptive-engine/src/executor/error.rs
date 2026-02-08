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

//! Error types and execution statistics for the executor.

use crate::graph::GraphError;
use crate::pipeline::{PipelineError, PipelineId};
use thiserror::Error;

/// Type of entity where an error occurred.
#[derive(Debug, Clone, Copy)]
pub enum EntityType {
    /// Error occurred in a pipeline.
    Pipeline,
    /// Error occurred in a source.
    Source,
}

/// Type of task being executed when error occurred.
#[derive(Debug, Clone, Copy)]
pub enum TaskType {
    /// Error during WorkTask execution (pipeline processing buffer).
    WorkTask,
    /// Error during source startup.
    StartSource,
    /// Error during graph deployment (pipeline setup).
    DeployGraph,
    /// Error during pipeline stop.
    StopPipeline,
}

/// Detailed information about an execution error.
///
/// Captures the full context of an error including which entity failed,
/// what type of entity it was, what task was being executed, and the
/// error message itself.
#[derive(Debug, Clone)]
pub struct ExecutionError {
    /// Pipeline or source ID where error occurred.
    pub entity_id: PipelineId,

    /// Type of entity (Pipeline or Source).
    pub entity_type: EntityType,

    /// The actual error message.
    pub error: String,

    /// Task type when error occurred.
    pub task_type: TaskType,
}

/// Errors that can occur during executor operations.
#[derive(Error, Debug)]
pub enum ExecutorError {
    /// Pipeline execution error.
    #[error("Pipeline error: {0}")]
    Pipeline(#[from] PipelineError),

    /// Graph operation error.
    #[error("Graph error: {0}")]
    Graph(#[from] GraphError),

    /// Pipeline not found in the graph.
    #[error("Pipeline not found: {0}")]
    PipelineNotFound(PipelineId),

    /// Task queue operation error.
    #[error("Task queue error: {0}")]
    TaskQueue(String),

    /// No graph has been deployed yet.
    #[error("No graph deployed")]
    GraphNotDeployed,
}

/// Statistics tracking execution engine performance.
///
/// Tracks various metrics about task execution, buffer processing,
/// and lifecycle events during executor operation.
#[derive(Debug, Clone, Default)]
pub struct ExecutionStats {
    /// Total number of tasks executed (all types).
    pub tasks_executed: usize,

    /// Total number of buffers processed through pipelines.
    pub buffers_processed: usize,

    /// Number of pipelines started.
    pub pipelines_started: usize,

    /// Number of pipelines stopped.
    pub pipelines_stopped: usize,

    /// Number of graphs deployed.
    pub graphs_deployed: usize,

    /// Number of errors encountered during execution.
    pub errors_encountered: usize,

    /// Number of tasks skipped due to query being stopped/removed.
    ///
    /// When a query is stopped via `stop_query()`, its state is removed from the
    /// HashMap. Tasks for that query that are still in the queue are skipped
    /// when dequeued (filter-on-dequeue pattern). This counter tracks how many
    /// such orphaned tasks were discarded.
    pub tasks_skipped: usize,

    /// Detailed error information for all errors encountered.
    ///
    /// Each error includes the entity ID, entity type, error message,
    /// and task type for complete debugging context.
    pub errors: Vec<ExecutionError>,
}

impl ExecutionStats {
    /// Create a new execution stats tracker with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if any errors were encountered during execution.
    ///
    /// Returns `true` if the errors vector is non-empty.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Get the first error encountered, if any.
    ///
    /// Useful for quickly checking what went wrong without
    /// examining the full error list.
    pub fn first_error(&self) -> Option<&ExecutionError> {
        self.errors.first()
    }

    /// Merge another `ExecutionStats` into this one by summing counters
    /// and extending error lists.
    pub fn merge(&mut self, other: ExecutionStats) {
        self.tasks_executed += other.tasks_executed;
        self.buffers_processed += other.buffers_processed;
        self.pipelines_started += other.pipelines_started;
        self.pipelines_stopped += other.pipelines_stopped;
        self.graphs_deployed += other.graphs_deployed;
        self.errors_encountered += other.errors_encountered;
        self.tasks_skipped += other.tasks_skipped;
        self.errors.extend(other.errors);
    }
}
