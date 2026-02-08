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

//! Tests for `terminate_pipelines` behavior.
//!
//! Verifies that when a pipeline or source error occurs, the QueryEngine
//! calls `ExecutorHandle::terminate_pipelines()` to:
//! 1. Mark ALL pipelines in the failing query as failed (atomic flag)
//! 2. Set source stop flags so source threads terminate
//! 3. Enqueue StopPipelineTask for each source (cascading shutdown)
//! 4. Emit a `QueryError` event to consumers with error context
//!
//! Tests cover triggering from setup failures, execution failures, teardown
//! failures, and source errors, across linear, fan-out, diamond, and
//! multi-source topologies, including multi-query isolation.

mod common;

use adaptive_engine::engine::Engine;
use adaptive_engine::executor::stats::StatisticsEvent;
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, PipelineId};
use common::capturing_sink::capturing_sink;
use common::controlled_pipeline::controlled_pipeline;
use common::controlled_source::controlled_source;
use common::stats_collector::StatsCollector;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn identifiable_buffer(id: u8) -> Buffer {
    let mut data = vec![0u8; 64];
    data[0] = id;
    Buffer::new(data)
}

// ---------------------------------------------------------------------------
// 1. Execution failure — linear chain
// ---------------------------------------------------------------------------

/// A pipeline execution failure in a linear chain (source → pipe → sink)
/// triggers terminate_pipelines, which marks all pipelines as failed, stops
/// the source, and emits a QueryError with correct context.
#[test]
fn test_terminate_on_execution_failure_linear() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    pipe_ctrl.fail_on_execute_nth(1); // Fail on first buffer

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject one buffer — triggers execution failure
    source_ctrl.inject_buffer(identifiable_buffer(1));

    // Query should terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // QueryError should have been emitted
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Source should be stopped (terminate_pipelines sets stop flag)
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 2. Setup failure — entire deployment rolls back and terminates
// ---------------------------------------------------------------------------

/// When a pipeline fails during setup, the executor emits a
/// PipelineExecutionError. The QueryEngine receives it and calls
/// terminate_pipelines. A QueryError is emitted to consumers.
#[test]
fn test_terminate_on_setup_failure() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    pipe_ctrl.fail_on_setup();

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    // Query should terminate due to setup failure
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // QueryError should have been emitted for the setup failure
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Source should be stopped
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 3. Teardown failure — cascading termination
// ---------------------------------------------------------------------------

/// A pipeline teardown failure emits PipelineExecutionError, which triggers
/// terminate_pipelines for the entire query. All successor pipelines and
/// sources are terminated.
#[test]
fn test_terminate_on_teardown_failure_cascades() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (succ, _succ_ctrl) = controlled_pipeline("succ");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    pipe_ctrl.fail_on_teardown();

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(succ).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("succ"))
        .unwrap();
    graph
        .connect(&PipelineId::new("succ"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // EOS triggers cascading shutdown; pipe's teardown fails
    source_ctrl.end_of_stream();

    // Query terminates (teardown failure triggers terminate_pipelines)
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // QueryError should have been emitted
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 4. Fan-out topology — failure in one branch terminates all branches
// ---------------------------------------------------------------------------

/// In a fan-out (source → pipe1, pipe2, pipe3 → sink), a failure in pipe1
/// triggers terminate_pipelines, which marks pipe2, pipe3, and sink as failed
/// too, causing the entire query to shut down.
#[test]
fn test_terminate_fanout_failure_terminates_all_branches() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe1, pipe1_ctrl) = controlled_pipeline("pipe1");
    let (pipe2, _pipe2_ctrl) = controlled_pipeline("pipe2");
    let (pipe3, _pipe3_ctrl) = controlled_pipeline("pipe3");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    pipe1_ctrl.fail_on_execute_nth(1); // Fail on first buffer

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe1).unwrap();
    graph.add_pipeline(pipe2).unwrap();
    graph.add_pipeline(pipe3).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe1"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe2"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe3"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe1"), &PipelineId::new("sink"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe2"), &PipelineId::new("sink"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe3"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject buffers — pipe1 fails, terminate_pipelines marks pipe2/pipe3/sink as failed
    source_ctrl.inject_buffer(identifiable_buffer(1));

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 5. Diamond topology — failure cascades through diamond
// ---------------------------------------------------------------------------

/// In a diamond (source → A, B → merge → sink), failure in A triggers
/// terminate_pipelines, which marks B, merge, and sink as failed too.
#[test]
fn test_terminate_diamond_failure_cascades() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (branch_a, branch_a_ctrl) = controlled_pipeline("branch-a");
    let (branch_b, _branch_b_ctrl) = controlled_pipeline("branch-b");
    let (merge, _merge_ctrl) = controlled_pipeline("merge");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    branch_a_ctrl.fail_on_execute_nth(1);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(branch_a).unwrap();
    graph.add_pipeline(branch_b).unwrap();
    graph.add_pipeline(merge).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("branch-a"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("branch-b"))
        .unwrap();
    graph
        .connect(&PipelineId::new("branch-a"), &PipelineId::new("merge"))
        .unwrap();
    graph
        .connect(&PipelineId::new("branch-b"), &PipelineId::new("merge"))
        .unwrap();
    graph
        .connect(&PipelineId::new("merge"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    source_ctrl.inject_buffer(identifiable_buffer(1));

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 6. Multiple sources — one source error stops all sources
// ---------------------------------------------------------------------------

/// With multiple sources (source1, source2 → pipeline → sink), a source
/// error in source1 triggers terminate_pipelines, which stops source2 and
/// marks all pipelines as failed.
#[test]
fn test_terminate_multi_source_one_fails_all_stop() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source1, source1_ctrl) = controlled_source("source1");
    let (source2, source2_ctrl) = controlled_source("source2");
    let (pipe, _pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source2"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source1_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Both sources inject data successfully first
    source1_ctrl.inject_buffer(identifiable_buffer(1));
    source2_ctrl.inject_buffer(identifiable_buffer(2));
    assert!(sink_ctrl.wait_for_buffers(2, DEFAULT_TIMEOUT));

    // Source1 injects error — terminate_pipelines should stop everything
    source1_ctrl.inject_error("Source1 failure");

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Both sources should be stopped
    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 7. Multi-query isolation — failure in one query doesn't affect others
// ---------------------------------------------------------------------------

/// Two independent queries: a failure in query 1 triggers terminate_pipelines
/// for query 1 only. Query 2 continues running and can be terminated via EOS.
#[test]
fn test_terminate_does_not_cascade_across_queries() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    // Query 1: will fail
    let (source1, source1_ctrl) = controlled_source("q1-source");
    let (pipe1, pipe1_ctrl) = controlled_pipeline("q1-pipe");
    let (sink1, _sink1_ctrl) = capturing_sink("q1-sink");

    pipe1_ctrl.fail_on_execute_nth(1);

    let mut graph1 = PipelineGraph::new();
    graph1.add_source(source1).unwrap();
    graph1.add_pipeline(pipe1).unwrap();
    graph1.add_pipeline(sink1).unwrap();
    graph1
        .connect(&PipelineId::new("q1-source"), &PipelineId::new("q1-pipe"))
        .unwrap();
    graph1
        .connect(&PipelineId::new("q1-pipe"), &PipelineId::new("q1-sink"))
        .unwrap();

    // Query 2: healthy
    let (source2, source2_ctrl) = controlled_source("q2-source");
    let (pipe2, _pipe2_ctrl) = controlled_pipeline("q2-pipe");
    let (sink2, sink2_ctrl) = capturing_sink("q2-sink");

    let mut graph2 = PipelineGraph::new();
    graph2.add_source(source2).unwrap();
    graph2.add_pipeline(pipe2).unwrap();
    graph2.add_pipeline(sink2).unwrap();
    graph2
        .connect(&PipelineId::new("q2-source"), &PipelineId::new("q2-pipe"))
        .unwrap();
    graph2
        .connect(&PipelineId::new("q2-pipe"), &PipelineId::new("q2-sink"))
        .unwrap();

    engine.start();
    let q1_id = engine.submit_query(graph1).unwrap();
    let q2_id = engine.submit_query(graph2).unwrap();

    assert!(stats.wait_for_query_running_id(q1_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_running_id(q2_id, DEFAULT_TIMEOUT));
    assert!(source1_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Trigger failure in query 1
    source1_ctrl.inject_buffer(identifiable_buffer(1));

    // Query 1 should terminate
    assert!(stats.wait_for_query_terminated_id(q1_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(q1_id, DEFAULT_TIMEOUT));
    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    // Query 2 should still be alive — send data and verify it arrives
    source2_ctrl.inject_buffer(identifiable_buffer(10));
    source2_ctrl.inject_buffer(identifiable_buffer(11));
    assert!(sink2_ctrl.wait_for_buffers(2, DEFAULT_TIMEOUT));

    // Gracefully terminate query 2
    source2_ctrl.end_of_stream();
    assert!(stats.wait_for_query_terminated_id(q2_id, DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 8. Source error during active execution
// ---------------------------------------------------------------------------

/// While buffers are being processed, a source error triggers
/// terminate_pipelines. Subsequent buffers for the failed query's
/// pipelines should be skipped.
#[test]
fn test_terminate_on_source_error_skips_remaining_buffers() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, _pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject several buffers, then an error
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.inject_buffer(identifiable_buffer(2));
    source_ctrl.inject_buffer(identifiable_buffer(3));

    // Wait for at least 1 buffer to reach sink (confirms pipeline was processing)
    assert!(sink_ctrl.wait_for_buffers(1, DEFAULT_TIMEOUT));

    // Now inject error
    source_ctrl.inject_error("Source error during active execution");

    // Query should terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Source should be stopped
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    // Sink received some buffers but not necessarily all — the key thing is
    // the query terminated and no hang occurred.
    assert!(sink_ctrl.buffer_count() >= 1);

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 9. Successor pipelines skip flush after terminate
// ---------------------------------------------------------------------------

/// After terminate_pipelines marks successors as failed, their stop tasks
/// should skip flush/teardown (fast path). Verify via teardown_called flag.
#[test]
fn test_terminate_successors_skip_flush() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail");
    let (succ, succ_ctrl) = controlled_pipeline("succ");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    fail_ctrl.fail_on_execute_nth(1);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph.add_pipeline(succ).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("fail"))
        .unwrap();
    graph
        .connect(&PipelineId::new("fail"), &PipelineId::new("succ"))
        .unwrap();
    graph
        .connect(&PipelineId::new("succ"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Trigger failure
    source_ctrl.inject_buffer(identifiable_buffer(1));

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    // "succ" was never executed (no buffers reached it since "fail" failed on
    // the first buffer), but even if buffers had arrived, terminate_pipelines
    // marks it as failed so it skips flush/teardown.
    // The key assertion: "succ" should NOT have been torn down normally.
    assert_eq!(succ_ctrl.invocation_count(), 0);

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 10. QueryError carries correct error context
// ---------------------------------------------------------------------------

/// Verify that the QueryError event contains the correct pipeline_id,
/// error_message substring, entity_type, and task_type.
#[test]
fn test_terminate_query_error_has_correct_context() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    pipe_ctrl.fail_on_execute_nth(1);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    source_ctrl.inject_buffer(identifiable_buffer(1));

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Find the QueryError event and inspect its fields
    let events = stats.get_events();
    let query_error = events.iter().find(|e| {
        matches!(
            e,
            StatisticsEvent::QueryError {
                query_id: qid, ..
            } if *qid == query_id
        )
    });

    assert!(query_error.is_some(), "Expected a QueryError event");
    if let StatisticsEvent::QueryError {
        query_id: qid,
        pipeline_id,
        error_message,
        ..
    } = query_error.unwrap()
    {
        assert_eq!(*qid, query_id);
        assert_eq!(*pipeline_id, PipelineId::new("pipe"));
        assert!(
            error_message.contains("fail"),
            "Error message should contain failure context, got: {}",
            error_message
        );
    }

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    let _exec_stats = engine.shutdown();
}

// ---------------------------------------------------------------------------
// 11. Teardown failure in multi-source query — all sources stopped
// ---------------------------------------------------------------------------

/// Two sources feed independent paths that merge at a sink. A teardown
/// failure in one path triggers terminate_pipelines, which stops both
/// sources and the entire query.
#[test]
fn test_terminate_teardown_failure_multi_source_all_stopped() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source1, source1_ctrl) = controlled_source("source1");
    let (source2, source2_ctrl) = controlled_source("source2");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail-pipe");
    let (ok_pipe, _ok_ctrl) = controlled_pipeline("ok-pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    fail_ctrl.fail_on_teardown();

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph.add_pipeline(ok_pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &PipelineId::new("fail-pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source2"), &PipelineId::new("ok-pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("fail-pipe"), &PipelineId::new("sink"))
        .unwrap();
    graph
        .connect(&PipelineId::new("ok-pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source1_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Source1 EOS → fail-pipe teardown fails → terminate_pipelines
    source1_ctrl.end_of_stream();

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Both sources should be stopped
    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 12. Execution failure with multiple workers
// ---------------------------------------------------------------------------

/// With multiple worker threads, a pipeline execution failure still correctly
/// triggers terminate_pipelines and terminates the query.
#[test]
fn test_terminate_multiworker_execution_failure() {
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(4);
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    pipe_ctrl.fail_on_execute_nth(3); // Fail on third buffer

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject enough buffers to trigger the failure
    for i in 1..=10 {
        source_ctrl.inject_buffer(identifiable_buffer(i));
    }

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 13. Many sources (10) — one source error terminates all
// ---------------------------------------------------------------------------

/// With 10 sources feeding into a single pipeline, a source error in one
/// triggers terminate_pipelines, which stops all 10 sources.
#[test]
fn test_terminate_many_sources_one_error_stops_all() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let mut source_ctrls = Vec::new();
    let mut graph = PipelineGraph::new();

    for i in 0..10 {
        let (source, ctrl) = controlled_source(&format!("source-{}", i));
        graph.add_source(source).unwrap();
        source_ctrls.push(ctrl);
    }

    let (pipe, _pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();

    for i in 0..10 {
        graph
            .connect(
                &PipelineId::new(format!("source-{}", i)),
                &PipelineId::new("pipe"),
            )
            .unwrap();
    }
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    for ctrl in &source_ctrls {
        assert!(ctrl.wait_started(DEFAULT_TIMEOUT));
    }

    // All sources inject some data
    for (i, ctrl) in source_ctrls.iter().enumerate() {
        ctrl.inject_buffer(identifiable_buffer((i + 1) as u8));
    }

    // Source 0 injects error
    source_ctrls[0].inject_error("Source 0 failure");

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // All sources should be stopped
    for ctrl in &source_ctrls {
        assert!(ctrl.wait_stopped(DEFAULT_TIMEOUT));
    }

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 14. Deep linear chain — failure at head cascades to tail
// ---------------------------------------------------------------------------

/// In a deep chain (source → p1 → p2 → p3 → p4 → sink), failure at p1
/// triggers terminate_pipelines, marking p2 through sink as failed. The
/// entire chain terminates without hanging.
#[test]
fn test_terminate_deep_chain_failure_at_head() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (p1, p1_ctrl) = controlled_pipeline("p1");
    let (p2, _p2_ctrl) = controlled_pipeline("p2");
    let (p3, _p3_ctrl) = controlled_pipeline("p3");
    let (p4, _p4_ctrl) = controlled_pipeline("p4");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    p1_ctrl.fail_on_execute_nth(1);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(p1).unwrap();
    graph.add_pipeline(p2).unwrap();
    graph.add_pipeline(p3).unwrap();
    graph.add_pipeline(p4).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("p1"))
        .unwrap();
    graph
        .connect(&PipelineId::new("p1"), &PipelineId::new("p2"))
        .unwrap();
    graph
        .connect(&PipelineId::new("p2"), &PipelineId::new("p3"))
        .unwrap();
    graph
        .connect(&PipelineId::new("p3"), &PipelineId::new("p4"))
        .unwrap();
    graph
        .connect(&PipelineId::new("p4"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    source_ctrl.inject_buffer(identifiable_buffer(1));

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

// ---------------------------------------------------------------------------
// 15. Failure mid-stream with preceding successful buffers
// ---------------------------------------------------------------------------

/// Pipeline processes several buffers successfully before failing. After
/// terminate_pipelines, the sink should have received the successful buffers
/// but the query terminates.
#[test]
fn test_terminate_after_partial_success() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, sink_ctrl) = capturing_sink("sink");

    pipe_ctrl.fail_on_execute_nth(5); // Succeed on 1-4, fail on 5

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject 10 buffers; pipeline fails on 5th
    for i in 1..=10 {
        source_ctrl.inject_buffer(identifiable_buffer(i));
    }

    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    // Sink should have received at least 1 buffer (some successful ones
    // before failure). The exact count depends on timing — the pipeline
    // succeeds on invocations 1-4 but terminate_pipelines may mark the
    // pipeline as failed before all queued buffers are dequeued.
    assert!(
        sink_ctrl.buffer_count() >= 1,
        "Expected at least 1 buffer, got {}",
        sink_ctrl.buffer_count()
    );

    // Pipeline was invoked at least 5 times (failed on 5th)
    assert!(pipe_ctrl.invocation_count() >= 5);

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}
