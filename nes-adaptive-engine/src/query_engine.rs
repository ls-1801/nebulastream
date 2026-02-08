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

//! Query-level event mapping layer.
//!
//! The executor operates on a single `PipelineGraph` with no query concept.
//! This module sits between the executor and event consumers, intercepting
//! raw pipeline-level events, mapping `pipeline_id → query_id`, and
//! synthesizing query-level events (QueryStart, QueryRunning, QueryStop,
//! QueryTerminated) with correct query IDs.
//!
//! ```text
//! Executor → [raw channel] → QueryEngine thread → [processed channel] → Consumer
//!                                 ↑
//!                           register_query()
//!                           (from submit_query)
//! ```

use crate::executor::stats::StatisticsEvent;
use crate::executor::{ExecutorHandle, QueryId};
use crate::pipeline::PipelineId;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Per-query lifecycle state.
struct QueryState {
    /// All pipeline IDs belonging to this query.
    pipeline_ids: HashSet<PipelineId>,
    /// Source pipeline IDs belonging to this query.
    source_ids: HashSet<PipelineId>,
    /// Sources that have been successfully started.
    sources_started: HashSet<PipelineId>,
    /// Pipelines that have been stopped.
    pipelines_stopped: HashSet<PipelineId>,
    /// Whether QueryRunning has been emitted.
    is_running: bool,
    /// Whether QueryTerminated has been emitted.
    is_terminated: bool,
    /// Whether a stop has already been issued for this query (error handling).
    stop_issued: bool,
}

/// Maps pipeline IDs to queries and tracks per-query lifecycle.
struct QueryTracker {
    /// Maps pipeline_id → query_id for event rewriting.
    pipeline_to_query: HashMap<PipelineId, QueryId>,
    /// Per-query state.
    queries: HashMap<QueryId, QueryState>,
}

impl QueryTracker {
    fn new() -> Self {
        Self {
            pipeline_to_query: HashMap::new(),
            queries: HashMap::new(),
        }
    }

    /// Register a new query with its pipeline and source IDs.
    fn register_query(
        &mut self,
        query_id: QueryId,
        pipeline_ids: Vec<PipelineId>,
        source_ids: Vec<PipelineId>,
    ) {
        for pid in &pipeline_ids {
            self.pipeline_to_query.insert(pid.clone(), query_id);
        }

        self.queries.insert(
            query_id,
            QueryState {
                pipeline_ids: pipeline_ids.into_iter().collect(),
                source_ids: source_ids.into_iter().collect(),
                sources_started: HashSet::new(),
                pipelines_stopped: HashSet::new(),
                is_running: false,
                is_terminated: false,
                stop_issued: false,
            },
        );
    }

    /// Look up the query_id for a pipeline_id.
    fn lookup_query(&self, pipeline_id: &PipelineId) -> QueryId {
        self.pipeline_to_query
            .get(pipeline_id)
            .copied()
            .unwrap_or(0)
    }

    /// Process a raw event, returning the events to forward to consumers.
    fn process(&mut self, event: StatisticsEvent) -> Vec<StatisticsEvent> {
        match event {
            StatisticsEvent::PipelineStart {
                worker_id,
                pipeline_id,
                ..
            } => {
                let qid = self.lookup_query(&pipeline_id);
                vec![StatisticsEvent::PipelineStart {
                    worker_id,
                    query_id: qid,
                    pipeline_id,
                }]
            }

            StatisticsEvent::PipelineStop {
                worker_id,
                pipeline_id,
                ..
            } => {
                let qid = self.lookup_query(&pipeline_id);
                let mut result = vec![StatisticsEvent::PipelineStop {
                    worker_id,
                    query_id: qid,
                    pipeline_id: pipeline_id.clone(),
                }];

                // Track pipeline stop and check if all pipelines for this query stopped
                if let Some(state) = self.queries.get_mut(&qid) {
                    state.pipelines_stopped.insert(pipeline_id);
                    if !state.is_terminated
                        && state.pipelines_stopped.len() == state.pipeline_ids.len()
                    {
                        state.is_terminated = true;
                        result.push(StatisticsEvent::QueryStop {
                            worker_id,
                            query_id: qid,
                        });
                        result.push(StatisticsEvent::QueryTerminated {
                            worker_id,
                            query_id: qid,
                        });
                    }
                }

                result
            }

            StatisticsEvent::SourceStarted {
                worker_id,
                source_id,
            } => {
                let qid = self.lookup_query(&source_id);

                // Track source start and check if all sources for this query started
                if let Some(state) = self.queries.get_mut(&qid) {
                    state.sources_started.insert(source_id);
                    if !state.is_running && state.sources_started.len() == state.source_ids.len() {
                        state.is_running = true;
                        return vec![StatisticsEvent::QueryRunning {
                            worker_id,
                            query_id: qid,
                        }];
                    }
                }

                // SourceStarted is consumed, not forwarded
                vec![]
            }

            StatisticsEvent::TaskExecutionStart {
                worker_id,
                pipeline_id,
                task_id,
                ..
            } => {
                let qid = self.lookup_query(&pipeline_id);
                vec![StatisticsEvent::TaskExecutionStart {
                    worker_id,
                    query_id: qid,
                    pipeline_id,
                    task_id,
                }]
            }

            StatisticsEvent::TaskExecutionComplete {
                worker_id,
                pipeline_id,
                task_id,
                ..
            } => {
                let qid = self.lookup_query(&pipeline_id);
                vec![StatisticsEvent::TaskExecutionComplete {
                    worker_id,
                    query_id: qid,
                    pipeline_id,
                    task_id,
                }]
            }

            StatisticsEvent::TaskEmit {
                worker_id,
                from_pipeline_id,
                to_pipeline_id,
                task_id,
                ..
            } => {
                let qid = self.lookup_query(&from_pipeline_id);
                vec![StatisticsEvent::TaskEmit {
                    worker_id,
                    query_id: qid,
                    from_pipeline_id,
                    to_pipeline_id,
                    task_id,
                }]
            }

            StatisticsEvent::PipelineExecutionError { pipeline_id, .. } => {
                // A pipeline failed — mark the query for stop.
                // The actual stop_pipelines call is made by the processing loop.
                let qid = self.lookup_query(&pipeline_id);
                if let Some(state) = self.queries.get_mut(&qid) {
                    if !state.stop_issued {
                        state.stop_issued = true;
                        // Return a sentinel to signal the processing loop to issue the stop.
                        // We reuse the event so the loop can extract the query_id.
                        return vec![StatisticsEvent::PipelineExecutionError {
                            worker_id: 0,
                            pipeline_id,
                        }];
                    }
                }
                vec![]
            }

            // Query-level events should not come from the executor anymore,
            // but pass them through defensively.
            other => vec![other],
        }
    }

    /// Get the source pipeline IDs for a query.
    fn get_source_ids(&self, query_id: QueryId) -> Vec<PipelineId> {
        self.queries
            .get(&query_id)
            .map(|state| state.source_ids.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all pipeline IDs for a query.
    fn get_pipeline_ids(&self, query_id: QueryId) -> Vec<PipelineId> {
        self.queries
            .get(&query_id)
            .map(|state| state.pipeline_ids.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Emit QueryStop + QueryTerminated for all active (non-terminated) queries.
    /// Called when the raw channel closes (executor exited).
    fn terminate_remaining(&mut self) -> Vec<StatisticsEvent> {
        let mut result = Vec::new();
        for (&qid, state) in &mut self.queries {
            if !state.is_terminated {
                state.is_terminated = true;
                result.push(StatisticsEvent::QueryStop {
                    worker_id: 0,
                    query_id: qid,
                });
                result.push(StatisticsEvent::QueryTerminated {
                    worker_id: 0,
                    query_id: qid,
                });
            }
        }
        result
    }
}

/// Sits between executor and consumers, adding query context to events.
pub struct QueryEngine {
    tracker: Arc<Mutex<QueryTracker>>,
    processed_tx: mpsc::Sender<StatisticsEvent>,
    processing_thread: Option<JoinHandle<()>>,
}

impl QueryEngine {
    /// Create a new QueryEngine that reads raw events and writes processed events.
    ///
    /// Spawns a processing thread that runs until the raw channel disconnects.
    /// The `executor_handle` is used to issue `stop_pipelines()` on query errors.
    pub fn new(
        raw_rx: mpsc::Receiver<StatisticsEvent>,
        processed_tx: mpsc::Sender<StatisticsEvent>,
        executor_handle: ExecutorHandle,
    ) -> Self {
        let tracker = Arc::new(Mutex::new(QueryTracker::new()));
        let tracker_clone = Arc::clone(&tracker);
        let tx_clone = processed_tx.clone();
        let handle_clone = executor_handle.clone();

        let processing_thread = std::thread::Builder::new()
            .name("nes-query-engine".to_string())
            .spawn(move || {
                Self::processing_loop(raw_rx, tx_clone, tracker_clone, handle_clone);
            })
            .expect("Failed to spawn query engine thread");

        Self {
            tracker,
            processed_tx,
            processing_thread: Some(processing_thread),
        }
    }

    /// Register a query with its pipeline and source IDs.
    ///
    /// This also immediately injects a QueryStart event into the processed channel.
    /// If the query has no sources, a QueryRunning event is also emitted immediately.
    pub fn register_query(
        &self,
        query_id: QueryId,
        pipeline_ids: Vec<PipelineId>,
        source_ids: Vec<PipelineId>,
    ) {
        let no_sources = source_ids.is_empty();

        {
            let mut tracker = self.tracker.lock().unwrap();
            tracker.register_query(query_id, pipeline_ids, source_ids);
        }

        // Inject QueryStart into the processed channel
        let _ = self.processed_tx.send(StatisticsEvent::QueryStart {
            worker_id: 0,
            query_id,
        });

        // If no sources, the query is immediately "running"
        if no_sources {
            let _ = self.processed_tx.send(StatisticsEvent::QueryRunning {
                worker_id: 0,
                query_id,
            });
        }
    }

    /// Stop the processing thread (call after executor thread exits).
    pub fn stop(mut self) {
        if let Some(handle) = self.processing_thread.take() {
            let _ = handle.join();
        }
    }

    /// The processing loop run on the background thread.
    fn processing_loop(
        raw_rx: mpsc::Receiver<StatisticsEvent>,
        processed_tx: mpsc::Sender<StatisticsEvent>,
        tracker: Arc<Mutex<QueryTracker>>,
        executor_handle: ExecutorHandle,
    ) {
        loop {
            match raw_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    let events = tracker.lock().unwrap().process(event);
                    for e in events {
                        // If this is a PipelineExecutionError, mark all query pipelines
                        // as failed and issue stop_pipelines for sources.
                        if let StatisticsEvent::PipelineExecutionError {
                            ref pipeline_id, ..
                        } = e
                        {
                            let (source_ids, all_pipeline_ids) = {
                                let t = tracker.lock().unwrap();
                                let qid = t.lookup_query(pipeline_id);
                                (t.get_source_ids(qid), t.get_pipeline_ids(qid))
                            };

                            // Mark ALL pipelines in the query as failed so they
                            // skip flush during the stop cascade.
                            {
                                let graph = executor_handle.graph().read().unwrap();
                                for pid in &all_pipeline_ids {
                                    if let Some(node) = graph.get_node(pid) {
                                        node.metadata().mark_failed();
                                    }
                                }
                            }

                            if !source_ids.is_empty() {
                                let _ = executor_handle.stop_pipelines(&source_ids);
                            }
                            // Don't forward PipelineExecutionError to consumers
                            continue;
                        }
                        if processed_tx.send(e).is_err() {
                            return;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Executor exited — terminate remaining queries
                    let events = tracker.lock().unwrap().terminate_remaining();
                    for e in events {
                        let _ = processed_tx.send(e);
                    }
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;

    /// Create a dummy ExecutorHandle for tests that don't need a running executor.
    fn dummy_executor_handle() -> ExecutorHandle {
        Executor::new().get_handle()
    }

    #[test]
    fn test_query_tracker_register_and_lookup() {
        let mut tracker = QueryTracker::new();
        let pid1 = PipelineId::new("p1");
        let pid2 = PipelineId::new("p2");

        tracker.register_query(42, vec![pid1.clone(), pid2.clone()], vec![pid1.clone()]);

        assert_eq!(tracker.lookup_query(&pid1), 42);
        assert_eq!(tracker.lookup_query(&pid2), 42);
        assert_eq!(tracker.lookup_query(&PipelineId::new("unknown")), 0);
    }

    #[test]
    fn test_pipeline_start_rewrite() {
        let mut tracker = QueryTracker::new();
        let pid = PipelineId::new("p1");
        tracker.register_query(5, vec![pid.clone()], vec![]);

        let events = tracker.process(StatisticsEvent::PipelineStart {
            worker_id: 0,
            query_id: 0,
            pipeline_id: pid.clone(),
        });

        assert_eq!(events.len(), 1);
        match &events[0] {
            StatisticsEvent::PipelineStart { query_id, .. } => assert_eq!(*query_id, 5),
            _ => panic!("Expected PipelineStart"),
        }
    }

    #[test]
    fn test_source_started_synthesizes_query_running() {
        let mut tracker = QueryTracker::new();
        let src1 = PipelineId::new("s1");
        let src2 = PipelineId::new("s2");
        let pipe = PipelineId::new("p1");

        tracker.register_query(
            10,
            vec![src1.clone(), src2.clone(), pipe.clone()],
            vec![src1.clone(), src2.clone()],
        );

        // First source started — no QueryRunning yet
        let events = tracker.process(StatisticsEvent::SourceStarted {
            worker_id: 0,
            source_id: src1,
        });
        assert!(events.is_empty());

        // Second source started — QueryRunning emitted
        let events = tracker.process(StatisticsEvent::SourceStarted {
            worker_id: 0,
            source_id: src2,
        });
        assert_eq!(events.len(), 1);
        match &events[0] {
            StatisticsEvent::QueryRunning { query_id, .. } => assert_eq!(*query_id, 10),
            _ => panic!("Expected QueryRunning"),
        }
    }

    #[test]
    fn test_pipeline_stop_synthesizes_query_terminated() {
        let mut tracker = QueryTracker::new();
        let pid1 = PipelineId::new("p1");
        let pid2 = PipelineId::new("p2");

        tracker.register_query(7, vec![pid1.clone(), pid2.clone()], vec![]);

        // First pipeline stop — no QueryTerminated yet
        let events = tracker.process(StatisticsEvent::PipelineStop {
            worker_id: 0,
            query_id: 0,
            pipeline_id: pid1,
        });
        assert_eq!(events.len(), 1); // Just PipelineStop

        // Second pipeline stop — QueryStop + QueryTerminated
        let events = tracker.process(StatisticsEvent::PipelineStop {
            worker_id: 0,
            query_id: 0,
            pipeline_id: pid2,
        });
        assert_eq!(events.len(), 3); // PipelineStop + QueryStop + QueryTerminated
        assert!(matches!(
            events[1],
            StatisticsEvent::QueryStop { query_id: 7, .. }
        ));
        assert!(matches!(
            events[2],
            StatisticsEvent::QueryTerminated { query_id: 7, .. }
        ));
    }

    #[test]
    fn test_terminate_remaining() {
        let mut tracker = QueryTracker::new();
        tracker.register_query(1, vec![PipelineId::new("p1")], vec![]);
        tracker.register_query(2, vec![PipelineId::new("p2")], vec![]);

        let events = tracker.terminate_remaining();
        assert_eq!(events.len(), 4); // 2 queries × (QueryStop + QueryTerminated)
    }

    #[test]
    fn test_query_engine_end_to_end() {
        let (raw_tx, raw_rx) = mpsc::channel();
        let (processed_tx, processed_rx) = mpsc::channel();

        let engine = QueryEngine::new(raw_rx, processed_tx, dummy_executor_handle());

        let src = PipelineId::new("src");
        let pipe = PipelineId::new("pipe");
        engine.register_query(42, vec![src.clone(), pipe.clone()], vec![src.clone()]);

        // Should get QueryStart from register_query
        let event = processed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            StatisticsEvent::QueryStart { query_id: 42, .. }
        ));

        // Send PipelineStart through raw channel
        raw_tx
            .send(StatisticsEvent::PipelineStart {
                worker_id: 0,
                query_id: 0,
                pipeline_id: src.clone(),
            })
            .unwrap();

        let event = processed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match event {
            StatisticsEvent::PipelineStart { query_id, .. } => assert_eq!(query_id, 42),
            _ => panic!("Expected PipelineStart"),
        }

        // Send SourceStarted — should synthesize QueryRunning
        raw_tx
            .send(StatisticsEvent::SourceStarted {
                worker_id: 0,
                source_id: src,
            })
            .unwrap();

        let event = processed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            StatisticsEvent::QueryRunning { query_id: 42, .. }
        ));

        // Drop raw_tx to close the channel — should trigger terminate_remaining
        drop(raw_tx);

        // The pipe hasn't stopped yet, so terminate_remaining should emit QueryStop + QueryTerminated
        let event = processed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            event,
            StatisticsEvent::QueryStop { query_id: 42, .. }
        ));
        let event = processed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            StatisticsEvent::QueryTerminated { query_id: 42, .. }
        ));

        engine.stop();
    }

    #[test]
    fn test_no_source_query_immediately_running() {
        let (raw_tx, raw_rx) = mpsc::channel();
        let (processed_tx, processed_rx) = mpsc::channel();

        let engine = QueryEngine::new(raw_rx, processed_tx, dummy_executor_handle());

        let pipe = PipelineId::new("pipe");
        engine.register_query(1, vec![pipe], vec![]);

        // Should get QueryStart then QueryRunning immediately
        let event = processed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            StatisticsEvent::QueryStart { query_id: 1, .. }
        ));

        let event = processed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            StatisticsEvent::QueryRunning { query_id: 1, .. }
        ));

        // Drop raw_tx to close the channel so stop() can join the processing thread
        drop(raw_tx);
        engine.stop();
    }
}
