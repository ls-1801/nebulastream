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

//! Engine handle for FFI.
//!
//! This module provides the EngineHandle that wraps the Rust Executor,
//! allowing C++ code to create and control the execution engine.

use crate::executor::stats::{StatisticsEvent, StatisticsSender};
use crate::executor::{Executor, ExecutorHandle, QueryId};
#[cfg(feature = "cpp-ffi")]
use crate::ffi::callbacks::{CppPipelineStage, CppSourceAdapter, CppSourceHandle};
#[cfg(feature = "cpp-ffi")]
use crate::graph::PipelineGraph;
use crate::pipeline::PipelineId;
use crate::query_engine::QueryEngine;
use std::collections::HashMap;
#[cfg(feature = "cpp-ffi")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// Global counter for generating unique query IDs.
#[cfg(feature = "cpp-ffi")]
static NEXT_QUERY_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque handle to the Rust execution engine.
///
/// This type is exported to C++ via CXX and provides the main interface
/// for controlling the adaptive execution engine.
pub struct EngineHandle {
    /// Handle for submitting tasks to the executor (updated on each start)
    executor_handle: Arc<Mutex<Option<ExecutorHandle>>>,
    /// Opaque context pointer (e.g., NesBufferProvider*)
    context_ptr: usize,
    /// The executor instance (Some before start, None after start)
    executor: Arc<Mutex<Option<Executor>>>,
    /// Thread handle for the executor (Some when running)
    executor_thread: Arc<Mutex<Option<JoinHandle<crate::executor::ExecutionStats>>>>,
    /// Execution statistics from the last run
    last_stats: Arc<Mutex<Option<crate::executor::ExecutionStats>>>,
    /// Maps QueryId to the pipeline IDs belonging to that query.
    queries: Mutex<HashMap<QueryId, Vec<PipelineId>>>,
    /// Query engine for pipeline-to-query event mapping (Some when stats enabled).
    query_engine: Arc<Mutex<Option<QueryEngine>>>,
}

// SAFETY: EngineHandle is Send because:
// - ExecutorHandle is Clone and thread-safe
// - context_ptr is just a usize
// - executor, executor_thread and last_stats use Arc<Mutex>
unsafe impl Send for EngineHandle {}

// SAFETY: EngineHandle is Sync because:
// - All mutable state is protected by Mutex
// - ExecutorHandle is thread-safe
unsafe impl Sync for EngineHandle {}

impl EngineHandle {
    /// Create a new engine handle.
    ///
    /// # Arguments
    /// * `context_ptr` - Opaque context pointer
    ///
    /// # Returns
    /// A new EngineHandle ready to be started
    pub fn new(context_ptr: usize) -> Self {
        Self::new_with_workers(context_ptr, 1)
    }

    /// Create a new engine handle with the specified number of worker threads.
    pub fn new_with_workers(context_ptr: usize, num_workers: usize) -> Self {
        let executor = Executor::with_worker_count(num_workers);
        let handle = executor.get_handle();

        Self {
            executor_handle: Arc::new(Mutex::new(Some(handle))),
            context_ptr,
            executor: Arc::new(Mutex::new(Some(executor))),
            executor_thread: Arc::new(Mutex::new(None)),
            last_stats: Arc::new(Mutex::new(None)),
            queries: Mutex::new(HashMap::new()),
            query_engine: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a new engine handle with statistics collection enabled.
    ///
    /// Returns both the engine handle and a stats queue for polling events.
    pub fn new_with_stats(context_ptr: usize) -> (Self, StatsQueueHandle) {
        Self::new_with_workers_and_stats(context_ptr, 1)
    }

    /// Create a new engine handle with worker threads and statistics collection.
    pub fn new_with_workers_and_stats(
        context_ptr: usize,
        num_workers: usize,
    ) -> (Self, StatsQueueHandle) {
        // Two channels: raw (executor → query engine) and processed (query engine → consumer)
        let (raw_tx, raw_rx) = mpsc::channel::<StatisticsEvent>();
        let (processed_tx, processed_rx) = mpsc::channel::<StatisticsEvent>();

        let sender = StatisticsSender::new(raw_tx);
        let executor = Executor::with_worker_count_and_stats(num_workers, sender);
        let handle = executor.get_handle();

        let query_engine = QueryEngine::new(raw_rx, processed_tx, handle.clone());

        let engine = Self {
            executor_handle: Arc::new(Mutex::new(Some(handle))),
            context_ptr,
            executor: Arc::new(Mutex::new(Some(executor))),
            executor_thread: Arc::new(Mutex::new(None)),
            last_stats: Arc::new(Mutex::new(None)),
            queries: Mutex::new(HashMap::new()),
            query_engine: Arc::new(Mutex::new(Some(query_engine))),
        };

        let stats_queue = StatsQueueHandle {
            receiver: Mutex::new(processed_rx),
        };

        (engine, stats_queue)
    }

    /// Start the engine's execution thread.
    ///
    /// This spawns a new thread that runs the executor's main loop.
    /// The thread will process tasks until shutdown is called.
    pub fn start(&self) {
        // Take the executor out - it will be moved to the thread
        let executor = {
            let mut guard = self.executor.lock().unwrap();
            guard.take()
        };

        let Some(executor) = executor else {
            eprintln!("Warning: Engine already started or no executor available");
            return;
        };

        // Spawn the execution thread
        let thread_handle = thread::spawn(move || executor.run());

        // Store the thread handle
        let mut guard = self.executor_thread.lock().unwrap();
        *guard = Some(thread_handle);
    }

    /// Shutdown the engine.
    ///
    /// This signals the executor to stop processing and waits for the
    /// execution thread to complete.
    pub fn shutdown(&self) {
        // Signal shutdown via the executor handle
        {
            let guard = self.executor_handle.lock().unwrap();
            if let Some(ref handle) = *guard {
                if let Err(e) = handle.shutdown() {
                    eprintln!("Error signaling shutdown: {}", e);
                }
            }
        }

        // Wait for the execution thread to finish
        let thread_handle = {
            let mut guard = self.executor_thread.lock().unwrap();
            guard.take()
        };

        if let Some(handle) = thread_handle {
            match handle.join() {
                Ok(stats) => {
                    // Store the final stats
                    let mut stats_guard = self.last_stats.lock().unwrap();
                    *stats_guard = Some(stats);
                }
                Err(e) => {
                    eprintln!("Executor thread panicked: {:?}", e);
                }
            }
        }

        // Drop the executor handle to close the raw stats channel,
        // which signals the QueryEngine processing thread to exit.
        {
            let mut guard = self.executor_handle.lock().unwrap();
            guard.take();
        }

        // Stop the QueryEngine processing thread
        {
            let mut guard = self.query_engine.lock().unwrap();
            if let Some(qe) = guard.take() {
                qe.stop();
            }
        }
    }

    /// Get the buffer provider pointer.
    pub fn context_ptr(&self) -> usize {
        self.context_ptr
    }

    /// Get a clone of the executor handle for submitting tasks.
    ///
    /// Returns None if the engine hasn't been created properly.
    pub fn get_executor_handle(&self) -> Option<ExecutorHandle> {
        let guard = self.executor_handle.lock().unwrap();
        guard.clone()
    }

    /// Get the last execution statistics.
    pub fn get_stats(&self) -> super::ffi::FfiExecutionStats {
        let guard = self.last_stats.lock().unwrap();
        match &*guard {
            Some(stats) => super::ffi::FfiExecutionStats {
                buffers_processed: stats.buffers_processed as u64,
                bytes_processed: 0, // Not tracked in current implementation
                tasks_executed: stats.tasks_executed as u64,
                active_queries: 0,   // Not tracked in current implementation
                avg_latency_ms: 0.0, // Not tracked in current implementation
            },
            None => super::ffi::FfiExecutionStats {
                buffers_processed: 0,
                bytes_processed: 0,
                tasks_executed: 0,
                active_queries: 0,
                avg_latency_ms: 0.0,
            },
        }
    }
}

/// Opaque handle to the statistics event queue.
///
/// This type is exported to C++ via FFI and provides polling access to
/// the statistics event channel.
pub struct StatsQueueHandle {
    #[cfg_attr(not(feature = "cpp-ffi"), allow(dead_code))]
    receiver: Mutex<mpsc::Receiver<StatisticsEvent>>,
}

// SAFETY: StatsQueueHandle is Send/Sync because the receiver is behind a Mutex
unsafe impl Send for StatsQueueHandle {}
unsafe impl Sync for StatsQueueHandle {}

/// C-compatible statistics event type tag.
#[cfg(feature = "cpp-ffi")]
#[repr(u32)]
pub enum FfiStatisticsEventType {
    None = 0,
    QueryStart = 1,
    QueryStop = 2,
    PipelineStart = 3,
    PipelineStop = 4,
    TaskExecutionStart = 5,
    TaskExecutionComplete = 6,
    TaskEmit = 7,
    QueryRunning = 8,
    QueryTerminated = 9,
}

/// C-compatible statistics event.
///
/// All events share the same flat struct. Fields that don't apply to a
/// particular event type are set to 0/empty.
#[cfg(feature = "cpp-ffi")]
#[repr(C)]
pub struct FfiStatisticsEvent {
    pub event_type: u32,
    pub worker_id: u64,
    pub query_id: u64,
    pub pipeline_id_ptr: *const u8,
    pub pipeline_id_len: usize,
    pub to_pipeline_id_ptr: *const u8,
    pub to_pipeline_id_len: usize,
    pub task_id: u64,
}

#[cfg(feature = "cpp-ffi")]
impl Default for FfiStatisticsEvent {
    fn default() -> Self {
        Self {
            event_type: FfiStatisticsEventType::None as u32,
            worker_id: 0,
            query_id: 0,
            pipeline_id_ptr: std::ptr::null(),
            pipeline_id_len: 0,
            to_pipeline_id_ptr: std::ptr::null(),
            to_pipeline_id_len: 0,
            task_id: 0,
        }
    }
}

// FFI functions exported via CXX bridge

/// Create a new engine instance.
///
/// # Arguments
/// * `context_ptr` - Opaque context pointer
///
/// # Returns
/// Box containing the new EngineHandle
pub fn engine_create(context_ptr: usize) -> Box<EngineHandle> {
    Box::new(EngineHandle::new(context_ptr))
}

/// Create a new engine instance with the specified number of worker threads.
///
/// # Arguments
/// * `context_ptr` - Opaque context pointer
/// * `num_workers` - Number of worker threads (minimum 1)
///
/// # Returns
/// Box containing the new EngineHandle
pub fn engine_create_with_workers(context_ptr: usize, num_workers: usize) -> Box<EngineHandle> {
    Box::new(EngineHandle::new_with_workers(context_ptr, num_workers))
}

/// Start the engine's worker threads.
///
/// # Arguments
/// * `engine` - The engine handle
pub fn engine_start(engine: &EngineHandle) {
    engine.start();
}

/// Shutdown the engine.
///
/// # Arguments
/// * `engine` - The engine handle
pub fn engine_shutdown(engine: &EngineHandle) {
    engine.shutdown();
}

/// Get global engine statistics.
///
/// # Arguments
/// * `engine` - The engine handle
///
/// # Returns
/// Current execution statistics
pub fn engine_get_stats(engine: &EngineHandle) -> super::ffi::FfiExecutionStats {
    engine.get_stats()
}

/// Submit a query to the engine for execution.
///
/// This function builds a pipeline graph from the provided C++ stage pointers
/// and edges, then deploys it to the executor.
///
/// # Arguments
/// * `engine` - The engine handle
/// * `stage_ptrs` - Pointers to C++ PipelineStage instances
/// * `edges` - Pairs of (source_index, target_index) defining connections
/// * `source_ptrs` - Pointers to C++ SourceHandle instances (currently unused)
/// * `source_to_stage` - Pairs of (source_index, stage_index) mappings (currently unused)
/// * `user_data` - Opaque user data pointer (currently unused)
///
/// # Returns
/// A QueryId for tracking and controlling the query
///
/// # Safety
/// The caller must ensure:
/// - All stage_ptrs are valid C++ PipelineStage pointers
/// - All source_ptrs are valid C++ SourceHandle pointers
/// - All edge indices are within bounds
#[cfg(feature = "cpp-ffi")]
pub fn engine_submit_query(
    engine: &EngineHandle,
    stage_ptrs: &[usize],
    edges: &[(u64, u64)],
    source_ptrs: &[usize],
    source_to_stage: &[(u64, u64)],
    _user_data: usize,
) -> QueryId {
    // Generate a unique query ID
    let query_id = NEXT_QUERY_ID.fetch_add(1, Ordering::SeqCst);

    // Build a pipeline graph from the C++ stage pointers
    let mut graph = PipelineGraph::new();
    let context_ptr = engine.context_ptr();

    // Add each stage as a pipeline
    for (idx, &stage_ptr) in stage_ptrs.iter().enumerate() {
        let pipeline_id = PipelineId::new(format!("query-{}-stage-{}", query_id, idx));
        let stage = unsafe { CppPipelineStage::new(pipeline_id, stage_ptr, context_ptr) };
        if let Err(e) = graph.add_pipeline(Box::new(stage)) {
            eprintln!("Error adding pipeline for query {}: {}", query_id, e);
            return 0;
        }
    }

    // Add each source as a CppSourceAdapter
    for (idx, &source_ptr) in source_ptrs.iter().enumerate() {
        let source_id = PipelineId::new(format!("query-{}-source-{}", query_id, idx));
        let source_handle = unsafe { CppSourceHandle::new(source_id, source_ptr, context_ptr) };
        let source_adapter = Arc::new(CppSourceAdapter::new(source_handle));
        if let Err(e) = graph.add_source(source_adapter) {
            eprintln!("Error adding source for query {}: {}", query_id, e);
            return 0;
        }
    }

    // Connect stages according to edges
    for &(source_idx, target_idx) in edges {
        let source_id = PipelineId::new(format!("query-{}-stage-{}", query_id, source_idx));
        let target_id = PipelineId::new(format!("query-{}-stage-{}", query_id, target_idx));
        if let Err(e) = graph.connect(&source_id, &target_id) {
            eprintln!("Error connecting stages for query {}: {}", query_id, e);
            return 0;
        }
    }

    // Connect sources to stages according to source_to_stage mappings
    for &(src_idx, stage_idx) in source_to_stage {
        let source_id = PipelineId::new(format!("query-{}-source-{}", query_id, src_idx));
        let target_id = PipelineId::new(format!("query-{}-stage-{}", query_id, stage_idx));
        if let Err(e) = graph.connect(&source_id, &target_id) {
            eprintln!(
                "Error connecting source to stage for query {}: {}",
                query_id, e
            );
            return 0;
        }
    }

    // Validate the graph is a valid DAG
    if let Err(e) = graph.validate() {
        eprintln!("Invalid graph for query {}: {}", query_id, e);
        return 0;
    }

    // Record all pipeline IDs for this query
    let pipeline_ids = graph.get_all_pipeline_ids();

    // Extract source IDs before deploying
    let source_ids: Vec<PipelineId> = pipeline_ids
        .iter()
        .filter(|id| graph.is_source(id))
        .cloned()
        .collect();

    {
        let mut queries = engine.queries.lock().unwrap();
        queries.insert(query_id, pipeline_ids.clone());
    }

    // Register with QueryEngine if present
    {
        let guard = engine.query_engine.lock().unwrap();
        if let Some(ref qe) = *guard {
            qe.register_query(query_id, pipeline_ids, source_ids);
        }
    }

    // Deploy the graph to the executor
    let Some(handle) = engine.get_executor_handle() else {
        eprintln!(
            "Error deploying graph for query {}: no executor handle",
            query_id
        );
        return 0;
    };
    if let Err(e) = handle.deploy_graph(graph) {
        eprintln!("Error deploying graph for query {}: {}", query_id, e);
        return 0;
    }

    query_id
}

/// Stop a running query.
///
/// This function stops only the specified query, leaving other queries running.
/// The query's pipelines will be gracefully stopped by enqueueing StopPipelineTask
/// for each source pipeline.
///
/// # Arguments
/// * `engine` - The engine handle
/// * `query_id` - The ID of the query to stop
///
/// # Returns
/// True if the query was found and stop was initiated, false if the query ID
/// was not found or an error occurred.
pub fn engine_stop_query(engine: &EngineHandle, query_id: QueryId) -> bool {
    // Look up which pipelines belong to this query
    let pipeline_ids = {
        let queries = engine.queries.lock().unwrap();
        match queries.get(&query_id) {
            Some(ids) => ids.clone(),
            None => {
                eprintln!("Query {} not found", query_id);
                return false;
            }
        }
    };

    let Some(handle) = engine.get_executor_handle() else {
        eprintln!("Error stopping query {}: no executor handle", query_id);
        return false;
    };

    // Find which of these pipelines are sources in the current graph
    let source_ids: Vec<PipelineId> = {
        let graph = match handle.graph().read() {
            Ok(g) => g,
            Err(e) => {
                eprintln!(
                    "Error stopping query {}: graph lock poisoned: {}",
                    query_id, e
                );
                return false;
            }
        };

        pipeline_ids
            .iter()
            .filter(|id| graph.is_source(id))
            .cloned()
            .collect()
    };

    if source_ids.is_empty() {
        // Query has no source pipelines in the graph (may have already completed)
        let mut queries = engine.queries.lock().unwrap();
        queries.remove(&query_id);
        return false;
    }

    match handle.stop_pipelines(&source_ids) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("Error stopping query {}: {}", query_id, e);
            false
        }
    }
}

// =============================================================================
// Raw C FFI exports for direct C++ interoperability
// =============================================================================
// These functions use #[no_mangle] and extern "C" to be callable from C++ code
// without going through CXX bridge.

/// Create a new engine instance (C FFI).
///
/// # Safety
/// The caller must ensure the returned pointer is eventually freed with `engine_destroy`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub extern "C" fn engine_create_ffi(context_ptr: usize) -> *mut EngineHandle {
    Box::into_raw(Box::new(EngineHandle::new(context_ptr)))
}

/// Create a new engine instance with the specified number of worker threads (C FFI).
///
/// # Safety
/// The caller must ensure the returned pointer is eventually freed with `engine_destroy`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub extern "C" fn engine_create_with_workers_ffi(
    context_ptr: usize,
    num_workers: usize,
) -> *mut EngineHandle {
    Box::into_raw(Box::new(EngineHandle::new_with_workers(
        context_ptr,
        num_workers,
    )))
}

/// Start the engine's worker threads (C FFI).
///
/// # Safety
/// The caller must ensure `engine` is a valid pointer returned by `engine_create_ffi`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_start_ffi(engine: *const EngineHandle) {
    if let Some(engine) = unsafe { engine.as_ref() } {
        engine.start();
    }
}

/// Shutdown the engine (C FFI).
///
/// # Safety
/// The caller must ensure `engine` is a valid pointer returned by `engine_create_ffi`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_shutdown_ffi(engine: *const EngineHandle) {
    if let Some(engine) = unsafe { engine.as_ref() } {
        engine.shutdown();
    }
}

/// Get global engine statistics via out parameters (C FFI).
///
/// # Safety
/// The caller must ensure:
/// - `engine` is a valid pointer returned by `engine_create_ffi`
/// - All out pointers are valid and aligned
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_get_stats_raw(
    engine: *const EngineHandle,
    buffers_processed: *mut u64,
    bytes_processed: *mut u64,
    tasks_executed: *mut u64,
    active_queries: *mut u64,
    avg_latency_ms: *mut f64,
) {
    let stats = if let Some(engine) = unsafe { engine.as_ref() } {
        engine.get_stats()
    } else {
        super::ffi::FfiExecutionStats::default()
    };

    if !buffers_processed.is_null() {
        unsafe { *buffers_processed = stats.buffers_processed };
    }
    if !bytes_processed.is_null() {
        unsafe { *bytes_processed = stats.bytes_processed };
    }
    if !tasks_executed.is_null() {
        unsafe { *tasks_executed = stats.tasks_executed };
    }
    if !active_queries.is_null() {
        unsafe { *active_queries = stats.active_queries };
    }
    if !avg_latency_ms.is_null() {
        unsafe { *avg_latency_ms = stats.avg_latency_ms };
    }
}

/// Submit a query to the engine (C FFI).
///
/// # Safety
/// The caller must ensure:
/// - `engine` is a valid pointer returned by `engine_create_ffi`
/// - All array pointers are valid for their respective lengths
/// - All stage/source pointers in the arrays are valid C++ objects
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_submit_query_raw(
    engine: *const EngineHandle,
    stage_ptrs: *const usize,
    num_stages: usize,
    edge_sources: *const u64,
    edge_targets: *const u64,
    num_edges: usize,
    source_ptrs: *const usize,
    num_sources: usize,
    source_to_stage_sources: *const u64,
    source_to_stage_targets: *const u64,
    num_source_mappings: usize,
    user_data: usize,
) -> QueryId {
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return 0;
    };

    // Convert C arrays to Rust slices
    let stages = if stage_ptrs.is_null() || num_stages == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(stage_ptrs, num_stages) }
    };

    // Build edges from separate source/target arrays
    let edges: Vec<(u64, u64)> =
        if edge_sources.is_null() || edge_targets.is_null() || num_edges == 0 {
            vec![]
        } else {
            let sources = unsafe { std::slice::from_raw_parts(edge_sources, num_edges) };
            let targets = unsafe { std::slice::from_raw_parts(edge_targets, num_edges) };
            sources
                .iter()
                .copied()
                .zip(targets.iter().copied())
                .collect()
        };

    let sources = if source_ptrs.is_null() || num_sources == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(source_ptrs, num_sources) }
    };

    // Build source_to_stage from separate arrays
    let source_to_stage: Vec<(u64, u64)> = if source_to_stage_sources.is_null()
        || source_to_stage_targets.is_null()
        || num_source_mappings == 0
    {
        vec![]
    } else {
        let s_sources =
            unsafe { std::slice::from_raw_parts(source_to_stage_sources, num_source_mappings) };
        let s_targets =
            unsafe { std::slice::from_raw_parts(source_to_stage_targets, num_source_mappings) };
        s_sources
            .iter()
            .copied()
            .zip(s_targets.iter().copied())
            .collect()
    };

    engine_submit_query(engine, stages, &edges, sources, &source_to_stage, user_data)
}

/// Stop a running query (C FFI).
///
/// # Safety
/// The caller must ensure `engine` is a valid pointer returned by `engine_create_ffi`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_stop_query_ffi(
    engine: *const EngineHandle,
    query_id: QueryId,
) -> bool {
    if let Some(engine) = unsafe { engine.as_ref() } {
        engine_stop_query(engine, query_id)
    } else {
        false
    }
}

/// Free the engine handle (C FFI).
///
/// # Safety
/// The caller must ensure `engine` is a valid pointer returned by `engine_create_ffi`
/// and that it hasn't already been freed.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_destroy(engine: *mut EngineHandle) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

// =============================================================================
// Statistics FFI exports
// =============================================================================

/// Create a new engine instance with statistics collection (C FFI).
///
/// Returns the engine handle via `out_engine` and the stats queue via `out_stats`.
///
/// # Safety
/// The caller must ensure out_engine and out_stats are valid pointers.
/// The returned pointers must be freed with `engine_destroy` and `stats_queue_destroy`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_create_with_stats_ffi(
    context_ptr: usize,
    out_stats: *mut *mut StatsQueueHandle,
) -> *mut EngineHandle {
    let (engine, stats) = EngineHandle::new_with_stats(context_ptr);

    if !out_stats.is_null() {
        unsafe { *out_stats = Box::into_raw(Box::new(stats)) };
    }

    Box::into_raw(Box::new(engine))
}

/// Create a new engine instance with worker threads and statistics collection (C FFI).
///
/// # Safety
/// The caller must ensure out_stats is a valid pointer.
/// The returned pointers must be freed with `engine_destroy` and `stats_queue_destroy`.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_create_with_workers_and_stats_ffi(
    context_ptr: usize,
    num_workers: usize,
    out_stats: *mut *mut StatsQueueHandle,
) -> *mut EngineHandle {
    let (engine, stats) = EngineHandle::new_with_workers_and_stats(context_ptr, num_workers);

    if !out_stats.is_null() {
        unsafe { *out_stats = Box::into_raw(Box::new(stats)) };
    }

    Box::into_raw(Box::new(engine))
}

/// Poll the next statistics event from the queue (C FFI).
///
/// Writes the event into the provided `out_event`. Returns true if an event
/// was available, false if the queue is empty or the timeout expired.
///
/// The `timeout_ms` parameter specifies how long to wait for an event:
/// - 0: non-blocking (try_recv)
/// - >0: wait up to timeout_ms milliseconds
///
/// IMPORTANT: The pipeline_id_ptr and to_pipeline_id_ptr in the returned event
/// point to thread-local storage that is only valid until the next call to
/// engine_poll_event_ffi. The caller must copy the string data before calling again.
///
/// # Safety
/// The caller must ensure `stats` is a valid pointer and `out_event` is valid.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn engine_poll_event_ffi(
    stats: *const StatsQueueHandle,
    timeout_ms: u64,
    out_event_type: *mut u32,
    out_worker_id: *mut u64,
    out_query_id: *mut u64,
    out_pipeline_id_ptr: *mut *const u8,
    out_pipeline_id_len: *mut usize,
    out_to_pipeline_id_ptr: *mut *const u8,
    out_to_pipeline_id_len: *mut usize,
    out_task_id: *mut u64,
) -> bool {
    // Thread-local storage for pipeline ID strings to keep them alive until next call
    thread_local! {
        static PIPELINE_ID_BUF: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
        static TO_PIPELINE_ID_BUF: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }

    let Some(stats) = (unsafe { stats.as_ref() }) else {
        return false;
    };

    let receiver = stats.receiver.lock().unwrap();
    let event = if timeout_ms == 0 {
        receiver.try_recv().ok()
    } else {
        receiver
            .recv_timeout(std::time::Duration::from_millis(timeout_ms))
            .ok()
    };

    let Some(event) = event else {
        return false;
    };

    // Write out the event fields
    match event {
        StatisticsEvent::QueryStart {
            worker_id,
            query_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::QueryStart as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
        }
        StatisticsEvent::QueryStop {
            worker_id,
            query_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::QueryStop as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
        }
        StatisticsEvent::PipelineStart {
            worker_id,
            query_id,
            pipeline_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::PipelineStart as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
            PIPELINE_ID_BUF.with(|buf| {
                let mut buf = buf.borrow_mut();
                *buf = pipeline_id.to_string();
                unsafe { *out_pipeline_id_ptr = buf.as_ptr() };
                unsafe { *out_pipeline_id_len = buf.len() };
            });
        }
        StatisticsEvent::PipelineStop {
            worker_id,
            query_id,
            pipeline_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::PipelineStop as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
            PIPELINE_ID_BUF.with(|buf| {
                let mut buf = buf.borrow_mut();
                *buf = pipeline_id.to_string();
                unsafe { *out_pipeline_id_ptr = buf.as_ptr() };
                unsafe { *out_pipeline_id_len = buf.len() };
            });
        }
        StatisticsEvent::TaskExecutionStart {
            worker_id,
            query_id,
            pipeline_id,
            task_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::TaskExecutionStart as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
            unsafe { *out_task_id = task_id };
            PIPELINE_ID_BUF.with(|buf| {
                let mut buf = buf.borrow_mut();
                *buf = pipeline_id.to_string();
                unsafe { *out_pipeline_id_ptr = buf.as_ptr() };
                unsafe { *out_pipeline_id_len = buf.len() };
            });
        }
        StatisticsEvent::TaskExecutionComplete {
            worker_id,
            query_id,
            pipeline_id,
            task_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::TaskExecutionComplete as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
            unsafe { *out_task_id = task_id };
            PIPELINE_ID_BUF.with(|buf| {
                let mut buf = buf.borrow_mut();
                *buf = pipeline_id.to_string();
                unsafe { *out_pipeline_id_ptr = buf.as_ptr() };
                unsafe { *out_pipeline_id_len = buf.len() };
            });
        }
        StatisticsEvent::TaskEmit {
            worker_id,
            query_id,
            from_pipeline_id,
            to_pipeline_id,
            task_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::TaskEmit as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
            unsafe { *out_task_id = task_id };
            PIPELINE_ID_BUF.with(|buf| {
                let mut buf = buf.borrow_mut();
                *buf = from_pipeline_id.to_string();
                unsafe { *out_pipeline_id_ptr = buf.as_ptr() };
                unsafe { *out_pipeline_id_len = buf.len() };
            });
            TO_PIPELINE_ID_BUF.with(|buf| {
                let mut buf = buf.borrow_mut();
                *buf = to_pipeline_id.to_string();
                unsafe { *out_to_pipeline_id_ptr = buf.as_ptr() };
                unsafe { *out_to_pipeline_id_len = buf.len() };
            });
        }
        StatisticsEvent::QueryRunning {
            worker_id,
            query_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::QueryRunning as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
        }
        StatisticsEvent::QueryTerminated {
            worker_id,
            query_id,
        } => {
            unsafe { *out_event_type = FfiStatisticsEventType::QueryTerminated as u32 };
            unsafe { *out_worker_id = worker_id };
            unsafe { *out_query_id = query_id };
        }
        StatisticsEvent::SourceStarted { .. }
        | StatisticsEvent::PipelineExecutionError { .. }
        | StatisticsEvent::QueryError { .. } => {
            // Internal events consumed by QueryEngine, should not reach here.
            // Defensive: skip and report no event.
            return false;
        }
    }

    true
}

/// Free the stats queue handle (C FFI).
///
/// # Safety
/// The caller must ensure `stats` is a valid pointer returned by
/// `engine_create_with_stats_ffi` and that it hasn't already been freed.
#[cfg(feature = "cpp-ffi")]
#[no_mangle]
pub unsafe extern "C" fn stats_queue_destroy(stats: *mut StatsQueueHandle) {
    if !stats.is_null() {
        drop(unsafe { Box::from_raw(stats) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_create() {
        let engine = engine_create(0);
        assert_eq!(engine.context_ptr(), 0);
    }

    #[test]
    fn test_engine_stats_empty() {
        let engine = engine_create(0);
        let stats = engine_get_stats(&engine);
        assert_eq!(stats.buffers_processed, 0);
        assert_eq!(stats.tasks_executed, 0);
    }

    #[cfg(feature = "cpp-ffi")]
    #[test]
    fn test_engine_submit_query_empty() {
        let engine = engine_create(0);

        // Submit an empty query (no stages, no edges)
        let query_id = engine_submit_query(&engine, &[], &[], &[], &[], 0);

        // Should succeed with a valid query ID
        assert!(query_id > 0);
    }

    #[cfg(feature = "cpp-ffi")]
    #[test]
    fn test_engine_submit_query_unique_ids() {
        let engine = engine_create(0);

        // Submit multiple queries and verify unique IDs
        let id1 = engine_submit_query(&engine, &[], &[], &[], &[], 0);
        let id2 = engine_submit_query(&engine, &[], &[], &[], &[], 0);
        let id3 = engine_submit_query(&engine, &[], &[], &[], &[], 0);

        assert!(id1 > 0);
        assert!(id2 > 0);
        assert!(id3 > 0);
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[cfg(feature = "cpp-ffi")]
    #[test]
    fn test_ffi_engine_lifecycle() {
        // Test the raw C FFI functions
        unsafe {
            let engine = engine_create_ffi(12345);
            assert!(!engine.is_null());

            // Check buffer provider was stored
            assert_eq!((*engine).context_ptr(), 12345);

            // Get stats (should be empty)
            let mut buffers: u64 = 0;
            let mut bytes: u64 = 0;
            let mut tasks: u64 = 0;
            let mut queries: u64 = 0;
            let mut latency: f64 = 0.0;

            engine_get_stats_raw(
                engine,
                &mut buffers,
                &mut bytes,
                &mut tasks,
                &mut queries,
                &mut latency,
            );

            assert_eq!(buffers, 0);
            assert_eq!(tasks, 0);

            // Submit an empty query
            let query_id = engine_submit_query_raw(
                engine,
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
            );
            assert!(query_id > 0);

            // Clean up
            engine_destroy(engine);
        }
    }
}
