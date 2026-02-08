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

//! Rust-native engine API for lifecycle management.
//!
//! This module provides `Engine`, a high-level API for creating, running,
//! and shutting down the adaptive execution engine without going through FFI.
//!
//! The Engine layer tracks query-to-pipeline mappings, while the underlying
//! Executor operates on a single PipelineGraph without query awareness.
//!
//! # Examples
//!
//! ```no_run
//! use adaptive_engine::engine::Engine;
//! use adaptive_engine::graph::PipelineGraph;
//!
//! let mut engine = Engine::new();
//! engine.start();
//!
//! // Submit queries via engine.submit_query(graph)
//! // ...
//!
//! let stats = engine.shutdown();
//! ```

use crate::executor::stats::{StatisticsEvent, StatisticsSender};
use crate::executor::{ExecutionStats, Executor, ExecutorError, ExecutorHandle, QueryId};
use crate::graph::PipelineGraph;
use crate::pipeline::PipelineId;
use crate::query_engine::QueryEngine;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// High-level engine wrapping the executor with thread management.
///
/// `Engine` manages the executor lifecycle and tracks query-to-pipeline
/// mappings. The executor itself only knows about a single PipelineGraph;
/// the Engine maps QueryId → Vec<PipelineId> for stop_query support.
pub struct Engine {
    executor_handle: Option<ExecutorHandle>,
    executor: Option<Executor>,
    executor_thread: Option<JoinHandle<ExecutionStats>>,
    next_query_id: AtomicU64,
    /// Maps QueryId to the pipeline IDs belonging to that query.
    queries: Mutex<HashMap<QueryId, Vec<PipelineId>>>,
    /// Query engine for pipeline-to-query event mapping (Some when stats enabled).
    query_engine: Option<QueryEngine>,
}

impl Engine {
    /// Create a new engine without statistics collection.
    pub fn new() -> Self {
        Self::with_worker_count(1)
    }

    /// Create a new engine with the specified number of worker threads.
    pub fn with_worker_count(worker_count: usize) -> Self {
        let executor = Executor::with_worker_count(worker_count);
        let handle = executor.get_handle();
        Self {
            executor_handle: Some(handle),
            executor: Some(executor),
            executor_thread: None,
            next_query_id: AtomicU64::new(1),
            queries: Mutex::new(HashMap::new()),
            query_engine: None,
        }
    }

    /// Create a new engine with statistics collection enabled.
    ///
    /// Returns the engine and a `StatsReceiver` for polling statistics events.
    pub fn with_stats() -> (Self, StatsReceiver) {
        Self::with_worker_count_and_stats(1)
    }

    /// Create a new engine with the specified number of worker threads and
    /// statistics collection enabled.
    pub fn with_worker_count_and_stats(worker_count: usize) -> (Self, StatsReceiver) {
        // Two channels: raw (executor → query engine) and processed (query engine → consumer)
        let (raw_tx, raw_rx) = mpsc::channel::<StatisticsEvent>();
        let (processed_tx, processed_rx) = mpsc::channel::<StatisticsEvent>();

        let sender = StatisticsSender::new(raw_tx);
        let executor = Executor::with_worker_count_and_stats(worker_count, sender);
        let handle = executor.get_handle();

        let query_engine = QueryEngine::new(raw_rx, processed_tx, handle.clone());

        let engine = Self {
            executor_handle: Some(handle),
            executor: Some(executor),
            executor_thread: None,
            next_query_id: AtomicU64::new(1),
            queries: Mutex::new(HashMap::new()),
            query_engine: Some(query_engine),
        };

        let receiver = StatsReceiver {
            receiver: processed_rx,
        };

        (engine, receiver)
    }

    /// Start the executor thread.
    ///
    /// This spawns a background thread that processes tasks until shutdown.
    /// Must be called before submitting queries.
    pub fn start(&mut self) {
        let executor = self
            .executor
            .take()
            .expect("Engine already started or no executor available");

        let thread_handle = thread::spawn(move || executor.run());
        self.executor_thread = Some(thread_handle);
    }

    /// Submit a query (pipeline graph) for execution.
    ///
    /// Records all pipeline IDs from the graph, deploys it to the executor,
    /// and returns a QueryId that can be used to stop the query later.
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError` if the task queue lock is poisoned.
    pub fn submit_query(&self, graph: PipelineGraph) -> Result<QueryId, ExecutorError> {
        let query_id = self.next_query_id.fetch_add(1, Ordering::SeqCst);

        // Record all pipeline IDs for this query
        let pipeline_ids = graph.get_all_pipeline_ids();

        // Extract source IDs before deploying
        let source_ids: Vec<PipelineId> = pipeline_ids
            .iter()
            .filter(|id| graph.is_source(id))
            .cloned()
            .collect();

        {
            let mut queries = self.queries.lock().unwrap();
            queries.insert(query_id, pipeline_ids.clone());
        }

        // Register with QueryEngine if present
        if let Some(ref qe) = self.query_engine {
            qe.register_query(query_id, pipeline_ids, source_ids);
        }

        let handle = self
            .executor_handle
            .as_ref()
            .expect("Engine not initialized");
        handle.deploy_graph(graph)?;
        Ok(query_id)
    }

    /// Stop a specific query.
    ///
    /// Looks up the pipeline IDs for the given query and enqueues
    /// StopPipelineTask for each source pipeline via the executor.
    ///
    /// Returns `true` if the query was found and stop was initiated.
    pub fn stop_query(&self, query_id: QueryId) -> Result<bool, ExecutorError> {
        // Look up which pipelines belong to this query
        let pipeline_ids = {
            let queries = self.queries.lock().unwrap();
            match queries.get(&query_id) {
                Some(ids) => ids.clone(),
                None => return Ok(false),
            }
        };

        let handle = self
            .executor_handle
            .as_ref()
            .expect("Engine not initialized");

        // Find which of these pipelines are sources in the current graph
        let source_ids: Vec<PipelineId> = {
            let graph = handle
                .graph()
                .read()
                .map_err(|e| ExecutorError::TaskQueue(format!("Graph lock poisoned: {}", e)))?;

            pipeline_ids
                .iter()
                .filter(|id| graph.is_source(id))
                .cloned()
                .collect()
        };

        if source_ids.is_empty() {
            // Query has no source pipelines in the graph (may have already completed)
            // Remove from tracking
            let mut queries = self.queries.lock().unwrap();
            queries.remove(&query_id);
            return Ok(false);
        }

        handle.stop_pipelines(&source_ids)?;
        Ok(true)
    }

    /// Shutdown the engine and join the executor thread.
    ///
    /// Returns the final execution statistics.
    pub fn shutdown(mut self) -> ExecutionStats {
        // Signal shutdown
        if let Some(ref handle) = self.executor_handle {
            if let Err(e) = handle.shutdown() {
                eprintln!("Error signaling shutdown: {}", e);
            }
        }

        // Join the executor thread
        let stats = if let Some(thread_handle) = self.executor_thread.take() {
            match thread_handle.join() {
                Ok(stats) => stats,
                Err(e) => {
                    eprintln!("Executor thread panicked: {:?}", e);
                    ExecutionStats::default()
                }
            }
        } else {
            ExecutionStats::default()
        };

        // Drop the executor handle to close the raw stats channel,
        // which signals the QueryEngine processing thread to exit.
        self.executor_handle.take();

        // Stop the QueryEngine processing thread
        if let Some(qe) = self.query_engine.take() {
            qe.stop();
        }

        stats
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Receiver end of the statistics channel.
///
/// Provides methods to poll and collect statistics events emitted
/// by the executor during query execution.
pub struct StatsReceiver {
    receiver: mpsc::Receiver<StatisticsEvent>,
}

impl StatsReceiver {
    /// Poll for a single statistics event with a timeout.
    ///
    /// Returns `None` if no event is available within the timeout.
    pub fn poll(&self, timeout: Duration) -> Option<StatisticsEvent> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Try to receive a single event without blocking.
    pub fn try_recv(&self) -> Option<StatisticsEvent> {
        self.receiver.try_recv().ok()
    }

    /// Collect all currently available events (non-blocking).
    pub fn drain(&self) -> Vec<StatisticsEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// Collect events for a given duration, then drain remaining.
    pub fn collect(&self, timeout: Duration) -> Vec<StatisticsEvent> {
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.receiver.recv_timeout(remaining) {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_create_and_shutdown() {
        let mut engine = Engine::new();
        engine.start();
        let stats = engine.shutdown();
        assert_eq!(stats.buffers_processed, 0);
    }

    #[test]
    fn test_engine_with_stats() {
        let (mut engine, _receiver) = Engine::with_stats();
        engine.start();
        let stats = engine.shutdown();
        assert_eq!(stats.buffers_processed, 0);
    }

    #[test]
    fn test_engine_submit_empty_query() {
        let mut engine = Engine::new();
        engine.start();

        let graph = PipelineGraph::new();
        let query_id = engine.submit_query(graph).unwrap();
        assert!(query_id > 0);

        let _stats = engine.shutdown();
    }
}
