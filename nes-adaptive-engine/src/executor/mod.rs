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

//! Task-driven execution engine for stream processing pipelines.
//!
//! The executor provides one or more execution threads that process tasks from
//! a thread-safe queue, orchestrating buffer flow through a dynamic pipeline DAG.
//!
//! # Threading Model
//!
//! - **Execution threads:** N threads run the worker loop in parallel, popping
//!   tasks from the shared queue
//! - **Source threads:** Multiple threads submit buffers via `ExecutorHandle::emit()`
//! - **Deployment threads:** External threads deploy graphs via `ExecutorHandle::deploy_graph()`
//! - **Synchronization:** Mutex for task queue, RwLock for graph, Atomic for reference counting,
//!   Condvar for efficient task notification
//!
//! # Architecture
//!
//! The executor operates on a **single `PipelineGraph`** shared across all workers.
//! Query-level concepts (submit_query, stop_query) are handled at the `Engine` layer
//! above, which maps query IDs to pipeline IDs. The executor only knows about
//! pipelines and their graph topology.
//!
//! # Lifecycle
//!
//! ```no_run
//! # use adaptive_engine::Executor;
//! # use adaptive_engine::graph::PipelineGraph;
//! # use adaptive_engine::pipeline::{Buffer, PipelineId};
//! # use std::thread;
//! let executor = Executor::new();
//! let handle = executor.get_handle();
//!
//! // Spawn execution thread
//! thread::spawn(move || {
//!     executor.run()
//! });
//!
//! // Build and deploy graph - pipelines auto-start
//! let graph = PipelineGraph::new();
//! handle.deploy_graph(graph).unwrap();
//!
//! // Emit buffers - pipelines already started
//! handle.emit(PipelineId::new("source"), Buffer::new(vec![1, 2, 3])).unwrap();
//!
//! // Shutdown - pipelines auto-stop gracefully
//! handle.shutdown().unwrap();
//! ```
//!
//! # Error Handling
//!
//! The executor implements per-pipeline error handling:
//!
//! - **Per-pipeline isolation**: A failed pipeline is marked as failed and skips future buffers,
//!   but other pipelines continue processing normally
//! - **Error propagation via events**: The executor emits `PipelineExecutionError` events that
//!   the query layer (QueryEngine) uses to initiate per-query termination
//! - **Error collection**: All errors are captured in `ExecutionStats.errors` with full context
//! - **Thread safety**: Per-pipeline failed flag uses atomic bool for lock-free checking
//!
//! When an error occurs:
//! 1. Error is recorded with entity ID, type, and task context
//! 2. Pipeline is marked as failed (per-pipeline atomic flag)
//! 3. `PipelineExecutionError` event is emitted for the query layer
//! 4. Future buffers for the failed pipeline are skipped
//! 5. Other pipelines in the graph continue processing normally
//!
//! Example:
//! ```no_run
//! # use adaptive_engine::Executor;
//! # use adaptive_engine::graph::PipelineGraph;
//! let executor = Executor::new();
//! let handle = executor.get_handle();
//!
//! // ... deploy graph, emit buffers ...
//! # handle.deploy_graph(PipelineGraph::new()).unwrap();
//! # handle.shutdown().unwrap();
//!
//! let stats = executor.run();
//!
//! if stats.has_errors() {
//!     eprintln!("Execution failed: {:?}", stats.first_error());
//!     for error in &stats.errors {
//!         eprintln!("  - {}: {}", error.entity_id, error.error);
//!     }
//! }
//! ```

pub mod context;
pub mod delayed;
pub mod error;
pub mod metadata;
pub mod queue;
pub mod stats;
pub mod task;

// Re-export public types
pub use context::{ExecutorContext, PipelineExecutionContext};
pub use delayed::{DelayedMessage, DelayedTaskSubmitter, DelayedTaskSubmitterHandle};
pub use error::{ExecutionStats, ExecutorError};
pub use queue::{FifoQueue, LifoQueue, PriorityQueue, RandomQueue, TaskQueue};
pub use stats::{StatisticsEvent, StatisticsSender, TaskId, WorkerId};

/// Unique identifier for a submitted query.
///
/// Used at the Engine/FFI layer to track queries. The executor itself
/// does not use QueryId — it operates on a single PipelineGraph.
pub type QueryId = u64;
use crate::graph::{PipelineGraph, PipelineNode};
use crate::pipeline::{Buffer, PipelineId};
use error::{EntityType, ExecutionError, TaskType};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::time::Duration;
use task::Task;

/// Thread-safe error state tracking.
///
/// Maintains a lock-free error flag for fast checking in the hot path,
/// plus a mutex-protected collection of all errors for detailed reporting.
pub struct ErrorState {
    /// Flag indicating if any error has occurred (lock-free check).
    has_error: AtomicBool,

    /// Collection of all errors encountered.
    errors: Mutex<Vec<ExecutionError>>,
}

impl ErrorState {
    /// Create a new error state with no errors.
    pub fn new() -> Self {
        Self {
            has_error: AtomicBool::new(false),
            errors: Mutex::new(Vec::new()),
        }
    }

    /// Record an error.
    ///
    /// Sets the atomic error flag (visible to all threads) and appends
    /// the error details to the collection.
    pub fn record_error(&self, error: ExecutionError) {
        // Set flag first (atomic, visible to all threads)
        self.has_error.store(true, Ordering::SeqCst);

        // Then append error details
        self.errors.lock().unwrap().push(error);
    }

    /// Check if any error has occurred.
    ///
    /// Uses atomic load for lock-free checking in the hot path.
    pub fn has_error(&self) -> bool {
        self.has_error.load(Ordering::SeqCst)
    }

    /// Get a copy of all errors.
    pub fn get_errors(&self) -> Vec<ExecutionError> {
        self.errors.lock().unwrap().clone()
    }
}

impl Default for ErrorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe handle for submitting tasks to the executor.
///
/// `ExecutorHandle` can be cloned and shared across threads, providing
/// a thread-safe interface for submitting work tasks, deploying graphs,
/// and controlling pipeline lifecycle.
#[derive(Clone)]
pub struct ExecutorHandle {
    task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
    /// Single pipeline graph shared by all workers.
    graph: Arc<RwLock<PipelineGraph>>,
    /// Error state for the executor (used by commit() in Phase 2).
    #[allow(dead_code)]
    error_state: Arc<ErrorState>,
    /// Flag set by shutdown() - executor thread pushes Shutdown task when last pipeline stops.
    shutting_down: Arc<AtomicBool>,
    /// Condvar to wake worker threads when new tasks are available.
    task_available: Arc<Condvar>,
}

impl ExecutorHandle {
    /// Submit a buffer for execution by a pipeline.
    ///
    /// This is the primary method for sources to submit data for processing.
    /// The buffer will be enqueued as a work task and processed by the
    /// execution thread.
    ///
    /// # Arguments
    ///
    /// * `pipeline_id` - ID of the pipeline to execute
    /// * `buffer` - Buffer to process
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the queue lock cannot be acquired.
    pub fn emit(&self, pipeline_id: PipelineId, buffer: Buffer) -> Result<(), ExecutorError> {
        // Look up the pipeline node from the graph, increment pending, and get a Weak ref
        let node_weak = {
            let graph = self
                .graph
                .read()
                .map_err(|e| ExecutorError::TaskQueue(format!("Graph lock poisoned: {}", e)))?;

            if let Some(node) = graph.get_node(&pipeline_id) {
                node.metadata().increment_pending();
                Arc::downgrade(node)
            } else {
                // Pipeline not found - still enqueue with an empty Weak so the executor
                // can handle the error at dequeue time
                Weak::new()
            }
        };

        // Enqueue work task
        {
            let mut queue = self
                .task_queue
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

            queue.push(Task::WorkTask {
                pipeline_id,
                node: node_weak,
                buffer,
            });
        }

        self.task_available.notify_one();
        Ok(())
    }

    /// Deploy a new pipeline graph.
    ///
    /// Enqueues a DeployGraph task that will merge the new graph's pipelines
    /// into the executor's single graph. This operation is processed by the
    /// execution thread.
    ///
    /// # Arguments
    ///
    /// * `graph` - The pipeline graph to deploy
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the queue lock cannot be acquired.
    pub fn deploy_graph(&self, graph: PipelineGraph) -> Result<(), ExecutorError> {
        {
            let mut queue = self
                .task_queue
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

            queue.push(Task::DeployGraph { graph });
        }

        self.task_available.notify_all();
        Ok(())
    }

    /// Signal end-of-stream for a source → pipeline connection.
    ///
    /// Indicates that this source will emit no more buffers to the specified
    /// pipeline. When all sources feeding a pipeline signal EOS, the pipeline
    /// will be gracefully stopped after processing all pending buffers.
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the source that finished emitting
    /// * `pipeline_id` - ID of the pipeline that will receive no more buffers
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the queue lock cannot be acquired.
    pub fn end_of_stream(
        &self,
        source_id: PipelineId,
        pipeline_id: PipelineId,
    ) -> Result<(), ExecutorError> {
        {
            let mut queue = self
                .task_queue
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

            queue.push(Task::EndOfStream {
                source_id,
                pipeline_id,
            });
        }

        self.task_available.notify_one();
        Ok(())
    }

    /// Stop specific pipelines by enqueuing StopPipelineTask for each source.
    ///
    /// This initiates cascading shutdown through the DAG starting from the given
    /// source pipeline IDs. Used by the Engine layer to implement stop_query().
    ///
    /// # Arguments
    ///
    /// * `source_ids` - IDs of the source pipelines to stop
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the locks are poisoned.
    pub fn stop_pipelines(&self, source_ids: &[PipelineId]) -> Result<(), ExecutorError> {
        // Set source stop flags so source threads terminate
        {
            let graph = self
                .graph
                .read()
                .map_err(|e| ExecutorError::TaskQueue(format!("Graph lock poisoned: {}", e)))?;

            for source_id in source_ids {
                if let Some(node) = graph.get_node(source_id) {
                    node.metadata().request_source_stop();
                }
            }
        }

        // Enqueue StopPipelineTask for each source
        {
            let mut queue = self
                .task_queue
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

            for source_id in source_ids {
                queue.push(Task::StopPipelineTask {
                    pipeline_id: source_id.clone(),
                });
            }
        }

        self.task_available.notify_all();
        Ok(())
    }

    /// Shutdown the executor and stop all active pipelines gracefully.
    ///
    /// This method initiates cascading shutdown by stopping source pipelines first.
    /// Each pipeline flushes buffers before teardown, with data propagating through
    /// the DAG to downstream pipelines.
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the task queue lock is poisoned.
    pub fn shutdown(&self) -> Result<(), ExecutorError> {
        self.shutting_down.store(true, Ordering::SeqCst);

        // Collect sources from the graph
        let all_sources: Vec<PipelineId> = {
            let graph = self
                .graph
                .read()
                .map_err(|e| ExecutorError::TaskQueue(format!("Graph lock poisoned: {}", e)))?;

            graph.get_sources()
        };

        {
            let mut queue = self
                .task_queue
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

            if all_sources.is_empty() {
                // No active pipelines - enqueue Shutdown immediately
                queue.push(Task::Shutdown);
            } else {
                // Enqueue StopPipelineTask for each source
                // Shutdown will be enqueued when the last pipeline stops
                for source_id in all_sources {
                    queue.push(Task::StopPipelineTask {
                        pipeline_id: source_id,
                    });
                }
            }
        }

        self.task_available.notify_all();
        Ok(())
    }

    /// Terminate pipelines for a failed query.
    ///
    /// Marks all `pipeline_ids` as failed (atomic, lock-free for the hot path)
    /// and initiates shutdown of `source_ids` by setting stop flags and
    /// enqueuing `StopPipelineTask` for each source.
    ///
    /// This is the query engine's single entry point for error-driven
    /// termination, replacing direct graph access.
    pub fn terminate_pipelines(
        &self,
        pipeline_ids: &[PipelineId],
        source_ids: &[PipelineId],
    ) -> Result<(), ExecutorError> {
        // 1. Mark all pipelines as failed (atomic flag, no queue interaction)
        {
            let graph = self
                .graph
                .read()
                .map_err(|e| ExecutorError::TaskQueue(format!("Graph lock poisoned: {}", e)))?;

            for pid in pipeline_ids {
                if let Some(node) = graph.get_node(pid) {
                    node.metadata().mark_failed();
                }
            }

            // 2. Set source stop flags so source threads terminate
            for source_id in source_ids {
                if let Some(node) = graph.get_node(source_id) {
                    node.metadata().request_source_stop();
                }
            }
        }

        // 3. Enqueue StopPipelineTask for each source
        if !source_ids.is_empty() {
            let mut queue = self
                .task_queue
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

            for source_id in source_ids {
                queue.push(task::Task::StopPipelineTask {
                    pipeline_id: source_id.clone(),
                });
            }

            self.task_available.notify_all();
        }

        Ok(())
    }

    /// Get a read lock on the graph. Used by Engine layer for query tracking.
    pub fn graph(&self) -> &Arc<RwLock<PipelineGraph>> {
        &self.graph
    }
}

/// Shared state accessible by all worker threads.
///
/// Contains all `Arc`-wrapped state and all task-execution methods.
/// Methods take `&self` and a `worker_id` parameter.
pub(crate) struct ExecutorShared {
    /// Single pipeline graph shared by all workers.
    graph: Arc<RwLock<PipelineGraph>>,
    /// Error state for the executor.
    error_state: Arc<ErrorState>,
    task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
    /// Handle for the delayed task submitter (for repeat_task re-queueing)
    delayed_submitter_handle: Option<DelayedTaskSubmitterHandle>,
    /// Statistics event sender for observability
    stats_sender: stats::StatisticsSender,
    /// Counter for generating unique task IDs
    next_task_id: AtomicU64,
    /// Flag set by shutdown() - executor thread pushes Shutdown task when graph is empty.
    shutting_down: Arc<AtomicBool>,
    /// Condvar to wake worker threads when new tasks are available.
    task_available: Arc<Condvar>,
    /// Number of worker threads.
    worker_count: usize,
    /// Flag set when a worker processes the Shutdown task. Other workers
    /// use this to exit their loops.
    shutdown_complete: AtomicBool,
}

impl ExecutorShared {
    /// Pop a task from the queue, blocking on the condvar if empty.
    ///
    /// Returns `None` only when `shutdown_complete` is set and the queue is empty.
    fn pop_task(&self) -> Option<Task> {
        let mut queue = self.task_queue.lock().unwrap();
        loop {
            if let Some(task) = queue.pop() {
                return Some(task);
            }
            // If shutdown has been fully processed by another worker, exit
            if self.shutdown_complete.load(Ordering::SeqCst) {
                return None;
            }
            let (guard, _timeout) = self
                .task_available
                .wait_timeout(queue, Duration::from_millis(100))
                .unwrap();
            queue = guard;
        }
    }

    /// Execute a work task (pipeline execution with buffer).
    fn execute_work_task(
        &self,
        worker_id: usize,
        pipeline_id: &PipelineId,
        node: Arc<PipelineNode>,
        buffer: Buffer,
        stats: &mut ExecutionStats,
    ) {
        // Check if pipeline has been marked as failed — skip if so
        if node.metadata().is_failed() {
            stats.tasks_skipped += 1;
            self.decrement_ref_and_check_stop(&node, pipeline_id, stats);
            return;
        }

        // Check that setup succeeded (direct atomic read, no lock)
        if !node.metadata().is_setup_succeeded() {
            // Pipeline hasn't completed setup - skip this task
            stats.tasks_skipped += 1;
            return;
        }

        // Read-lock the graph
        let graph_guard = self.graph.read().unwrap();

        // Check if this is a source node
        if graph_guard.is_source(pipeline_id) {
            // Sources don't execute - just route buffers directly to successors
            stats.buffers_processed += 1;
            self.route_buffers(pipeline_id, vec![buffer], &graph_guard);
            return;
        }

        // Get the pipeline from the node (direct deref, no graph lookup)
        let pipeline = node.pipeline();

        // Create channel for context-emitted buffers
        let (emit_tx, emit_rx) = channel();

        // Create execution context
        let context = context::ExecutorContext::new(
            pipeline_id.clone(),
            worker_id,
            self.worker_count,
            emit_tx,
        );

        // Generate unique task ID for statistics tracking
        let task_id = self.next_task_id.fetch_add(1, Ordering::SeqCst);

        // Emit TaskExecutionStart event
        self.stats_sender.task_execution_start(
            worker_id as u64,
            0, // query_id placeholder
            pipeline_id.clone(),
            task_id,
        );

        // Execute the pipeline with context
        match pipeline.execute(buffer, &context) {
            Ok(returned_buffers) => {
                stats.buffers_processed += 1;

                // Emit TaskExecutionComplete event
                self.stats_sender.task_execution_complete(
                    worker_id as u64,
                    0,
                    pipeline_id.clone(),
                    task_id,
                );

                // Check if repeat_task stored a buffer during execution
                if let Some(repeat_buffer) = context.take_repeat_buffer() {
                    let delay_ms = context.get_repeat_delay_ms();

                    // Increment pending counter before enqueueing the repeat task
                    node.metadata().increment_pending();

                    // Re-enqueue the buffer as-is (no copy, no modification)
                    let repeat_task = Task::WorkTask {
                        pipeline_id: pipeline_id.clone(),
                        node: Arc::downgrade(&node),
                        buffer: repeat_buffer,
                    };

                    if delay_ms == 0 {
                        // Immediate re-queue
                        {
                            let mut queue = self.task_queue.lock().unwrap();
                            queue.push(repeat_task);
                        }
                        self.task_available.notify_one();
                    } else {
                        // Delayed re-queue via DelayedTaskSubmitter
                        if let Some(ref handle) = self.delayed_submitter_handle {
                            let _ = handle.submit_delayed(repeat_task, delay_ms);
                        }
                    }
                }

                // Collect emitted buffers from context
                let mut emitted_buffers = Vec::new();
                while let Ok((_pid, buf)) = emit_rx.try_recv() {
                    emitted_buffers.push(buf);
                }

                // Combine returned and emitted buffers
                let all_buffers: Vec<Buffer> = returned_buffers
                    .into_iter()
                    .chain(emitted_buffers)
                    .collect();

                // Route all output buffers to successors
                if !all_buffers.is_empty() {
                    self.route_buffers_with_stats(pipeline_id, all_buffers, &graph_guard, task_id);
                }
            }
            Err(e) => {
                // Emit TaskExecutionComplete event (even on failure)
                self.stats_sender.task_execution_complete(
                    worker_id as u64,
                    0,
                    pipeline_id.clone(),
                    task_id,
                );

                // Record error
                self.error_state.record_error(ExecutionError {
                    entity_id: pipeline_id.clone(),
                    entity_type: EntityType::Pipeline,
                    error: e.to_string(),
                    task_type: TaskType::WorkTask,
                });

                eprintln!("Pipeline {} failed during execution: {}", pipeline_id, e);

                stats.errors_encountered += 1;

                // Mark the pipeline as failed so future buffers are skipped
                node.metadata().mark_failed();

                // Emit error event — the query layer will handle stopping the query
                self.stats_sender.pipeline_execution_error(
                    worker_id as u64,
                    pipeline_id.clone(),
                    e.to_string(),
                    EntityType::Pipeline,
                    TaskType::WorkTask,
                );

                // Still decrement pending counter for proper lifecycle tracking
                drop(graph_guard);
                self.decrement_ref_and_check_stop(&node, pipeline_id, stats);
                return;
            }
        }

        // Always decrement reference count after execution (success path only)
        drop(graph_guard);
        self.decrement_ref_and_check_stop(&node, pipeline_id, stats);
    }

    /// Route output buffers to successor pipelines.
    fn route_buffers(&self, source_id: &PipelineId, buffers: Vec<Buffer>, graph: &PipelineGraph) {
        self.route_buffers_internal(source_id, buffers, graph, None);
    }

    /// Route output buffers to successor pipelines with statistics tracking.
    fn route_buffers_with_stats(
        &self,
        source_id: &PipelineId,
        buffers: Vec<Buffer>,
        graph: &PipelineGraph,
        task_id: stats::TaskId,
    ) {
        self.route_buffers_internal(source_id, buffers, graph, Some(task_id));
    }

    /// Internal buffer routing with optional statistics.
    fn route_buffers_internal(
        &self,
        source_id: &PipelineId,
        buffers: Vec<Buffer>,
        graph: &PipelineGraph,
        stats_task_id: Option<stats::TaskId>,
    ) {
        let successors = graph.get_successors(source_id);

        // Handle edge cases
        if successors.is_empty() {
            // Filter case: no successors, buffers are dropped
            return;
        }

        // Route each buffer to all successors
        for buffer in buffers {
            for successor_id in successors.iter() {
                let buffer_to_send = buffer.clone();

                // Emit TaskEmit event if statistics task_id is provided
                if let Some(task_id) = stats_task_id {
                    self.stats_sender.task_emit(
                        0,
                        0, // query_id placeholder
                        source_id.clone(),
                        successor_id.clone(),
                        task_id,
                    );
                }

                if let Some(successor_node) = graph.get_node(successor_id) {
                    self.enqueue_work_task(successor_id.clone(), successor_node, buffer_to_send);
                }
            }
        }
    }

    /// Enqueue a work task for a pipeline.
    fn enqueue_work_task(&self, pipeline_id: PipelineId, node: &Arc<PipelineNode>, buffer: Buffer) {
        // Increment pending task counter (direct atomic, no lock)
        node.metadata().increment_pending();

        // Enqueue the task
        {
            let mut queue = self.task_queue.lock().unwrap();
            queue.push(Task::WorkTask {
                pipeline_id,
                node: Arc::downgrade(node),
                buffer,
            });
        }
        self.task_available.notify_one();
    }

    /// Execute a deploy graph task with automatic pipeline initialization.
    fn execute_deploy_graph(
        &self,
        worker_id: usize,
        graph: PipelineGraph,
        stats: &mut ExecutionStats,
    ) {
        // Get pipeline IDs from the new graph before moving it
        let pipeline_ids = graph.get_all_pipeline_ids();

        // Merge the new graph into the executor's single graph
        {
            let mut main_graph = self.graph.write().unwrap();
            main_graph.merge(graph);
        }

        // Auto-start all pipelines in the new graph
        let mut setup_failed = false;
        let mut successfully_setup: Vec<PipelineId> = Vec::new();

        {
            let graph_guard = self.graph.read().unwrap();

            for pipeline_id in &pipeline_ids {
                // Skip if pipeline wasn't actually added (e.g. duplicate)
                if graph_guard.get_node(pipeline_id).is_none() {
                    continue;
                }

                // Configure expected sources on the node's metadata
                let expected_sources = graph_guard.count_predecessors(pipeline_id);
                if let Some(node) = graph_guard.get_node(pipeline_id) {
                    node.metadata().set_expected_sources(expected_sources);
                }

                // Call setup() on the pipeline
                if let Some(pipeline) = graph_guard.get_pipeline(pipeline_id) {
                    // Create context for setup
                    let (emit_tx, _emit_rx) = channel();
                    let context = context::ExecutorContext::new(
                        pipeline_id.clone(),
                        worker_id,
                        self.worker_count,
                        emit_tx,
                    );

                    match pipeline.setup(&context) {
                        Ok(()) => {
                            if let Some(node) = graph_guard.get_node(pipeline_id) {
                                node.metadata().mark_setup_succeeded();
                            }
                            successfully_setup.push(pipeline_id.clone());
                            stats.pipelines_started += 1;

                            // Emit PipelineStart event
                            self.stats_sender.pipeline_start(
                                worker_id as u64,
                                0,
                                pipeline_id.clone(),
                            );
                        }
                        Err(e) => {
                            // Record setup failure
                            self.error_state.record_error(ExecutionError {
                                entity_id: pipeline_id.clone(),
                                entity_type: EntityType::Pipeline,
                                error: e.to_string(),
                                task_type: TaskType::DeployGraph,
                            });

                            eprintln!("FATAL ERROR: Pipeline {} setup failed: {}", pipeline_id, e);
                            eprintln!("Terminating execution immediately");

                            // Emit error event so the query layer is notified
                            self.stats_sender.pipeline_execution_error(
                                worker_id as u64,
                                pipeline_id.clone(),
                                e.to_string(),
                                EntityType::Pipeline,
                                TaskType::DeployGraph,
                            );

                            stats.errors_encountered += 1;
                            setup_failed = true;
                            break;
                        }
                    }
                }
            }

            // Only enqueue StartSource tasks if no setup failed
            if !setup_failed {
                let source_ids: Vec<PipelineId> = pipeline_ids
                    .iter()
                    .filter(|id| graph_guard.is_source(id))
                    .cloned()
                    .collect();

                for source_id in source_ids {
                    let task = Task::StartSource { source_id };
                    self.task_queue.lock().unwrap().push(task);
                }
                self.task_available.notify_all();
            }
        }

        // If setup failed, teardown already-setup pipelines and remove the deployment
        if setup_failed {
            // Teardown pipelines that were successfully set up (in reverse order)
            {
                let graph_guard = self.graph.read().unwrap();
                for pid in successfully_setup.iter().rev() {
                    if let Some(pipeline) = graph_guard.get_pipeline(pid) {
                        let (emit_tx, _emit_rx) = channel();
                        let context = context::ExecutorContext::new(
                            pid.clone(),
                            worker_id,
                            self.worker_count,
                            emit_tx,
                        );
                        if let Err(e) = pipeline.teardown(&context) {
                            eprintln!("Error during teardown for {}: {}", pid, e);
                        }
                    }
                }

                // Also teardown source nodes that weren't set up yet (HashMap
                // iteration order is non-deterministic, so a source might not
                // have been reached before the failure).
                for pid in &pipeline_ids {
                    if !successfully_setup.contains(pid) && graph_guard.is_source(pid) {
                        if let Some(source_pipeline) = graph_guard.get_source_pipeline(pid) {
                            let _ = source_pipeline.source().teardown();
                        }
                    }
                }
            }

            // Remove all pipelines from the failed deployment
            {
                let mut graph_guard = self.graph.write().unwrap();
                for pid in &pipeline_ids {
                    if graph_guard.get_node(pid).is_some() {
                        graph_guard.remove_node(pid);
                    }
                }
            }
            stats.pipelines_stopped += pipeline_ids.len();

            // Emit PipelineStop for each removed pipeline so QueryEngine tracks termination
            for pid in &pipeline_ids {
                self.stats_sender
                    .pipeline_stop(worker_id as u64, 0, pid.clone());
            }

            // Check if graph is empty for shutdown
            let graph_empty = {
                let graph_guard = self.graph.read().unwrap();
                graph_guard.is_empty()
            };
            if graph_empty && self.shutting_down.load(Ordering::SeqCst) {
                self.task_queue.lock().unwrap().push(Task::Shutdown);
                self.task_available.notify_all();
            }
        }

        stats.graphs_deployed += 1;
    }

    /// Execute a stop pipeline task.
    fn execute_stop_task(
        &self,
        worker_id: usize,
        pipeline_id: &PipelineId,
        stats: &mut ExecutionStats,
    ) {
        // 1. Get successors and check setup status BEFORE teardown
        let (successors, should_teardown) = {
            let graph_guard = self.graph.read().unwrap();

            if graph_guard.get_pipeline(pipeline_id).is_none() {
                return;
            }

            let successors = graph_guard.get_successors(pipeline_id).to_vec();
            let should_teardown = graph_guard
                .get_node(pipeline_id)
                .map(|node| node.metadata().is_setup_succeeded())
                .unwrap_or(false);
            (successors, should_teardown)
        };

        // Check if pipeline has been marked as failed — skip flush/teardown but still cascade
        let is_failed = {
            let graph_guard = self.graph.read().unwrap();
            graph_guard
                .get_node(pipeline_id)
                .map(|node| node.metadata().is_failed())
                .unwrap_or(false)
        };

        if !should_teardown || is_failed {
            // Skip flush/teardown — just stop source if applicable, remove, and cascade

            // If this is a source, stop it and call teardown to clean up resources
            if is_failed {
                let graph_guard = self.graph.read().unwrap();
                if graph_guard.is_source(pipeline_id) {
                    if let Some(node) = graph_guard.get_node(pipeline_id) {
                        node.metadata().request_source_stop();
                    }
                    if let Some(source_pipeline) = graph_guard.get_source_pipeline(pipeline_id) {
                        let _ = source_pipeline.stop_source();
                        let _ = source_pipeline.source().teardown();
                    }
                }
            }

            // Emit PipelineStop event
            self.stats_sender
                .pipeline_stop(worker_id as u64, 0, pipeline_id.clone());

            // Enqueue EndOfStream to all successors (cascade the stop)
            {
                let mut queue = self.task_queue.lock().unwrap();
                for successor_id in successors {
                    queue.push(Task::EndOfStream {
                        source_id: pipeline_id.clone(),
                        pipeline_id: successor_id,
                    });
                }
            }
            self.task_available.notify_all();

            // Remove the node from the graph
            {
                let mut graph_guard = self.graph.write().unwrap();
                graph_guard.remove_node(pipeline_id);
            }
            stats.pipelines_stopped += 1;

            // Check if graph is empty for shutdown
            let graph_empty = {
                let graph_guard = self.graph.read().unwrap();
                graph_guard.is_empty()
            };
            if graph_empty && self.shutting_down.load(Ordering::SeqCst) {
                self.task_queue.lock().unwrap().push(Task::Shutdown);
                self.task_available.notify_all();
            }
            return;
        }

        // 2. Call flush() to get final buffers
        let flush_result: Result<Vec<Buffer>, String> = {
            let graph_guard = self.graph.read().unwrap();

            if let Some(pipeline) = graph_guard.get_pipeline(pipeline_id) {
                let (emit_tx, emit_rx) = channel();
                let context = context::ExecutorContext::new(
                    pipeline_id.clone(),
                    worker_id,
                    self.worker_count,
                    emit_tx,
                );

                match pipeline.flush(&context) {
                    Ok(returned_buffers) => {
                        let mut emitted_buffers = Vec::new();
                        while let Ok((_pid, buf)) = emit_rx.try_recv() {
                            emitted_buffers.push(buf);
                        }
                        Ok(returned_buffers
                            .into_iter()
                            .chain(emitted_buffers)
                            .collect())
                    }
                    Err(e) => Err(e.to_string()),
                }
            } else {
                Ok(vec![])
            }
        };

        // If flush failed, record error but still cascade the stop to successors
        if let Err(e) = &flush_result {
            eprintln!("Error during flush for {}: {}", pipeline_id, e);

            self.error_state.record_error(ExecutionError {
                entity_id: pipeline_id.clone(),
                entity_type: EntityType::Pipeline,
                error: e.clone(),
                task_type: TaskType::StopPipeline,
            });

            stats.errors_encountered += 1;

            // Emit error event for the query layer
            self.stats_sender.pipeline_execution_error(
                worker_id as u64,
                pipeline_id.clone(),
                e.clone(),
                EntityType::Pipeline,
                TaskType::StopPipeline,
            );

            // Emit PipelineStop
            self.stats_sender
                .pipeline_stop(worker_id as u64, 0, pipeline_id.clone());

            // Mark direct successors as failed before cascading EndOfStream.
            // This is a mechanical safety measure: successors of a corrupted
            // stream must skip flush/teardown regardless of whether the
            // QueryEngine's terminate_pipelines() has run yet.
            {
                let graph_guard = self.graph.read().unwrap();
                for successor_id in &successors {
                    if let Some(node) = graph_guard.get_node(successor_id) {
                        node.metadata().mark_failed();
                    }
                }
            }

            // Cascade EndOfStream to successors
            {
                let mut queue = self.task_queue.lock().unwrap();
                for successor_id in successors {
                    queue.push(Task::EndOfStream {
                        source_id: pipeline_id.clone(),
                        pipeline_id: successor_id,
                    });
                }
            }
            self.task_available.notify_all();

            // Remove the node from the graph
            {
                let mut graph_guard = self.graph.write().unwrap();
                graph_guard.remove_node(pipeline_id);
            }
            stats.pipelines_stopped += 1;

            // Check if graph is empty for shutdown
            let graph_empty = {
                let graph_guard = self.graph.read().unwrap();
                graph_guard.is_empty()
            };
            if graph_empty && self.shutting_down.load(Ordering::SeqCst) {
                self.task_queue.lock().unwrap().push(Task::Shutdown);
                self.task_available.notify_all();
            }
            return;
        }

        let flushed_buffers = flush_result.unwrap();

        // 3. Route flushed buffers to successors
        if !flushed_buffers.is_empty() {
            let graph_guard = self.graph.read().unwrap();
            self.route_buffers(pipeline_id, flushed_buffers, &graph_guard);
        }

        // 3.5. If this is a source pipeline, signal its thread to stop before teardown.
        {
            let graph_guard = self.graph.read().unwrap();
            if graph_guard.is_source(pipeline_id) {
                if let Some(node) = graph_guard.get_node(pipeline_id) {
                    node.metadata().request_source_stop();
                }
                if let Some(source_pipeline) = graph_guard.get_source_pipeline(pipeline_id) {
                    let _ = source_pipeline.stop_source();
                }
            }
        }

        // 4. Call teardown()
        let teardown_error_message: Option<String> = {
            let graph_guard = self.graph.read().unwrap();

            if let Some(pipeline) = graph_guard.get_pipeline(pipeline_id) {
                let mut error_msg = None;
                loop {
                    let (emit_tx, _emit_rx) = channel();
                    let context = context::ExecutorContext::new(
                        pipeline_id.clone(),
                        worker_id,
                        self.worker_count,
                        emit_tx,
                    );

                    match pipeline.teardown(&context) {
                        Ok(()) => {
                            if context.take_repeat_buffer().is_some() {
                                continue;
                            }
                            break;
                        }
                        Err(e) => {
                            eprintln!("Pipeline {} teardown failed: {}", pipeline_id, e);

                            let msg = e.to_string();
                            self.error_state.record_error(ExecutionError {
                                entity_id: pipeline_id.clone(),
                                entity_type: EntityType::Pipeline,
                                error: msg.clone(),
                                task_type: TaskType::StopPipeline,
                            });

                            stats.errors_encountered += 1;
                            error_msg = Some(msg);
                            break;
                        }
                    }
                }
                error_msg
            } else {
                None
            }
        };

        if let Some(ref teardown_error_msg) = teardown_error_message {
            // Emit error event for the query layer
            self.stats_sender.pipeline_execution_error(
                worker_id as u64,
                pipeline_id.clone(),
                teardown_error_msg.clone(),
                EntityType::Pipeline,
                TaskType::StopPipeline,
            );

            // Mark direct successors as failed before cascading EndOfStream.
            // This is a mechanical safety measure: successors of a corrupted
            // stream must skip flush/teardown regardless of whether the
            // QueryEngine's terminate_pipelines() has run yet.
            {
                let graph_guard = self.graph.read().unwrap();
                for successor_id in &successors {
                    if let Some(node) = graph_guard.get_node(successor_id) {
                        node.metadata().mark_failed();
                    }
                }
            }
        }

        // 5. Emit PipelineStop event
        self.stats_sender
            .pipeline_stop(worker_id as u64, 0, pipeline_id.clone());

        // 6. Enqueue EndOfStream to all successors
        {
            let mut queue = self.task_queue.lock().unwrap();
            for successor_id in successors {
                queue.push(Task::EndOfStream {
                    source_id: pipeline_id.clone(),
                    pipeline_id: successor_id,
                });
            }
        }
        self.task_available.notify_all();

        // 7. Remove the node from the graph
        {
            let mut graph_guard = self.graph.write().unwrap();
            graph_guard.remove_node(pipeline_id);
        }
        stats.pipelines_stopped += 1;

        // 8. Check if graph is empty for shutdown
        let graph_empty = {
            let graph_guard = self.graph.read().unwrap();
            graph_guard.is_empty()
        };

        if graph_empty {
            // If engine is shutting down and graph is empty, push Shutdown
            if self.shutting_down.load(Ordering::SeqCst) {
                self.task_queue.lock().unwrap().push(Task::Shutdown);
                self.task_available.notify_all();
            }
        }
    }

    /// Execute a start source task.
    fn execute_start_source_task(
        &self,
        worker_id: usize,
        source_id: &PipelineId,
        stats: &mut ExecutionStats,
    ) {
        use crate::source::SourceEmitHandle;

        let graph_guard = self.graph.read().unwrap();

        let Some(source_pipeline) = graph_guard.get_source_pipeline(source_id) else {
            eprintln!("Source {} not found in graph", source_id);
            stats.errors_encountered += 1;
            return;
        };

        // Get the shared stop flag and Weak ref from the node's metadata
        let (stop_flag, node_weak) = {
            if let Some(node) = graph_guard.get_node(source_id) {
                (node.metadata().get_source_stop_flag(), Arc::downgrade(node))
            } else {
                eprintln!("Node for source {} not found", source_id);
                stats.errors_encountered += 1;
                return;
            }
        };

        // Create SourceEmitHandle (no query_id)
        let emit_handle = SourceEmitHandle::new(
            source_id.clone(),
            node_weak,
            self.task_queue.clone(),
            stop_flag,
            Arc::clone(&self.task_available),
        );

        // Start the source
        match source_pipeline.start_source(emit_handle) {
            Ok(()) => {
                if let Some(node) = graph_guard.get_node(source_id) {
                    node.metadata().mark_source_started();
                }

                // Notify QueryEngine that this source has started
                self.stats_sender
                    .source_started(worker_id as u64, source_id.clone());
            }
            Err(e) => {
                // Record error
                self.error_state.record_error(ExecutionError {
                    entity_id: source_id.clone(),
                    entity_type: EntityType::Source,
                    error: e.to_string(),
                    task_type: TaskType::StartSource,
                });

                eprintln!("Source {} failed to start: {}", source_id, e);

                stats.errors_encountered += 1;

                // Mark the source as failed
                if let Some(node) = graph_guard.get_node(source_id) {
                    node.metadata().mark_failed();
                }

                // Emit error event — the query layer will handle stopping the query
                self.stats_sender.pipeline_execution_error(
                    worker_id as u64,
                    source_id.clone(),
                    e.to_string(),
                    EntityType::Source,
                    TaskType::StartSource,
                );
            }
        }
    }

    /// Execute a source error task.
    fn execute_source_error_task(
        &self,
        worker_id: usize,
        source_id: &PipelineId,
        error: &str,
        stats: &mut ExecutionStats,
    ) {
        // Check if the source still exists
        {
            let graph_guard = self.graph.read().unwrap();
            if graph_guard.get_pipeline(source_id).is_none() {
                // Source already terminated
                return;
            }
        }

        // Record the error
        self.error_state.record_error(ExecutionError {
            entity_id: source_id.clone(),
            entity_type: EntityType::Source,
            error: error.to_string(),
            task_type: TaskType::StartSource,
        });

        eprintln!("Source {} failed: {}", source_id, error);

        stats.errors_encountered += 1;

        // Mark the source node as failed
        {
            let graph_guard = self.graph.read().unwrap();
            if let Some(node) = graph_guard.get_node(source_id) {
                node.metadata().mark_failed();
            }
        }

        // Emit error event — the query layer will handle stopping the query
        self.stats_sender.pipeline_execution_error(
            worker_id as u64,
            source_id.clone(),
            error.to_string(),
            EntityType::Source,
            TaskType::StartSource,
        );
    }

    /// Execute an end-of-stream task.
    fn execute_eos_task(
        &self,
        _source_id: &PipelineId,
        pipeline_id: &PipelineId,
        _stats: &mut ExecutionStats,
    ) {
        let should_stop = {
            let graph_guard = self.graph.read().unwrap();
            let node = graph_guard.get_node(pipeline_id);

            if let Some(node) = node {
                let meta = node.metadata();
                let eos_count = meta.increment_eos();
                let expected = meta.get_expected_sources();

                if expected > 0 && eos_count >= expected {
                    meta.request_termination();
                    meta.should_terminate()
                        && meta.get_pending() == 0
                        && !meta.stop_task_enqueued.swap(true, Ordering::SeqCst)
                } else {
                    false
                }
            } else {
                eprintln!(
                    "Warning: End-of-stream for pipeline {} that doesn't exist",
                    pipeline_id
                );
                false
            }
        };

        if should_stop {
            {
                self.task_queue
                    .lock()
                    .unwrap()
                    .push(Task::StopPipelineTask {
                        pipeline_id: pipeline_id.clone(),
                    });
            }
            self.task_available.notify_one();
        }
    }

    /// Decrement reference count and check if pipeline should be stopped.
    fn decrement_ref_and_check_stop(
        &self,
        node: &Arc<PipelineNode>,
        pipeline_id: &PipelineId,
        _stats: &mut ExecutionStats,
    ) {
        let meta = node.metadata();
        let should_stop = {
            meta.decrement_pending();
            meta.should_terminate()
                && meta.get_pending() == 0
                && !meta.stop_task_enqueued.swap(true, Ordering::SeqCst)
        };

        if should_stop {
            {
                self.task_queue
                    .lock()
                    .unwrap()
                    .push(Task::StopPipelineTask {
                        pipeline_id: pipeline_id.clone(),
                    });
            }
            self.task_available.notify_one();
        }
    }

    /// Emergency shutdown - stops all pipelines immediately without cascading.
    fn stop_all_pipelines(&self, stats: &mut ExecutionStats) {
        let graph_guard = self.graph.read().unwrap();
        let pipeline_ids = graph_guard.get_all_pipeline_ids();
        for pipeline_id in &pipeline_ids {
            if let Some(node) = graph_guard.get_node(pipeline_id) {
                if node.metadata().is_setup_succeeded() {
                    let (emit_tx, _emit_rx) = channel();
                    let context = context::ExecutorContext::new(pipeline_id.clone(), 0, 1, emit_tx);

                    if let Err(e) = node.pipeline().teardown(&context) {
                        eprintln!("Error during pipeline teardown for {}: {}", pipeline_id, e);
                        stats.errors_encountered += 1;
                    }
                }
            }
            stats.pipelines_stopped += 1;
        }
    }

    /// Run the worker loop for a single worker thread.
    fn worker_loop(self: &Arc<Self>, worker_id: usize) -> ExecutionStats {
        let mut stats = ExecutionStats::new();

        loop {
            // Check if another worker already processed Shutdown
            if self.shutdown_complete.load(Ordering::SeqCst) {
                break;
            }

            let task = self.pop_task();

            let Some(task) = task else {
                if self.shutdown_complete.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            };

            if matches!(task, Task::Shutdown) {
                stats.tasks_executed += 1;
                self.stop_all_pipelines(&mut stats);
                // Signal other workers to exit
                self.shutdown_complete.store(true, Ordering::SeqCst);
                self.task_available.notify_all();
                break;
            }

            self.dispatch_task(worker_id, task, &mut stats);
        }

        stats
    }

    /// Dispatch a single task to the appropriate handler.
    fn dispatch_task(&self, worker_id: usize, task: Task, stats: &mut ExecutionStats) {
        match task {
            Task::WorkTask {
                pipeline_id,
                node: node_weak,
                buffer,
            } => {
                // Try to upgrade the Weak pointer - if it fails, the pipeline was removed
                let Some(node) = node_weak.upgrade() else {
                    stats.tasks_skipped += 1;
                    return;
                };

                self.execute_work_task(worker_id, &pipeline_id, node, buffer, stats);
                stats.tasks_executed += 1;
            }
            Task::DeployGraph { graph } => {
                self.execute_deploy_graph(worker_id, graph, stats);
                stats.tasks_executed += 1;
            }
            Task::StartSource { source_id } => {
                // Check if pipeline still exists in graph
                {
                    let graph_guard = self.graph.read().unwrap();
                    if graph_guard.get_node(&source_id).is_none() {
                        stats.tasks_skipped += 1;
                        return;
                    }
                }
                self.execute_start_source_task(worker_id, &source_id, stats);
                stats.tasks_executed += 1;
            }
            Task::EndOfStream {
                source_id,
                pipeline_id,
            } => {
                // Check if pipeline still exists in graph
                {
                    let graph_guard = self.graph.read().unwrap();
                    if graph_guard.get_node(&pipeline_id).is_none() {
                        stats.tasks_skipped += 1;
                        return;
                    }
                }
                self.execute_eos_task(&source_id, &pipeline_id, stats);
                stats.tasks_executed += 1;
            }
            Task::StopPipelineTask { pipeline_id } => {
                self.execute_stop_task(worker_id, &pipeline_id, stats);
                stats.tasks_executed += 1;
            }
            Task::SourceError { source_id, error } => {
                self.execute_source_error_task(worker_id, &source_id, &error, stats);
                stats.tasks_executed += 1;
            }
            Task::Shutdown => {
                // Handled in worker_loop before dispatch
                unreachable!("Shutdown should be handled before dispatch_task");
            }
        }
    }
}

/// Execution engine with configurable worker thread count.
///
/// `Executor` manages shared state and worker threads. When `run()` is called,
/// it spawns N-1 additional worker threads (for a total of N workers) that
/// all process tasks from the shared queue.
pub struct Executor {
    /// Shared state accessible by all worker threads.
    shared: Arc<ExecutorShared>,
    /// The delayed task submitter for handling repeat_task() calls (owns thread lifecycle)
    delayed_submitter: Option<DelayedTaskSubmitter>,
    /// Number of worker threads.
    worker_count: usize,
}

impl Executor {
    /// Create a new executor with the default FIFO queue and 1 worker thread.
    pub fn new() -> Self {
        Self::with_queue(FifoQueue::new())
    }

    /// Create a new executor with a custom task queue implementation.
    pub fn with_queue<Q: TaskQueue + 'static>(queue: Q) -> Self {
        Self::with_queue_and_stats(queue, stats::StatisticsSender::noop())
    }

    /// Create a new executor with a custom task queue and statistics sender.
    pub fn with_queue_and_stats<Q: TaskQueue + 'static>(
        queue: Q,
        stats_sender: stats::StatisticsSender,
    ) -> Self {
        Self::with_worker_count_queue_and_stats(1, queue, stats_sender)
    }

    /// Create a new executor with the specified number of worker threads.
    pub fn with_worker_count(worker_count: usize) -> Self {
        Self::with_worker_count_queue_and_stats(
            worker_count,
            FifoQueue::new(),
            stats::StatisticsSender::noop(),
        )
    }

    /// Create a new executor with worker count and statistics.
    pub fn with_worker_count_and_stats(
        worker_count: usize,
        stats_sender: stats::StatisticsSender,
    ) -> Self {
        Self::with_worker_count_queue_and_stats(worker_count, FifoQueue::new(), stats_sender)
    }

    /// Create a new executor with all configuration options.
    pub fn with_worker_count_queue_and_stats<Q: TaskQueue + 'static>(
        worker_count: usize,
        queue: Q,
        stats_sender: stats::StatisticsSender,
    ) -> Self {
        let worker_count = worker_count.max(1);
        let task_queue: Arc<Mutex<Box<dyn TaskQueue>>> = Arc::new(Mutex::new(Box::new(queue)));
        let task_available = Arc::new(Condvar::new());

        // Create the DelayedTaskSubmitter with access to the task queue and condvar
        let delayed_submitter =
            DelayedTaskSubmitter::new(Arc::clone(&task_queue), Arc::clone(&task_available));

        let shared = Arc::new(ExecutorShared {
            graph: Arc::new(RwLock::new(PipelineGraph::new())),
            error_state: Arc::new(ErrorState::new()),
            task_queue,
            delayed_submitter_handle: Some(delayed_submitter.get_handle()),
            stats_sender,
            next_task_id: AtomicU64::new(1),
            shutting_down: Arc::new(AtomicBool::new(false)),
            task_available,
            worker_count,
            shutdown_complete: AtomicBool::new(false),
        });

        Self {
            shared,
            delayed_submitter: Some(delayed_submitter),
            worker_count,
        }
    }

    /// Get a cloneable handle for submitting tasks.
    pub fn get_handle(&self) -> ExecutorHandle {
        ExecutorHandle {
            task_queue: Arc::clone(&self.shared.task_queue),
            graph: Arc::clone(&self.shared.graph),
            error_state: Arc::clone(&self.shared.error_state),
            shutting_down: Arc::clone(&self.shared.shutting_down),
            task_available: Arc::clone(&self.shared.task_available),
        }
    }

    /// Run the execution loop (blocking).
    ///
    /// This method runs on the execution thread and processes tasks until
    /// a `Shutdown` task is received. It spawns N-1 additional worker threads
    /// (for a total of N workers) and returns merged execution statistics
    /// when shutdown is complete.
    pub fn run(mut self) -> ExecutionStats {
        let shared = Arc::clone(&self.shared);

        // Spawn N-1 additional worker threads
        let mut worker_handles = Vec::new();
        for worker_id in 1..self.worker_count {
            let shared_clone = Arc::clone(&shared);
            let handle = std::thread::Builder::new()
                .name(format!("nes-worker-{}", worker_id))
                .spawn(move || shared_clone.worker_loop(worker_id))
                .expect("Failed to spawn worker thread");
            worker_handles.push(handle);
        }

        // Worker 0 runs on the current thread
        let mut stats = shared.worker_loop(0);

        // Join all worker threads and merge their stats
        for handle in worker_handles {
            match handle.join() {
                Ok(worker_stats) => stats.merge(worker_stats),
                Err(e) => {
                    eprintln!("Worker thread panicked: {:?}", e);
                }
            }
        }

        // Shutdown the DelayedTaskSubmitter and wait for its thread to finish
        if let Some(submitter) = self.delayed_submitter.take() {
            submitter.shutdown();
        }

        // Aggregate errors from error state into stats
        stats.errors.extend(shared.error_state.get_errors());

        // Clear graph to release pipeline/source resources.
        {
            let mut graph_guard = shared.graph.write().unwrap();
            *graph_guard = PipelineGraph::new();
        }

        stats
    }

    /// Execute a single task (for testing).
    ///
    /// Pops one task from the queue and executes it. Returns true if a task
    /// was executed, false if the queue was empty.
    pub fn run_one(&mut self) -> bool {
        let task = {
            let mut queue = self.shared.task_queue.lock().unwrap();
            queue.pop()
        };

        let Some(task) = task else {
            return false;
        };

        let mut stats = ExecutionStats::new();

        match task {
            Task::WorkTask {
                pipeline_id,
                node: node_weak,
                buffer,
            } => {
                let Some(node) = node_weak.upgrade() else {
                    return true;
                };

                self.shared
                    .execute_work_task(0, &pipeline_id, node, buffer, &mut stats);
            }
            Task::DeployGraph { graph } => {
                self.shared.execute_deploy_graph(0, graph, &mut stats);
            }
            Task::StartSource { source_id } => {
                // Check if pipeline still exists
                {
                    let graph_guard = self.shared.graph.read().unwrap();
                    if graph_guard.get_node(&source_id).is_none() {
                        return true;
                    }
                }
                self.shared
                    .execute_start_source_task(0, &source_id, &mut stats);
            }
            Task::EndOfStream {
                source_id,
                pipeline_id,
            } => {
                // Check if pipeline still exists
                {
                    let graph_guard = self.shared.graph.read().unwrap();
                    if graph_guard.get_node(&pipeline_id).is_none() {
                        return true;
                    }
                }
                self.shared
                    .execute_eos_task(&source_id, &pipeline_id, &mut stats);
            }
            Task::StopPipelineTask { pipeline_id } => {
                self.shared.execute_stop_task(0, &pipeline_id, &mut stats);
            }
            Task::SourceError { source_id, error } => {
                self.shared
                    .execute_source_error_task(0, &source_id, &error, &mut stats);
            }
            Task::Shutdown => {
                // Stop all active pipelines
                self.shared.stop_all_pipelines(&mut stats);
            }
        }

        true
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}
