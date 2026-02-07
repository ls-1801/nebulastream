//! Rust-native engine API for lifecycle management.
//!
//! This module provides `Engine`, a high-level API for creating, running,
//! and shutting down the adaptive execution engine without going through FFI.
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
use crate::executor::{
    ExecutionStats, Executor, ExecutorError, ExecutorHandle, FifoQueue, QueryId,
};
use crate::graph::PipelineGraph;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// High-level engine wrapping the executor with thread management.
///
/// `Engine` manages the executor lifecycle: creating the executor,
/// spawning the execution thread, submitting queries, and shutting down.
pub struct Engine {
    executor_handle: Option<ExecutorHandle>,
    executor: Option<Executor>,
    executor_thread: Option<JoinHandle<ExecutionStats>>,
    next_query_id: AtomicU64,
}

impl Engine {
    /// Create a new engine without statistics collection.
    pub fn new() -> Self {
        let executor = Executor::new();
        let handle = executor.get_handle();
        Self {
            executor_handle: Some(handle),
            executor: Some(executor),
            executor_thread: None,
            next_query_id: AtomicU64::new(1),
        }
    }

    /// Create a new engine with statistics collection enabled.
    ///
    /// Returns the engine and a `StatsReceiver` for polling statistics events.
    pub fn with_stats() -> (Self, StatsReceiver) {
        let (tx, rx) = mpsc::channel::<StatisticsEvent>();
        let sender = StatisticsSender::new(tx);
        let executor = Executor::with_queue_and_stats(FifoQueue::new(), sender);
        let handle = executor.get_handle();

        let engine = Self {
            executor_handle: Some(handle),
            executor: Some(executor),
            executor_thread: None,
            next_query_id: AtomicU64::new(1),
        };

        let receiver = StatsReceiver { receiver: rx };

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
    /// Returns a `QueryId` that can be used to stop the query later.
    ///
    /// # Errors
    ///
    /// Returns `ExecutorError` if the task queue lock is poisoned.
    pub fn submit_query(&self, graph: PipelineGraph) -> Result<QueryId, ExecutorError> {
        let query_id = self.next_query_id.fetch_add(1, Ordering::SeqCst);
        let handle = self
            .executor_handle
            .as_ref()
            .expect("Engine not initialized");
        handle.deploy_graph_with_query_id(query_id, graph)?;
        Ok(query_id)
    }

    /// Stop a specific query.
    ///
    /// Returns `true` if the query was found and stop was initiated.
    pub fn stop_query(&self, query_id: QueryId) -> Result<bool, ExecutorError> {
        let handle = self
            .executor_handle
            .as_ref()
            .expect("Engine not initialized");
        handle.stop_query(query_id)
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
        if let Some(thread_handle) = self.executor_thread.take() {
            match thread_handle.join() {
                Ok(stats) => stats,
                Err(e) => {
                    eprintln!("Executor thread panicked: {:?}", e);
                    ExecutionStats::default()
                }
            }
        } else {
            ExecutionStats::default()
        }
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
