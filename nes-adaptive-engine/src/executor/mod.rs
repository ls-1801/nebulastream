//! Task-driven execution engine for stream processing pipelines.
//!
//! The executor provides a single execution thread that processes tasks from
//! a thread-safe queue, orchestrating buffer flow through a dynamic pipeline DAG.
//!
//! # Threading Model
//!
//! - **Execution thread:** Single thread runs `Executor::run()` in a blocking loop
//! - **Source threads:** Multiple threads submit buffers via `ExecutorHandle::emit()`
//! - **Deployment threads:** External threads deploy graphs via `ExecutorHandle::deploy_graph()`
//! - **Synchronization:** Mutex for task queue, RwLock for graph, Atomic for reference counting
//!
//! # Simplified Lifecycle (v0.2.0+)
//!
//! The executor now automatically manages pipeline lifecycle:
//!
//! ```no_run
//! # use adaptive_engine::Executor;
//! # use adaptive_engine::graph::PipelineGraph;
//! # use adaptive_engine::pipeline::{Buffer, PipelineId};
//! # use adaptive_engine::sequence::SequenceNumber;
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
//! # Legacy Lifecycle (backward compatible)
//!
//! The manual lifecycle methods remain available but are deprecated:
//!
//! 1. **Deploy graph** - `handle.deploy_graph(graph)` replaces the current graph and auto-starts pipelines
//! 2. **Start pipelines** - `handle.start_pipeline(id)` (DEPRECATED - auto-starts on deploy)
//! 3. **Execute buffers** - Sources call `handle.emit(id, buffer)` to process data
//! 4. **Stop pipelines** - `handle.stop_pipeline(id)` (DEPRECATED - use end_of_stream or shutdown)
//! 5. **Shutdown** - `handle.shutdown()` auto-stops all pipelines and stops the execution thread
//!
//! # Lifecycle Invariants
//!
//! The executor enforces critical lifecycle invariants via panics (not errors):
//!
//! - **MUST start before execute**: Attempting to execute a pipeline that was never started panics
//! - **MUST setup before execute**: Attempting to execute a pipeline whose `setup()` failed panics
//! - **MUST teardown after setup**: If `setup()` succeeds, `teardown()` is guaranteed to be called
//!
//! These are programming errors that should be caught during development, not runtime errors.
//!
//! # Error Handling
//!
//! The executor implements fail-fast error handling:
//!
//! - **Immediate termination**: Any pipeline or source error stops query processing immediately
//! - **No graceful shutdown**: On error, pending tasks are skipped without calling flush/teardown
//! - **Error collection**: All errors are captured in `ExecutionStats.errors` with full context
//! - **Thread safety**: Error state uses atomic flag for lock-free checking in hot path
//!
//! When an error occurs:
//! 1. Error is recorded with entity ID, type, and task context
//! 2. Atomic error flag is set (visible to all threads)
//! 3. Main loop detects flag and drains queue without processing
//! 4. Already-started tasks may complete (cannot be interrupted)
//! 5. Already-emitted buffers remain downstream (streaming semantics)
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
//!     eprintln!("Query failed: {:?}", stats.first_error());
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

use crate::ffi::QueryId;
use crate::graph::PipelineGraph;
use crate::pipeline::{Buffer, PipelineId};
use error::{EntityType, ExecutionError, TaskType};
use metadata::PipelineMetadata;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, RwLock};
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
    fn new() -> Self {
        Self {
            has_error: AtomicBool::new(false),
            errors: Mutex::new(Vec::new()),
        }
    }

    /// Record an error.
    ///
    /// Sets the atomic error flag (visible to all threads) and appends
    /// the error details to the collection.
    fn record_error(&self, error: ExecutionError) {
        // Set flag first (atomic, visible to all threads)
        self.has_error.store(true, Ordering::SeqCst);

        // Then append error details
        self.errors.lock().unwrap().push(error);
    }

    /// Check if any error has occurred for this query.
    ///
    /// Uses atomic load for lock-free checking in the hot path.
    /// Used for task filtering - tasks for errored queries are skipped.
    pub fn has_error(&self) -> bool {
        self.has_error.load(Ordering::SeqCst)
    }

    /// Get a copy of all errors.
    fn get_errors(&self) -> Vec<ExecutionError> {
        self.errors.lock().unwrap().clone()
    }
}

/// State for a single query.
///
/// Contains all state associated with a query including its pipeline graph,
/// error state, and source count. This enables multi-query support where
/// multiple queries can run concurrently with isolated state.
pub struct QueryState {
    /// The pipeline graph for this query.
    pub graph: PipelineGraph,

    /// Error state for this query (per-query isolation).
    pub error_state: ErrorState,

    /// Number of active sources for this query.
    pub source_count: usize,

    /// Total number of sources expected for this query (set at deploy time).
    pub expected_sources: std::sync::atomic::AtomicUsize,

    /// Number of sources that have been successfully started.
    pub sources_started: std::sync::atomic::AtomicUsize,

    /// Whether this query is in the process of stopping.
    ///
    /// When `stop_query()` is called, this flag is set to `true` instead of
    /// immediately removing the QueryState. This keeps the PipelineGraph alive
    /// so that cascading StopPipelineTasks can still find the graph to perform
    /// teardown/flush. The QueryState is only removed once all its pipelines
    /// have been stopped.
    pub stopping: AtomicBool,
}

impl QueryState {
    /// Create a new query state with the given graph.
    pub fn new(graph: PipelineGraph) -> Self {
        Self {
            graph,
            error_state: ErrorState::new(),
            source_count: 0,
            expected_sources: std::sync::atomic::AtomicUsize::new(0),
            sources_started: std::sync::atomic::AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
        }
    }

    /// Check if this query is in the process of stopping.
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Mark this query as stopping.
    pub fn mark_stopping(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }
}

/// Counter for generating unique query IDs within the executor.
///
/// This is separate from the FFI QueryId counter to allow internal query
/// management during the transition period. The FFI layer will eventually
/// provide the QueryId when submitting queries.
static NEXT_EXECUTOR_QUERY_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Thread-safe handle for submitting tasks to the executor.
///
/// `ExecutorHandle` can be cloned and shared across threads, providing
/// a thread-safe interface for submitting work tasks, deploying graphs,
/// and controlling pipeline lifecycle.
#[derive(Clone)]
pub struct ExecutorHandle {
    task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
    pub(crate) metadata: Arc<Mutex<HashMap<PipelineId, Arc<PipelineMetadata>>>>,
    /// Multi-query graph storage: maps QueryId to QueryState.
    queries: Arc<RwLock<HashMap<QueryId, QueryState>>>,
    /// Flag set by shutdown() - executor thread pushes Shutdown task when last query completes.
    shutting_down: Arc<AtomicBool>,
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
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::executor::Executor;
    /// # use adaptive_engine::pipeline::{Buffer, PipelineId};
    /// # let mut executor = Executor::new();
    /// # let handle = executor.get_handle();
    /// let buffer = Buffer::new(vec![1, 2, 3]);
    /// handle.emit(PipelineId::new("pipeline1"), buffer).unwrap();
    /// ```
    pub fn emit(&self, pipeline_id: PipelineId, buffer: Buffer) -> Result<(), ExecutorError> {
        // Note: Per-query error checking is done at dequeue time (filter-on-dequeue pattern).
        // This allows sources to continue emitting buffers even after errors, with cleanup
        // happening when tasks are processed. This is necessary because sources don't know
        // their query ID at emit time.

        // Increment pending task counter before enqueueing
        {
            let metadata = self
                .metadata
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Metadata lock poisoned: {}", e)))?;

            if let Some(meta) = metadata.get(&pipeline_id) {
                meta.increment_pending();
            }
            // If metadata doesn't exist, the pipeline hasn't been started yet
            // We'll still enqueue the task and let the executor handle the error
        }

        // Enqueue work task
        let mut queue = self
            .task_queue
            .lock()
            .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

        queue.push(Task::WorkTask {
            query_id: 0, // TODO: US-006 will add proper multi-query tracking
            pipeline_id,
            buffer,
        });

        Ok(())
    }

    /// Deploy a new pipeline graph.
    ///
    /// Replaces the current graph with a new one. This operation is atomic
    /// from the perspective of the execution thread.
    ///
    /// # Arguments
    ///
    /// * `graph` - The new pipeline graph to deploy
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the queue lock cannot be acquired.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::executor::Executor;
    /// # use adaptive_engine::graph::PipelineGraph;
    /// # let mut executor = Executor::new();
    /// # let handle = executor.get_handle();
    /// let graph = PipelineGraph::new();
    /// handle.deploy_graph(graph).unwrap();
    /// ```
    pub fn deploy_graph(&self, graph: PipelineGraph) -> Result<(), ExecutorError> {
        self.deploy_graph_with_query_id(0, graph)
    }

    /// Deploy a graph with a specific query ID.
    ///
    /// This is used by the FFI layer to pass the externally-generated query ID
    /// so that stop_query() can look up queries by the same ID.
    pub fn deploy_graph_with_query_id(
        &self,
        query_id: QueryId,
        graph: PipelineGraph,
    ) -> Result<(), ExecutorError> {
        // Note: Deploying a new graph doesn't check error state.
        // Each query has isolated error state, so a new query can always be deployed
        // even if existing queries have errors.

        let mut queue = self
            .task_queue
            .lock()
            .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

        queue.push(Task::DeployGraph { graph, query_id });
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
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::executor::Executor;
    /// # use adaptive_engine::pipeline::PipelineId;
    /// # let mut executor = Executor::new();
    /// # let handle = executor.get_handle();
    /// let source_id = PipelineId::new("source1");
    /// let pipeline_id = PipelineId::new("pipeline1");
    ///
    /// // After emitting all buffers
    /// handle.end_of_stream(source_id, pipeline_id).unwrap();
    /// ```
    pub fn end_of_stream(
        &self,
        source_id: PipelineId,
        pipeline_id: PipelineId,
    ) -> Result<(), ExecutorError> {
        // Note: Per-query error checking is done at dequeue time (filter-on-dequeue pattern).
        // EOS signals are still enqueued even if query has errors, to ensure proper cleanup.

        let mut queue = self
            .task_queue
            .lock()
            .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

        queue.push(Task::EndOfStream {
            query_id: 0, // TODO: US-006 will add proper multi-query tracking
            source_id,
            pipeline_id,
        });
        Ok(())
    }

    /// Stop a specific query and remove it from the executor.
    ///
    /// This method stops only the specified query, leaving other queries running.
    /// The query's pipelines will be gracefully stopped by enqueueing StopPipelineTask
    /// for each source pipeline, which triggers cascading shutdown through the DAG.
    ///
    /// # Arguments
    ///
    /// * `query_id` - The ID of the query to stop
    ///
    /// # Returns
    ///
    /// Returns `true` if the query was found and stop was initiated, `false` if the
    /// query ID was not found.
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError::TaskQueue` if the locks are poisoned.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::Executor;
    /// # use adaptive_engine::graph::PipelineGraph;
    /// let executor = Executor::new();
    /// let handle = executor.get_handle();
    ///
    /// // Submit a query...
    /// # handle.deploy_graph(PipelineGraph::new()).unwrap();
    ///
    /// // Stop only that query (other queries continue)
    /// let stopped = handle.stop_query(1).unwrap();
    /// ```
    pub fn stop_query(&self, query_id: QueryId) -> Result<bool, ExecutorError> {
        // Take a read lock to check the query and mark it as stopping.
        // We do NOT remove the QueryState yet - it must stay alive so that
        // cascading StopPipelineTasks can find the graph for teardown/flush.
        let queries = self
            .queries
            .read()
            .map_err(|e| ExecutorError::TaskQueue(format!("Queries lock poisoned: {}", e)))?;

        // Check if the query exists
        let Some(query_state) = queries.get(&query_id) else {
            return Ok(false);
        };

        // Mark the query as stopping (prevents new WorkTasks from being processed)
        query_state.mark_stopping();

        // Get the source pipelines
        let source_ids: Vec<PipelineId> = query_state.graph.get_sources();

        // Set source stop flags so CppSourceAdapter threads terminate.
        {
            let metadata = self
                .metadata
                .lock()
                .map_err(|e| ExecutorError::TaskQueue(format!("Metadata lock poisoned: {}", e)))?;

            for source_id in &source_ids {
                if let Some(meta) = metadata.get(source_id) {
                    meta.request_source_stop();
                }
            }
        }

        // Drop the read lock before acquiring the queue lock
        drop(queries);

        // Get task queue lock
        let mut queue = self
            .task_queue
            .lock()
            .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

        // Enqueue StopPipelineTask for each source to initiate cascading shutdown
        for source_id in source_ids {
            queue.push(Task::StopPipelineTask {
                pipeline_id: source_id,
            });
        }

        Ok(true)
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
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::Executor;
    /// # use adaptive_engine::graph::PipelineGraph;
    /// let executor = Executor::new();
    /// let handle = executor.get_handle();
    ///
    /// // Deploy and use...
    /// # handle.deploy_graph(PipelineGraph::new()).unwrap();
    ///
    /// // Shutdown with automatic cleanup
    /// handle.shutdown().unwrap();
    /// ```
    pub fn shutdown(&self) -> Result<(), ExecutorError> {
        self.shutting_down.store(true, Ordering::SeqCst);

        // Collect sources from all active queries
        let all_sources: Vec<PipelineId> = {
            let queries = self
                .queries
                .read()
                .map_err(|e| ExecutorError::TaskQueue(format!("Queries lock poisoned: {}", e)))?;

            queries
                .values()
                .flat_map(|qs| qs.graph.get_sources())
                .collect()
        };

        let mut queue = self
            .task_queue
            .lock()
            .map_err(|e| ExecutorError::TaskQueue(format!("Queue lock poisoned: {}", e)))?;

        if all_sources.is_empty() {
            // No active queries - enqueue Shutdown immediately
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

        Ok(())
    }
}

/// Single-threaded execution engine.
///
/// `Executor` runs on a dedicated thread and processes tasks from a thread-safe
/// queue, orchestrating buffer flow through the pipeline graph.
///
/// # DelayedTaskSubmitter Integration
///
/// The executor owns a `DelayedTaskSubmitter` that handles `repeat_task()` calls.
/// When a pipeline calls `context.repeat_task(delay_ms)`, the task is sent to the
/// submitter thread which sleeps for the delay and then pushes the task back into
/// the executor's task queue. On shutdown, the executor signals the submitter to
/// stop and joins its thread.
///
/// # Statistics Channel
///
/// The executor accepts an optional `StatisticsSender` for emitting execution
/// lifecycle events. If no sender is provided (or a no-op sender is used),
/// events are silently discarded with zero overhead.
pub struct Executor {
    /// Multi-query graph storage: maps QueryId to QueryState.
    queries: Arc<RwLock<HashMap<QueryId, QueryState>>>,
    task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
    metadata: Arc<Mutex<HashMap<PipelineId, Arc<PipelineMetadata>>>>,
    /// The delayed task submitter for handling repeat_task() calls
    delayed_submitter: Option<DelayedTaskSubmitter>,
    /// Statistics event sender for observability
    stats_sender: stats::StatisticsSender,
    /// Counter for generating unique task IDs
    next_task_id: std::sync::atomic::AtomicU64,
    /// Flag set by shutdown() - executor thread pushes Shutdown task when last query completes.
    shutting_down: Arc<AtomicBool>,
}

impl Executor {
    /// Create a new executor with the default FIFO queue.
    ///
    /// The executor starts with no graph deployed. Use `deploy_graph()` via
    /// the handle to deploy a graph before emitting buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::executor::Executor;
    ///
    /// let executor = Executor::new();
    /// ```
    pub fn new() -> Self {
        Self::with_queue(FifoQueue::new())
    }

    /// Create a new executor with a custom task queue implementation.
    ///
    /// This allows using different queue strategies for testing or
    /// performance tuning.
    ///
    /// # Arguments
    ///
    /// * `queue` - The task queue implementation to use
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::executor::{Executor, RandomQueue};
    ///
    /// // Use random queue for stress testing
    /// let executor = Executor::with_queue(RandomQueue::new());
    /// ```
    pub fn with_queue<Q: TaskQueue + 'static>(queue: Q) -> Self {
        Self::with_queue_and_stats(queue, stats::StatisticsSender::noop())
    }

    /// Create a new executor with a custom task queue and statistics sender.
    ///
    /// This allows using different queue strategies and collecting execution
    /// statistics for testing or monitoring.
    ///
    /// # Arguments
    ///
    /// * `queue` - The task queue implementation to use
    /// * `stats_sender` - The statistics event sender for observability
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::executor::{Executor, FifoQueue, StatisticsSender, StatisticsEvent};
    /// use std::sync::mpsc;
    ///
    /// // Create channel for statistics
    /// let (tx, rx) = mpsc::channel::<StatisticsEvent>();
    /// let sender = StatisticsSender::new(tx);
    ///
    /// // Create executor with statistics
    /// let executor = Executor::with_queue_and_stats(FifoQueue::new(), sender);
    /// ```
    pub fn with_queue_and_stats<Q: TaskQueue + 'static>(
        queue: Q,
        stats_sender: stats::StatisticsSender,
    ) -> Self {
        let task_queue: Arc<Mutex<Box<dyn TaskQueue>>> = Arc::new(Mutex::new(Box::new(queue)));

        // Create the DelayedTaskSubmitter with access to the task queue
        let delayed_submitter = DelayedTaskSubmitter::new(Arc::clone(&task_queue));

        Self {
            queries: Arc::new(RwLock::new(HashMap::new())),
            task_queue,
            metadata: Arc::new(Mutex::new(HashMap::new())),
            delayed_submitter: Some(delayed_submitter),
            stats_sender,
            next_task_id: std::sync::atomic::AtomicU64::new(1),
            shutting_down: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get a cloneable handle for submitting tasks.
    ///
    /// The handle can be cloned and shared across threads to submit work
    /// from multiple source threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::executor::Executor;
    ///
    /// let mut executor = Executor::new();
    /// let handle = executor.get_handle();
    /// let handle_clone = handle.clone();
    /// ```
    pub fn get_handle(&self) -> ExecutorHandle {
        ExecutorHandle {
            task_queue: Arc::clone(&self.task_queue),
            metadata: Arc::clone(&self.metadata),
            queries: Arc::clone(&self.queries),
            shutting_down: Arc::clone(&self.shutting_down),
        }
    }

    /// Run the execution loop (blocking).
    ///
    /// This method runs on the execution thread and processes tasks until
    /// a `Shutdown` task is received. It returns execution statistics when
    /// the shutdown is complete.
    ///
    /// # Returns
    ///
    /// Execution statistics tracking buffers processed, pipelines started/stopped, etc.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::executor::Executor;
    /// use std::thread;
    ///
    /// let mut executor = Executor::new();
    /// let handle = executor.get_handle();
    ///
    /// let exec_thread = thread::spawn(move || {
    ///     executor.run()
    /// });
    ///
    /// // Use handle to submit work...
    /// handle.shutdown().unwrap();
    /// let stats = exec_thread.join().unwrap();
    /// ```
    pub fn run(mut self) -> ExecutionStats {
        let mut stats = ExecutionStats::new();

        loop {
            // Note: Per-query error checking is done at task processing time.
            // Each query has isolated error state, so errors in one query
            // don't stop other queries from running. Tasks for errored queries
            // are skipped (US-009 task filtering).

            // Pop a task from the queue
            let task = {
                let mut queue = self.task_queue.lock().unwrap();
                queue.pop()
            };

            // If no task, sleep briefly and continue
            let Some(task) = task else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };

            // Process the task
            match task {
                Task::WorkTask {
                    query_id,
                    pipeline_id,
                    buffer,
                } => {
                    // Filter-on-dequeue: skip tasks for stopped/removed/stopping queries
                    // query_id 0 is backward compatibility - always process
                    if query_id != 0 && !self.query_accepts_work(query_id) {
                        stats.tasks_skipped += 1;
                        // Must decrement pending counter even for skipped tasks.
                        // The counter was incremented when the task was enqueued
                        // (via enqueue_work_task/route_buffers). Without this,
                        // pending stays > 0 and the stop cascade hangs waiting
                        // for should_terminate() && pending == 0.
                        self.decrement_ref_and_check_stop(&pipeline_id, &mut stats);
                        continue;
                    }
                    self.execute_work_task(query_id, &pipeline_id, buffer, &mut stats);
                    stats.tasks_executed += 1;
                }
                Task::DeployGraph { graph, query_id } => {
                    self.execute_deploy_graph(graph, query_id, &mut stats);
                    stats.tasks_executed += 1;
                }
                Task::StartSource {
                    query_id,
                    source_id,
                } => {
                    // Filter-on-dequeue: skip tasks for stopped/removed queries
                    // query_id 0 is backward compatibility - always process
                    if query_id != 0 && !self.query_exists(query_id) {
                        stats.tasks_skipped += 1;
                        continue;
                    }
                    self.execute_start_source_task(&source_id, &mut stats);
                    stats.tasks_executed += 1;
                }
                Task::EndOfStream {
                    query_id,
                    source_id,
                    pipeline_id,
                } => {
                    // Filter-on-dequeue: skip tasks for stopped/removed queries
                    // query_id 0 is backward compatibility - always process
                    if query_id != 0 && !self.query_exists(query_id) {
                        stats.tasks_skipped += 1;
                        continue;
                    }
                    self.execute_eos_task(&source_id, &pipeline_id, &mut stats);
                    stats.tasks_executed += 1;
                }
                Task::StopPipelineTask { pipeline_id } => {
                    self.execute_stop_task(&pipeline_id, &mut stats);
                    stats.tasks_executed += 1;
                }
                Task::SourceError {
                    query_id,
                    source_id,
                    error,
                } => {
                    self.execute_source_error_task(query_id, &source_id, &error, &mut stats);
                    stats.tasks_executed += 1;
                }
                Task::Shutdown => {
                    stats.tasks_executed += 1;
                    // Stop all active pipelines
                    self.stop_all_pipelines(&mut stats);
                    break;
                }
            }
        }

        // Shutdown the DelayedTaskSubmitter and wait for its thread to finish
        if let Some(submitter) = self.delayed_submitter.take() {
            submitter.shutdown();
        }

        // Aggregate errors from all queries into stats
        {
            let queries = self.queries.read().unwrap();
            for query_state in queries.values() {
                stats.errors.extend(query_state.error_state.get_errors());
            }
        }

        // Clear all query state to release pipeline/source resources.
        // This is critical for FFI: dropping CppPipelineStage/CppSourceHandle
        // calls C++ destructors (stage_destroy/source_destroy) to free C++ objects.
        {
            let mut queries = self.queries.write().unwrap();
            queries.clear();
        }

        stats
    }

    /// Execute a single task (for testing).
    ///
    /// Pops one task from the queue and executes it. Returns true if a task
    /// was executed, false if the queue was empty.
    ///
    /// This is useful for step-through testing without spawning a thread.
    ///
    /// # Returns
    ///
    /// True if a task was executed, false if the queue was empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::executor::Executor;
    ///
    /// let mut executor = Executor::new();
    /// let handle = executor.get_handle();
    ///
    /// // Submit a task
    /// handle.shutdown().unwrap();
    ///
    /// // Execute it
    /// let executed = executor.run_one();
    /// assert!(executed);
    /// ```
    pub fn run_one(&mut self) -> bool {
        let task = {
            let mut queue = self.task_queue.lock().unwrap();
            queue.pop()
        };

        let Some(task) = task else {
            return false;
        };

        let mut stats = ExecutionStats::new();

        match task {
            Task::WorkTask {
                query_id,
                pipeline_id,
                buffer,
            } => {
                // Filter-on-dequeue: skip tasks for stopped/removed/stopping queries
                // query_id 0 is backward compatibility - always process
                if query_id != 0 && !self.query_accepts_work(query_id) {
                    // Must decrement pending counter even for skipped tasks
                    // (see run() loop for detailed explanation).
                    self.decrement_ref_and_check_stop(&pipeline_id, &mut stats);
                    return true; // Task was "executed" (skipped)
                }
                self.execute_work_task(query_id, &pipeline_id, buffer, &mut stats);
            }
            Task::DeployGraph { graph, query_id } => {
                self.execute_deploy_graph(graph, query_id, &mut stats);
            }
            Task::StartSource {
                query_id,
                source_id,
            } => {
                // Filter-on-dequeue: skip tasks for stopped/removed queries
                // query_id 0 is backward compatibility - always process
                if query_id != 0 && !self.query_exists(query_id) {
                    return true; // Task was "executed" (skipped)
                }
                self.execute_start_source_task(&source_id, &mut stats);
            }
            Task::EndOfStream {
                query_id,
                source_id,
                pipeline_id,
            } => {
                // Filter-on-dequeue: skip tasks for stopped/removed queries
                // query_id 0 is backward compatibility - always process
                if query_id != 0 && !self.query_exists(query_id) {
                    return true; // Task was "executed" (skipped)
                }
                self.execute_eos_task(&source_id, &pipeline_id, &mut stats);
            }
            Task::StopPipelineTask { pipeline_id } => {
                self.execute_stop_task(&pipeline_id, &mut stats);
            }
            Task::SourceError {
                query_id,
                source_id,
                error,
            } => {
                self.execute_source_error_task(query_id, &source_id, &error, &mut stats);
            }
            Task::Shutdown => {
                // Stop all active pipelines
                self.stop_all_pipelines(&mut stats);
            }
        }

        true
    }

    /// Execute a work task (pipeline execution with buffer).
    fn execute_work_task(
        &mut self,
        query_id: QueryId,
        pipeline_id: &PipelineId,
        buffer: Buffer,
        stats: &mut ExecutionStats,
    ) {
        // Check that pipeline metadata exists (may have been removed by terminate_query)
        {
            let metadata = self.metadata.lock().unwrap();
            let Some(meta) = metadata.get(pipeline_id) else {
                // Pipeline was terminated - skip this task
                stats.tasks_skipped += 1;
                return;
            };

            if !meta.is_setup_succeeded() {
                // Pipeline hasn't completed setup - skip this task
                stats.tasks_skipped += 1;
                return;
            }
        }

        // Read-lock the queries and find the graph for this query
        let queries_guard = self.queries.read().unwrap();

        // Look up by query_id, or fall back to first query if query_id is 0 (backward compat)
        let query_state = if query_id == 0 {
            queries_guard.values().next()
        } else {
            queries_guard.get(&query_id)
        };

        let Some(query_state) = query_state else {
            eprintln!(
                "Error: No graph deployed for work task (query_id={})",
                query_id
            );
            stats.errors_encountered += 1;
            drop(queries_guard);
            self.decrement_ref_and_check_stop(pipeline_id, stats);
            return;
        };
        let graph = &query_state.graph;

        // Check if this is a source node
        if graph.is_source(pipeline_id) {
            // Sources don't execute - just route buffers directly to successors
            // Note: Sources don't use pending task reference counting, so no decrement needed
            stats.buffers_processed += 1;
            self.route_buffers(pipeline_id, vec![buffer], graph, query_id);
            return;
        }

        // Get the pipeline
        let Some(pipeline) = graph.get_pipeline(pipeline_id) else {
            eprintln!("Error: Pipeline not found: {}", pipeline_id);
            stats.errors_encountered += 1;
            self.decrement_ref_and_check_stop(pipeline_id, stats);
            return;
        };

        // Create channel for context-emitted buffers
        let (emit_tx, emit_rx) = channel();

        // Create execution context with query_id - repeat_task support is always available
        // (context sets a flag, executor handles re-queueing after execute returns)
        let context = context::ExecutorContext::with_query_id(
            pipeline_id.clone(),
            query_id,
            0, // Single-threaded executor: worker_id = 0
            1, // Single-threaded executor: worker_count = 1
            emit_tx,
        );

        // Generate unique task ID for statistics tracking
        let task_id = self.next_task_id.fetch_add(1, Ordering::SeqCst);

        // Emit TaskExecutionStart event
        self.stats_sender
            .task_execution_start(0, query_id, pipeline_id.clone(), task_id);

        // Clone the buffer BEFORE execution for repeat support.
        // For opaque buffers, clone goes through FFI (buffer_handle_clone).
        // For owned buffers, clone copies the Vec.
        let repeat_buffer = buffer.clone();

        // Execute the pipeline with context
        match pipeline.execute(buffer, &context) {
            Ok(returned_buffers) => {
                stats.buffers_processed += 1;

                // Emit TaskExecutionComplete event
                self.stats_sender.task_execution_complete(
                    0,
                    query_id,
                    pipeline_id.clone(),
                    task_id,
                );

                // Check if repeat_task was requested during execution
                if context.was_repeat_requested() {
                    let delay_ms = context.get_repeat_delay_ms();

                    // Create the repeat task with the correct query_id
                    let repeat_task = Task::WorkTask {
                        query_id,
                        pipeline_id: pipeline_id.clone(),
                        buffer: repeat_buffer,
                    };

                    // Increment pending counter before enqueueing the repeat task
                    {
                        let metadata = self.metadata.lock().unwrap();
                        if let Some(meta) = metadata.get(pipeline_id) {
                            meta.increment_pending();
                        }
                    }

                    if delay_ms == 0 {
                        // Immediate re-queue
                        let mut queue = self.task_queue.lock().unwrap();
                        queue.push(repeat_task);
                    } else {
                        // Delayed re-queue via DelayedTaskSubmitter
                        if let Some(ref submitter) = self.delayed_submitter {
                            let _ = submitter.get_handle().submit_delayed(repeat_task, delay_ms);
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
                    self.route_buffers_with_stats(
                        pipeline_id,
                        all_buffers,
                        graph,
                        query_id,
                        task_id,
                    );
                }
            }
            Err(e) => {
                // Emit TaskExecutionComplete event (even on failure)
                self.stats_sender.task_execution_complete(
                    0,
                    query_id,
                    pipeline_id.clone(),
                    task_id,
                );

                // Record error with full context in per-query error state
                query_state.error_state.record_error(ExecutionError {
                    entity_id: pipeline_id.clone(),
                    entity_type: EntityType::Pipeline,
                    error: e.to_string(),
                    task_type: TaskType::WorkTask,
                });

                // Log to stderr
                eprintln!(
                    "FATAL ERROR: Pipeline {} failed during execution: {}",
                    pipeline_id, e
                );
                eprintln!("Terminating query {} execution immediately", query_id);

                // Keep counter for backwards compatibility
                stats.errors_encountered += 1;

                // Release the read lock before terminate_query takes write lock
                // Find the actual query_id (may be 0 for backward compat)
                let actual_query_id = if query_id != 0 {
                    query_id
                } else {
                    // Find which query owns this pipeline
                    let mut found_qid = 0;
                    for (qid, qs) in queries_guard.iter() {
                        if qs.graph.get_pipeline(pipeline_id).is_some() {
                            found_qid = *qid;
                            break;
                        }
                    }
                    found_qid
                };
                drop(queries_guard);

                // Terminate the query (removes metadata, drops pipelines)
                if actual_query_id != 0 {
                    self.terminate_query(actual_query_id, stats);
                }
                return;
            }
        }

        // Always decrement reference count after execution (success path only)
        drop(queries_guard);
        self.decrement_ref_and_check_stop(pipeline_id, stats);
    }

    /// Route output buffers to successor pipelines.
    fn route_buffers(
        &self,
        source_id: &PipelineId,
        buffers: Vec<Buffer>,
        graph: &PipelineGraph,
        query_id: QueryId,
    ) {
        // Use default values for statistics (no event emission for source routing)
        self.route_buffers_internal(source_id, buffers, graph, query_id, None);
    }

    /// Route output buffers to successor pipelines with statistics tracking.
    fn route_buffers_with_stats(
        &self,
        source_id: &PipelineId,
        buffers: Vec<Buffer>,
        graph: &PipelineGraph,
        query_id: QueryId,
        task_id: stats::TaskId,
    ) {
        self.route_buffers_internal(source_id, buffers, graph, query_id, Some(task_id));
    }

    /// Internal buffer routing with optional statistics.
    fn route_buffers_internal(
        &self,
        source_id: &PipelineId,
        buffers: Vec<Buffer>,
        graph: &PipelineGraph,
        query_id: QueryId,
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
            // Clone buffer for all successors except the last one
            for (i, successor_id) in successors.iter().enumerate() {
                let buffer_to_send = if i == successors.len() - 1 {
                    // Last successor takes ownership
                    buffer.clone()
                } else {
                    // Others get clones
                    buffer.clone()
                };

                // Emit TaskEmit event if statistics task_id is provided
                if let Some(task_id) = stats_task_id {
                    self.stats_sender.task_emit(
                        0,
                        query_id,
                        source_id.clone(),
                        successor_id.clone(),
                        task_id,
                    );
                }

                self.enqueue_work_task(query_id, successor_id.clone(), buffer_to_send);
            }
        }
    }

    /// Enqueue a work task for a pipeline.
    fn enqueue_work_task(&self, query_id: QueryId, pipeline_id: PipelineId, buffer: Buffer) {
        // Increment pending task counter
        {
            let metadata = self.metadata.lock().unwrap();
            if let Some(meta) = metadata.get(&pipeline_id) {
                meta.increment_pending();
            }
        }

        // Enqueue the task
        let mut queue = self.task_queue.lock().unwrap();
        queue.push(Task::WorkTask {
            query_id,
            pipeline_id,
            buffer,
        });
    }

    /// Execute a deploy graph task with automatic pipeline initialization.
    ///
    /// Adds the graph to the queries HashMap and returns the assigned QueryId.
    fn execute_deploy_graph(
        &mut self,
        graph: PipelineGraph,
        provided_query_id: QueryId,
        stats: &mut ExecutionStats,
    ) {
        // Use provided query_id if non-zero, otherwise auto-generate
        let query_id = if provided_query_id != 0 {
            provided_query_id
        } else {
            NEXT_EXECUTOR_QUERY_ID.fetch_add(1, Ordering::SeqCst)
        };

        // Get pipeline IDs from the graph before moving it
        let pipeline_ids = graph.get_all_pipeline_ids();

        // Create QueryState and add to the HashMap
        let query_state = QueryState::new(graph);
        {
            let mut queries = self.queries.write().unwrap();
            queries.insert(query_id, query_state);
        }

        // Emit QueryStart event
        self.stats_sender.query_start(0, query_id);

        // Auto-start all pipelines in the new graph
        let mut setup_failed = false;
        let mut successfully_setup: Vec<PipelineId> = Vec::new();

        {
            let queries_guard = self.queries.read().unwrap();
            if let Some(query_state) = queries_guard.get(&query_id) {
                let graph = &query_state.graph;

                for pipeline_id in &pipeline_ids {
                    // Create metadata with auto-configured expected sources
                    let expected_sources = graph.count_predecessors(pipeline_id);
                    let meta = Arc::new(PipelineMetadata::new());
                    meta.set_expected_sources(expected_sources);

                    {
                        let mut metadata = self.metadata.lock().unwrap();
                        metadata.insert(pipeline_id.clone(), meta);
                    }

                    // Call setup() on the pipeline
                    if let Some(pipeline) = graph.get_pipeline(pipeline_id) {
                        // Create context for setup
                        let (emit_tx, _emit_rx) = channel();
                        let context =
                            context::ExecutorContext::new(pipeline_id.clone(), 0, 1, emit_tx);

                        match pipeline.setup(&context) {
                            Ok(()) => {
                                let metadata = self.metadata.lock().unwrap();
                                if let Some(meta) = metadata.get(pipeline_id) {
                                    meta.mark_setup_succeeded();
                                }
                                successfully_setup.push(pipeline_id.clone());
                                stats.pipelines_started += 1;

                                // Emit PipelineStart event
                                self.stats_sender
                                    .pipeline_start(0, query_id, pipeline_id.clone());
                            }
                            Err(e) => {
                                // Record setup failure in per-query error state
                                query_state.error_state.record_error(ExecutionError {
                                    entity_id: pipeline_id.clone(),
                                    entity_type: EntityType::Pipeline,
                                    error: e.to_string(),
                                    task_type: TaskType::DeployGraph,
                                });

                                eprintln!(
                                    "FATAL ERROR: Pipeline {} setup failed: {}",
                                    pipeline_id, e
                                );
                                eprintln!("Terminating query {} execution immediately", query_id);

                                stats.errors_encountered += 1;
                                setup_failed = true;
                                break; // Stop setting up more pipelines
                            }
                        }
                    }
                }

                // Only enqueue StartSource tasks if no setup failed
                if !setup_failed {
                    let source_ids: Vec<PipelineId> = graph
                        .get_all_pipeline_ids()
                        .into_iter()
                        .filter(|id| graph.is_source(id))
                        .collect();

                    // Set expected source count for QueryRunning tracking
                    if let Some(qs) = queries_guard.get(&query_id) {
                        qs.expected_sources
                            .store(source_ids.len(), Ordering::SeqCst);
                    }

                    if source_ids.is_empty() {
                        // No sources - query is immediately "running"
                        self.stats_sender.query_running(0, query_id);
                    }

                    for source_id in source_ids {
                        let task = Task::StartSource {
                            query_id,
                            source_id,
                        };
                        self.task_queue.lock().unwrap().push(task);
                    }
                }
            }
        }

        // If setup failed, teardown already-setup pipelines and terminate query
        if setup_failed {
            // Teardown pipelines that were successfully set up (in reverse order)
            {
                let queries_guard = self.queries.read().unwrap();
                if let Some(query_state) = queries_guard.get(&query_id) {
                    for pid in successfully_setup.iter().rev() {
                        if let Some(pipeline) = query_state.graph.get_pipeline(pid) {
                            let (emit_tx, _emit_rx) = channel();
                            let context = context::ExecutorContext::new(pid.clone(), 0, 1, emit_tx);
                            if let Err(e) = pipeline.teardown(&context) {
                                eprintln!("Error during teardown for {}: {}", pid, e);
                            }
                        }
                    }
                }
            }

            // Terminate the query (removes metadata, drops pipelines)
            self.terminate_query(query_id, stats);
        }

        stats.graphs_deployed += 1;
    }

    /// Execute a stop pipeline task.
    fn execute_stop_task(&mut self, pipeline_id: &PipelineId, stats: &mut ExecutionStats) {
        // 1. Get successors, query_id, and check setup status BEFORE teardown
        let (successors, should_teardown, query_id) = {
            let queries_guard = self.queries.read().unwrap();

            // Find graph containing this pipeline, and get query_id
            let mut found_info: Option<(QueryId, &PipelineGraph)> = None;
            for (qid, query_state) in queries_guard.iter() {
                if query_state.graph.get_pipeline(pipeline_id).is_some() {
                    found_info = Some((*qid, &query_state.graph));
                    break;
                }
            }

            let Some((qid, graph)) = found_info else {
                return;
            };

            let successors = graph.get_successors(pipeline_id).to_vec();
            let metadata = self.metadata.lock().unwrap();
            let should_teardown = metadata
                .get(pipeline_id)
                .map(|meta| meta.is_setup_succeeded())
                .unwrap_or(false);
            (successors, should_teardown, qid)
        };

        if !should_teardown {
            self.metadata.lock().unwrap().remove(pipeline_id);
            stats.pipelines_stopped += 1;
            return;
        }

        // 2. Call flush() to get final buffers
        let flushed_buffers: Vec<Buffer> = {
            let queries_guard = self.queries.read().unwrap();

            // Find graph containing this pipeline
            let mut found_graph = None;
            for query_state in queries_guard.values() {
                if query_state.graph.get_pipeline(pipeline_id).is_some() {
                    found_graph = Some(&query_state.graph);
                    break;
                }
            }

            if let Some(graph) = found_graph {
                if let Some(pipeline) = graph.get_pipeline(pipeline_id) {
                    // Create context for flush
                    let (emit_tx, emit_rx) = channel();
                    let context = context::ExecutorContext::new(pipeline_id.clone(), 0, 1, emit_tx);

                    let returned_buffers = pipeline.flush(&context).unwrap_or_else(|e| {
                        eprintln!("Error during flush for {}: {}", pipeline_id, e);
                        stats.errors_encountered += 1;
                        vec![]
                    });

                    // Collect emitted buffers from context
                    let mut emitted_buffers = Vec::new();
                    while let Ok((_pid, buf)) = emit_rx.try_recv() {
                        emitted_buffers.push(buf);
                    }

                    // Combine returned and emitted buffers
                    returned_buffers
                        .into_iter()
                        .chain(emitted_buffers)
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            }
        };

        // 3. Route flushed buffers to successors
        if !flushed_buffers.is_empty() {
            let queries_guard = self.queries.read().unwrap();

            // Find graph containing this pipeline
            let mut found_graph = None;
            for query_state in queries_guard.values() {
                if query_state.graph.get_pipeline(pipeline_id).is_some() {
                    found_graph = Some(&query_state.graph);
                    break;
                }
            }

            if let Some(graph) = found_graph {
                self.route_buffers(pipeline_id, flushed_buffers, graph, query_id);
            }
        }

        // 4. Call teardown() - if this fails, record error and terminate the query
        let teardown_failed = {
            let queries_guard = self.queries.read().unwrap();

            // Find graph containing this pipeline
            let mut found_graph = None;
            let mut found_query_state = None;
            for (_qid, query_state) in queries_guard.iter() {
                if query_state.graph.get_pipeline(pipeline_id).is_some() {
                    found_graph = Some(&query_state.graph);
                    found_query_state = Some(query_state);
                    break;
                }
            }

            if let (Some(graph), Some(query_state)) = (found_graph, found_query_state) {
                if let Some(pipeline) = graph.get_pipeline(pipeline_id) {
                    // Create context for teardown
                    let (emit_tx, _emit_rx) = channel();
                    let context = context::ExecutorContext::new(pipeline_id.clone(), 0, 1, emit_tx);

                    match pipeline.teardown(&context) {
                        Ok(()) => false,
                        Err(e) => {
                            eprintln!(
                                "FATAL ERROR: Pipeline {} teardown failed: {}",
                                pipeline_id, e
                            );
                            eprintln!("Terminating query {} execution immediately", query_id);

                            // Record error in per-query error state
                            query_state.error_state.record_error(ExecutionError {
                                entity_id: pipeline_id.clone(),
                                entity_type: EntityType::Pipeline,
                                error: e.to_string(),
                                task_type: TaskType::StopPipeline,
                            });

                            stats.errors_encountered += 1;
                            true
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            }
        };

        if teardown_failed {
            // Remove this pipeline's metadata first (it already threw during stop)
            self.metadata.lock().unwrap().remove(pipeline_id);
            stats.pipelines_stopped += 1;

            // Terminate the entire query - remaining pipelines are just dropped
            self.terminate_query(query_id, stats);
            return;
        }

        // 5. Emit PipelineStop event
        self.stats_sender
            .pipeline_stop(0, query_id, pipeline_id.clone());

        // 6. Enqueue EndOfStream to all successors
        for successor_id in successors {
            self.task_queue.lock().unwrap().push(Task::EndOfStream {
                query_id,
                source_id: pipeline_id.clone(),
                pipeline_id: successor_id,
            });
        }

        // 7. Remove metadata
        self.metadata.lock().unwrap().remove(pipeline_id);
        stats.pipelines_stopped += 1;

        // 8. Check if this was the last pipeline for the query.
        // Check per-query pipeline status regardless of whether stop_query() was called.
        let all_query_pipelines_stopped = {
            let queries_guard = self.queries.read().unwrap();
            if let Some(qs) = queries_guard.get(&query_id) {
                let metadata = self.metadata.lock().unwrap();
                qs.graph
                    .get_all_pipeline_ids()
                    .iter()
                    .all(|pid| !metadata.contains_key(pid))
            } else {
                false
            }
        };

        if all_query_pipelines_stopped {
            // All pipelines for this query have been torn down.
            // Note: Sources are NOT stopped/torn down here because each source pipeline
            // was already torn down by its own StopPipelineTask (which calls
            // Pipeline::teardown -> Source::teardown, joining the source thread).
            // Calling stop_source()/teardown() again would be redundant and racy —
            // it causes double source_close() FFI calls that race with the source
            // thread's own close(), triggering a crash in NesSourceHandle::close()
            // which uses a non-atomic bool guard.

            // Emit QueryStop event
            self.stats_sender.query_stop(0, query_id);
            // Emit QueryTerminated event
            self.stats_sender.query_terminated(0, query_id);

            // Remove QueryState from the HashMap
            let mut queries_guard = self.queries.write().unwrap();
            queries_guard.remove(&query_id);

            // If engine is shutting down and all queries are done, push Shutdown
            if self.shutting_down.load(Ordering::SeqCst)
                && queries_guard.is_empty()
                && self.metadata.lock().unwrap().is_empty()
            {
                self.task_queue.lock().unwrap().push(Task::Shutdown);
            }
        }
    }

    /// Execute a start source task.
    ///
    /// This method is called to start a source node after all pipelines have
    /// been set up. It creates a SourceEmitHandle for the source and calls
    /// the source's start() method.
    fn execute_start_source_task(&mut self, source_id: &PipelineId, stats: &mut ExecutionStats) {
        use crate::source::SourceEmitHandle;

        // Get the source pipeline and its query state from any query graph
        let queries_guard = self.queries.read().unwrap();

        // Find graph containing this source, keeping reference to query_state for error recording
        let mut found = None;
        for (qid, query_state) in queries_guard.iter() {
            if let Some(pipeline) = query_state.graph.get_source_pipeline(source_id) {
                found = Some((*qid, pipeline, query_state));
                break;
            }
        }

        let Some((query_id, source_pipeline, query_state)) = found else {
            eprintln!("Source {} not found in any graph", source_id);
            stats.errors_encountered += 1;
            return;
        };

        // Get the shared stop flag from metadata so stop_query() can signal
        // the source thread to stop in real-time
        let stop_flag = {
            let metadata = self.metadata.lock().unwrap();
            if let Some(meta) = metadata.get(source_id) {
                meta.get_source_stop_flag()
            } else {
                eprintln!("Metadata for source {} not found", source_id);
                stats.errors_encountered += 1;
                return;
            }
        };

        // Create SourceEmitHandle with the correct query_id for work task routing
        let emit_handle = SourceEmitHandle::new(
            source_id.clone(),
            query_id,
            self.task_queue.clone(),
            stop_flag,
        );

        // Start the source
        match source_pipeline.start_source(emit_handle) {
            Ok(()) => {
                let metadata = self.metadata.lock().unwrap();
                if let Some(meta) = metadata.get(source_id) {
                    meta.mark_source_started();
                }

                // Track sources started for QueryRunning event
                let prev = query_state.sources_started.fetch_add(1, Ordering::SeqCst);
                let expected = query_state.expected_sources.load(Ordering::SeqCst);
                if expected > 0 && prev + 1 >= expected {
                    // All sources started - query is now fully operational
                    self.stats_sender.query_running(0, query_id);
                }
            }
            Err(e) => {
                // Record error with full context in per-query error state
                query_state.error_state.record_error(ExecutionError {
                    entity_id: source_id.clone(),
                    entity_type: EntityType::Source,
                    error: e.to_string(),
                    task_type: TaskType::StartSource,
                });

                // Log to stderr
                eprintln!("FATAL ERROR: Source {} failed to start: {}", source_id, e);
                eprintln!("Terminating query {} execution immediately", query_id);

                // Keep counter for backwards compatibility
                stats.errors_encountered += 1;

                // Release the read lock before terminate_query takes write lock
                drop(queries_guard);

                // Terminate the query
                self.terminate_query(query_id, stats);
            }
        }
    }

    /// Execute a source error task.
    ///
    /// Called when a source encounters an error (e.g., C++ source throws during
    /// next_buffer). Records the error and terminates the query.
    fn execute_source_error_task(
        &mut self,
        query_id: QueryId,
        source_id: &PipelineId,
        error: &str,
        stats: &mut ExecutionStats,
    ) {
        // Find the query and record the error
        let actual_query_id = {
            let queries_guard = self.queries.read().unwrap();

            // Find the query either by ID or by searching for the source
            let found = if query_id != 0 {
                queries_guard.get(&query_id).map(|qs| (query_id, qs))
            } else {
                // Search all queries for one containing this source
                queries_guard.iter().find_map(|(qid, qs)| {
                    if qs.graph.get_pipeline(source_id).is_some() {
                        Some((*qid, qs))
                    } else {
                        None
                    }
                })
            };

            if let Some((qid, query_state)) = found {
                // Record the error
                query_state.error_state.record_error(ExecutionError {
                    entity_id: source_id.clone(),
                    entity_type: EntityType::Source,
                    error: error.to_string(),
                    task_type: TaskType::StartSource, // Closest match for source runtime errors
                });

                eprintln!("FATAL ERROR: Source {} failed: {}", source_id, error);
                eprintln!("Terminating query {} execution immediately", qid);

                stats.errors_encountered += 1;
                qid
            } else {
                // Query already terminated
                return;
            }
        };

        // Terminate the query
        self.terminate_query(actual_query_id, stats);
    }

    /// Execute an end-of-stream task.
    fn execute_eos_task(
        &mut self,
        _source_id: &PipelineId,
        pipeline_id: &PipelineId,
        _stats: &mut ExecutionStats,
    ) {
        let metadata = self.metadata.lock().unwrap();

        if let Some(meta) = metadata.get(pipeline_id) {
            // Increment EOS counter
            let eos_count = meta.increment_eos();
            let expected = meta.get_expected_sources();

            // Check if all sources have finished
            if expected > 0 && eos_count >= expected {
                // All sources finished - mark for termination
                meta.request_termination();
                drop(metadata);

                // Check if ready to stop (all sources done + no pending work)
                let should_stop = {
                    let metadata = self.metadata.lock().unwrap();
                    metadata
                        .get(pipeline_id)
                        .map(|m| m.should_terminate() && m.get_pending() == 0)
                        .unwrap_or(false)
                };

                if should_stop {
                    self.task_queue
                        .lock()
                        .unwrap()
                        .push(Task::StopPipelineTask {
                            pipeline_id: pipeline_id.clone(),
                        });
                }
            }
        } else {
            eprintln!(
                "Warning: End-of-stream for pipeline {} that doesn't exist",
                pipeline_id
            );
        }
    }

    /// Decrement reference count and check if pipeline should be stopped.
    fn decrement_ref_and_check_stop(&self, pipeline_id: &PipelineId, _stats: &mut ExecutionStats) {
        let metadata = self.metadata.lock().unwrap();

        if let Some(meta) = metadata.get(pipeline_id) {
            meta.decrement_pending();
        }

        drop(metadata);

        // Check if ready to stop
        let should_stop = {
            let metadata = self.metadata.lock().unwrap();
            metadata
                .get(pipeline_id)
                .map(|m| m.should_terminate() && m.get_pending() == 0)
                .unwrap_or(false)
        };

        if should_stop {
            self.task_queue
                .lock()
                .unwrap()
                .push(Task::StopPipelineTask {
                    pipeline_id: pipeline_id.clone(),
                });
        }
    }

    /// Check if a query exists in the executor.
    ///
    /// Used for filter-on-dequeue pattern: tasks for stopped/removed queries
    /// are skipped when dequeued.
    ///
    /// # Arguments
    ///
    /// * `query_id` - The ID of the query to check
    ///
    /// # Returns
    ///
    /// `true` if the query exists, `false` otherwise.
    fn query_exists(&self, query_id: QueryId) -> bool {
        let queries = self.queries.read().unwrap();
        queries.contains_key(&query_id)
    }

    /// Check if a query should accept new work tasks.
    ///
    /// Returns `false` if the query doesn't exist, or if it exists but is
    /// in the stopping state (stop_query was called). Used by filter-on-dequeue
    /// to skip WorkTasks for queries that are being shut down.
    fn query_accepts_work(&self, query_id: QueryId) -> bool {
        let queries = self.queries.read().unwrap();
        if let Some(qs) = queries.get(&query_id) {
            !qs.is_stopping()
        } else {
            false
        }
    }

    /// Terminate a query due to an error.
    ///
    /// This removes all pipeline metadata for the query and removes the query
    /// state from the HashMap. Dropping the QueryState triggers Drop on all
    /// pipelines, which calls C++ destructors (stage_destroy) but does NOT
    /// call stop() - this is exactly what the error path tests expect.
    ///
    /// CRITICAL: This does NOT call flush()/stop()/teardown() on any pipeline.
    /// On error, pipelines are just dropped (destroyed).
    fn terminate_query(&mut self, query_id: QueryId, stats: &mut ExecutionStats) {
        // Collect errors and source IDs from the query before modifying state
        let (pipeline_ids_to_remove, errors, source_ids) = {
            let queries = self.queries.read().unwrap();
            let Some(query_state) = queries.get(&query_id) else {
                return;
            };

            // Get all pipeline IDs belonging to this query
            let pipeline_ids: Vec<PipelineId> = query_state.graph.get_all_pipeline_ids();

            // Collect errors
            let errors = query_state.error_state.get_errors();

            // Get source pipeline IDs (need special handling to stop threads)
            let source_ids: Vec<PipelineId> = pipeline_ids
                .iter()
                .filter(|pid| query_state.graph.is_source(pid))
                .cloned()
                .collect();

            (pipeline_ids, errors, source_ids)
        };

        // Stop all source threads BEFORE removing query state.
        // This is critical because source threads hold Arc clones of the C++
        // SourceHandle. If we don't join the threads first, the CppSourceHandle
        // won't be destroyed (test expects wait_until_destroyed() to succeed).
        //
        // Note: We call stop() then teardown() on sources. This sets their
        // stopped flag, closes the C++ handle (unblocks next_buffer), and
        // joins the thread. This does NOT call C++ PipelineStage::stop()
        // (which is what was_stopped() checks) - it only affects the Source trait.
        {
            let queries = self.queries.read().unwrap();
            if let Some(query_state) = queries.get(&query_id) {
                for source_id in &source_ids {
                    if let Some(source_pipeline) = query_state.graph.get_source_pipeline(source_id)
                    {
                        // Stop the source (signals thread to exit, closes C++ handle)
                        let _ = source_pipeline.stop_source();
                        // Teardown joins the thread so Arc ref count drops
                        let _ = source_pipeline.source().teardown();
                    }
                }
            }
        }

        // Remove pipeline metadata for all pipelines in this query
        {
            let mut metadata = self.metadata.lock().unwrap();
            for pid in &pipeline_ids_to_remove {
                if metadata.remove(pid).is_some() {
                    stats.pipelines_stopped += 1;
                }
            }
        }

        // Remove query state (Drop cascade calls stage_destroy, NOT stop)
        let removed_query = {
            let mut queries = self.queries.write().unwrap();
            queries.remove(&query_id)
        };

        if let Some(_query_state) = removed_query {
            // Emit QueryStop event
            self.stats_sender.query_stop(0, query_id);
            // Emit QueryTerminated event
            self.stats_sender.query_terminated(0, query_id);
        }

        // Store errors for later aggregation in stats
        // We need to push them into stats.errors directly since the query_state
        // is now removed and won't be aggregated in run()'s final loop
        stats.errors.extend(errors);

        // If engine is shutting down and all queries are done, push Shutdown
        if self.shutting_down.load(Ordering::SeqCst) {
            let queries_empty = self.queries.read().unwrap().is_empty();
            let metadata_empty = self.metadata.lock().unwrap().is_empty();
            if queries_empty && metadata_empty {
                self.task_queue.lock().unwrap().push(Task::Shutdown);
            }
        }
    }

    /// Emergency shutdown - stops all pipelines immediately without cascading.
    ///
    /// Used only when Shutdown task is received (e.g., emergency stop or empty graph).
    /// Normal graceful shutdown uses cascading StopPipelineTasks.
    fn stop_all_pipelines(&self, stats: &mut ExecutionStats) {
        // Get list of all active pipelines
        let pipeline_ids: Vec<PipelineId> = {
            let metadata = self.metadata.lock().unwrap();
            metadata.keys().cloned().collect()
        };

        // Call teardown on each pipeline - search all query graphs
        let queries_guard = self.queries.read().unwrap();
        for pipeline_id in &pipeline_ids {
            // Find the graph containing this pipeline
            let mut found_pipeline = None;
            for query_state in queries_guard.values() {
                if let Some(pipeline) = query_state.graph.get_pipeline(pipeline_id) {
                    found_pipeline = Some(pipeline);
                    break;
                }
            }

            if let Some(pipeline) = found_pipeline {
                // Check if setup succeeded before calling teardown
                let should_teardown = {
                    let metadata = self.metadata.lock().unwrap();
                    metadata
                        .get(pipeline_id)
                        .map(|meta| meta.is_setup_succeeded())
                        .unwrap_or(false)
                };

                if should_teardown {
                    // Create context for teardown
                    let (emit_tx, _emit_rx) = channel();
                    let context = context::ExecutorContext::new(pipeline_id.clone(), 0, 1, emit_tx);

                    if let Err(e) = pipeline.teardown(&context) {
                        eprintln!("Error during pipeline teardown for {}: {}", pipeline_id, e);
                        stats.errors_encountered += 1;
                    }
                }
            }
        }

        // Remove all metadata
        let mut metadata = self.metadata.lock().unwrap();
        stats.pipelines_stopped += pipeline_ids.len();
        metadata.clear();
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}
