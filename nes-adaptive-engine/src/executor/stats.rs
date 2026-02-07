//! Statistics event types for query execution observability.
//!
//! This module provides a `StatisticsEvent` enum that captures lifecycle events
//! during query execution. These events can be used for:
//! - Test assertions (verify execution order, count pipelines started/stopped)
//! - Performance monitoring (track task execution timing)
//! - Debugging (understand execution flow)
//!
//! # Design Decisions
//!
//! - **Unbounded channel**: Statistics events use an unbounded channel to avoid
//!   back-pressure affecting the execution hot path. Events are best-effort
//!   diagnostics, not critical-path data.
//!
//! - **Optional listener**: If no channel is provided, events are simply not emitted.
//!   This adds zero overhead when statistics aren't needed.
//!
//! - **No ordering guarantees**: Events may arrive out of execution order due to
//!   concurrent sources and the single-threaded executor processing tasks from
//!   an abstract queue.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::mpsc;
//! use adaptive_engine::executor::stats::{StatisticsEvent, StatisticsSender};
//!
//! // Create channel for receiving events
//! let (tx, rx) = mpsc::channel::<StatisticsEvent>();
//! let sender = StatisticsSender::new(tx);
//!
//! // Pass sender to executor... (see Engine API)
//!
//! // Collect events in test
//! let mut events = Vec::new();
//! while let Ok(event) = rx.try_recv() {
//!     events.push(event);
//! }
//! ```

use crate::executor::QueryId;
use crate::pipeline::PipelineId;
use std::sync::mpsc::Sender;

/// Unique identifier for a task within the executor.
///
/// Used to correlate TaskExecutionStart with TaskExecutionComplete events.
pub type TaskId = u64;

/// Worker thread identifier.
///
/// For the single-threaded executor, this is always 0.
pub type WorkerId = u64;

/// Statistics event capturing execution lifecycle events.
///
/// Events are emitted at key points during query execution to enable
/// observability and test assertions. Each variant includes relevant
/// context like query ID, pipeline ID, and worker ID.
#[derive(Debug, Clone)]
pub enum StatisticsEvent {
    /// Emitted when a query starts executing (graph deployed).
    ///
    /// Occurs once per `deploy_graph()` call when the graph is successfully
    /// added to the executor's query map.
    QueryStart {
        /// Worker thread that deployed the query (always 0 for single-threaded executor).
        worker_id: WorkerId,
        /// Unique identifier for the query.
        query_id: QueryId,
    },

    /// Emitted when a query stops (all pipelines stopped).
    ///
    /// Occurs when the last pipeline in a query is torn down, either due to
    /// normal completion (all sources sent EOS) or explicit stop.
    QueryStop {
        /// Worker thread that processed the stop (always 0 for single-threaded executor).
        worker_id: WorkerId,
        /// Unique identifier for the query.
        query_id: QueryId,
    },

    /// Emitted when a pipeline starts (setup succeeds).
    ///
    /// Occurs once per pipeline during graph deployment when `setup()` succeeds.
    PipelineStart {
        /// Worker thread that started the pipeline.
        worker_id: WorkerId,
        /// Query this pipeline belongs to.
        query_id: QueryId,
        /// Unique identifier for the pipeline.
        pipeline_id: PipelineId,
    },

    /// Emitted when a pipeline stops (teardown called).
    ///
    /// Occurs once per pipeline when it's torn down, after flush() and before
    /// removal from metadata.
    PipelineStop {
        /// Worker thread that stopped the pipeline.
        worker_id: WorkerId,
        /// Query this pipeline belongs to.
        query_id: QueryId,
        /// Unique identifier for the pipeline.
        pipeline_id: PipelineId,
    },

    /// Emitted when a task starts executing.
    ///
    /// Occurs before calling `pipeline.execute()` for each buffer.
    TaskExecutionStart {
        /// Worker thread executing the task.
        worker_id: WorkerId,
        /// Query this task belongs to.
        query_id: QueryId,
        /// Pipeline processing the buffer.
        pipeline_id: PipelineId,
        /// Unique identifier for this task execution.
        task_id: TaskId,
    },

    /// Emitted when a task completes execution.
    ///
    /// Occurs after `pipeline.execute()` returns (success or failure).
    TaskExecutionComplete {
        /// Worker thread that executed the task.
        worker_id: WorkerId,
        /// Query this task belongs to.
        query_id: QueryId,
        /// Pipeline that processed the buffer.
        pipeline_id: PipelineId,
        /// Unique identifier matching the TaskExecutionStart.
        task_id: TaskId,
    },

    /// Emitted when a task emits data to downstream pipelines.
    ///
    /// Occurs when buffers are routed from one pipeline to its successors.
    TaskEmit {
        /// Worker thread handling the emit.
        worker_id: WorkerId,
        /// Query this task belongs to.
        query_id: QueryId,
        /// Pipeline that emitted the buffers.
        from_pipeline_id: PipelineId,
        /// Pipeline receiving the buffers.
        to_pipeline_id: PipelineId,
        /// Task that produced the emitted buffers.
        task_id: TaskId,
    },

    /// Emitted when a query becomes fully operational.
    ///
    /// Occurs after all pipelines have been started and all sources have been
    /// opened (i.e., the query is ready to process data).
    QueryRunning {
        /// Worker thread that completed query startup.
        worker_id: WorkerId,
        /// Unique identifier for the query.
        query_id: QueryId,
    },

    /// Emitted when a query has fully terminated.
    ///
    /// Occurs after all pipelines for a query have been stopped and all
    /// cleanup is complete. This is the counterpart to QueryRunning.
    QueryTerminated {
        /// Worker thread that completed query termination.
        worker_id: WorkerId,
        /// Unique identifier for the query.
        query_id: QueryId,
    },
}

/// Handle for sending statistics events to a listener.
///
/// Wraps an optional `Sender<StatisticsEvent>` channel. If no channel is
/// provided, all emit calls are no-ops.
///
/// This type is Clone and can be passed to multiple components of the
/// executor that need to emit events.
#[derive(Clone)]
pub struct StatisticsSender {
    /// The underlying channel sender, if any.
    sender: Option<Sender<StatisticsEvent>>,
}

impl StatisticsSender {
    /// Create a new statistics sender with the given channel.
    ///
    /// # Arguments
    /// * `sender` - The channel sender for statistics events
    pub fn new(sender: Sender<StatisticsEvent>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// Create a no-op statistics sender that discards all events.
    ///
    /// Use this when statistics collection is not needed.
    pub fn noop() -> Self {
        Self { sender: None }
    }

    /// Check if this sender has a listener attached.
    ///
    /// Returns `true` if events will be sent, `false` if they will be discarded.
    pub fn has_listener(&self) -> bool {
        self.sender.is_some()
    }

    /// Emit a statistics event.
    ///
    /// If no listener is attached, this is a no-op.
    /// If the channel is disconnected, the error is silently ignored.
    pub fn emit(&self, event: StatisticsEvent) {
        if let Some(ref sender) = self.sender {
            // Ignore send errors - the receiver may have been dropped
            let _ = sender.send(event);
        }
    }

    /// Emit a QueryStart event.
    #[inline]
    pub fn query_start(&self, worker_id: WorkerId, query_id: QueryId) {
        self.emit(StatisticsEvent::QueryStart {
            worker_id,
            query_id,
        });
    }

    /// Emit a QueryStop event.
    #[inline]
    pub fn query_stop(&self, worker_id: WorkerId, query_id: QueryId) {
        self.emit(StatisticsEvent::QueryStop {
            worker_id,
            query_id,
        });
    }

    /// Emit a PipelineStart event.
    #[inline]
    pub fn pipeline_start(&self, worker_id: WorkerId, query_id: QueryId, pipeline_id: PipelineId) {
        self.emit(StatisticsEvent::PipelineStart {
            worker_id,
            query_id,
            pipeline_id,
        });
    }

    /// Emit a PipelineStop event.
    #[inline]
    pub fn pipeline_stop(&self, worker_id: WorkerId, query_id: QueryId, pipeline_id: PipelineId) {
        self.emit(StatisticsEvent::PipelineStop {
            worker_id,
            query_id,
            pipeline_id,
        });
    }

    /// Emit a TaskExecutionStart event.
    #[inline]
    pub fn task_execution_start(
        &self,
        worker_id: WorkerId,
        query_id: QueryId,
        pipeline_id: PipelineId,
        task_id: TaskId,
    ) {
        self.emit(StatisticsEvent::TaskExecutionStart {
            worker_id,
            query_id,
            pipeline_id,
            task_id,
        });
    }

    /// Emit a TaskExecutionComplete event.
    #[inline]
    pub fn task_execution_complete(
        &self,
        worker_id: WorkerId,
        query_id: QueryId,
        pipeline_id: PipelineId,
        task_id: TaskId,
    ) {
        self.emit(StatisticsEvent::TaskExecutionComplete {
            worker_id,
            query_id,
            pipeline_id,
            task_id,
        });
    }

    /// Emit a TaskEmit event.
    #[inline]
    pub fn task_emit(
        &self,
        worker_id: WorkerId,
        query_id: QueryId,
        from_pipeline_id: PipelineId,
        to_pipeline_id: PipelineId,
        task_id: TaskId,
    ) {
        self.emit(StatisticsEvent::TaskEmit {
            worker_id,
            query_id,
            from_pipeline_id,
            to_pipeline_id,
            task_id,
        });
    }

    /// Emit a QueryRunning event.
    #[inline]
    pub fn query_running(&self, worker_id: WorkerId, query_id: QueryId) {
        self.emit(StatisticsEvent::QueryRunning {
            worker_id,
            query_id,
        });
    }

    /// Emit a QueryTerminated event.
    #[inline]
    pub fn query_terminated(&self, worker_id: WorkerId, query_id: QueryId) {
        self.emit(StatisticsEvent::QueryTerminated {
            worker_id,
            query_id,
        });
    }
}

impl Default for StatisticsSender {
    fn default() -> Self {
        Self::noop()
    }
}

impl std::fmt::Debug for StatisticsSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatisticsSender")
            .field("has_listener", &self.has_listener())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_noop_sender_does_not_panic() {
        let sender = StatisticsSender::noop();
        assert!(!sender.has_listener());

        // Should not panic even with no listener
        sender.query_start(0, 1);
        sender.query_stop(0, 1);
        sender.pipeline_start(0, 1, PipelineId::new("test"));
        sender.pipeline_stop(0, 1, PipelineId::new("test"));
        sender.task_execution_start(0, 1, PipelineId::new("test"), 100);
        sender.task_execution_complete(0, 1, PipelineId::new("test"), 100);
        sender.task_emit(0, 1, PipelineId::new("from"), PipelineId::new("to"), 100);
    }

    #[test]
    fn test_sender_with_channel_delivers_events() {
        let (tx, rx) = mpsc::channel();
        let sender = StatisticsSender::new(tx);
        assert!(sender.has_listener());

        sender.query_start(0, 42);
        sender.pipeline_start(0, 42, PipelineId::new("pipe1"));
        sender.task_execution_start(0, 42, PipelineId::new("pipe1"), 1);
        sender.task_execution_complete(0, 42, PipelineId::new("pipe1"), 1);
        sender.pipeline_stop(0, 42, PipelineId::new("pipe1"));
        sender.query_stop(0, 42);

        // Verify events were received
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        assert_eq!(events.len(), 6);

        // Verify event order and content
        assert!(matches!(
            events[0],
            StatisticsEvent::QueryStart { query_id: 42, .. }
        ));
        assert!(matches!(
            events[1],
            StatisticsEvent::PipelineStart { query_id: 42, .. }
        ));
        assert!(matches!(
            events[2],
            StatisticsEvent::TaskExecutionStart { task_id: 1, .. }
        ));
        assert!(matches!(
            events[3],
            StatisticsEvent::TaskExecutionComplete { task_id: 1, .. }
        ));
        assert!(matches!(
            events[4],
            StatisticsEvent::PipelineStop { query_id: 42, .. }
        ));
        assert!(matches!(
            events[5],
            StatisticsEvent::QueryStop { query_id: 42, .. }
        ));
    }

    #[test]
    fn test_sender_handles_dropped_receiver() {
        let (tx, rx) = mpsc::channel();
        let sender = StatisticsSender::new(tx);

        // Drop receiver
        drop(rx);

        // Should not panic even with dropped receiver
        sender.query_start(0, 1);
        sender.query_stop(0, 1);
    }

    #[test]
    fn test_task_emit_event() {
        let (tx, rx) = mpsc::channel();
        let sender = StatisticsSender::new(tx);

        sender.task_emit(0, 1, PipelineId::new("source"), PipelineId::new("sink"), 42);

        let event = rx.recv().unwrap();
        match event {
            StatisticsEvent::TaskEmit {
                worker_id,
                query_id,
                from_pipeline_id,
                to_pipeline_id,
                task_id,
            } => {
                assert_eq!(worker_id, 0);
                assert_eq!(query_id, 1);
                assert_eq!(from_pipeline_id.to_string(), "source");
                assert_eq!(to_pipeline_id.to_string(), "sink");
                assert_eq!(task_id, 42);
            }
            _ => panic!("Expected TaskEmit event"),
        }
    }
}
