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

//! US-022: Multi-Query Tests (3 tests)
//!
//! Port of C++ QueryEngineTest multi-query tests.

mod common;

use adaptive_engine::engine::Engine;
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, PipelineId};
use common::capturing_sink::capturing_sink;
use common::controlled_pipeline::controlled_pipeline;
use common::controlled_source::controlled_source;
use common::stats_collector::StatsCollector;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

fn identifiable_buffer(id: u8) -> Buffer {
    let mut data = vec![0u8; 64];
    data[0] = id;
    Buffer::new(data)
}

/// Helper to build a graph: source1, source2 -> pipeline -> sink
fn build_two_source_graph(
    query_idx: usize,
) -> (
    PipelineGraph,
    common::controlled_source::SourceController,
    common::controlled_source::SourceController,
    Arc<common::controlled_pipeline::PipelineController>,
    common::capturing_sink::SinkController,
) {
    let (source1, source1_ctrl) = controlled_source(&format!("q{}-source1", query_idx));
    let (source2, source2_ctrl) = controlled_source(&format!("q{}-source2", query_idx));
    let (pipeline, pipeline_ctrl) = controlled_pipeline(&format!("q{}-pipeline", query_idx));
    let (sink, sink_ctrl) = capturing_sink(&format!("q{}-sink", query_idx));

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(
            &PipelineId::new(format!("q{}-source1", query_idx)),
            &PipelineId::new(format!("q{}-pipeline", query_idx)),
        )
        .unwrap();
    graph
        .connect(
            &PipelineId::new(format!("q{}-source2", query_idx)),
            &PipelineId::new(format!("q{}-pipeline", query_idx)),
        )
        .unwrap();
    graph
        .connect(
            &PipelineId::new(format!("q{}-pipeline", query_idx)),
            &PipelineId::new(format!("q{}-sink", query_idx)),
        )
        .unwrap();

    (graph, source1_ctrl, source2_ctrl, pipeline_ctrl, sink_ctrl)
}

/// Test 23: ManyQueriesWithTwoSources — 10 queries, all graceful via EOS.
#[test]
fn test_many_queries_two_sources() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    engine.start();

    let mut queries = Vec::new();

    // Submit 10 queries
    for i in 0..10 {
        let (graph, s1_ctrl, s2_ctrl, _pipe_ctrl, sink_ctrl) = build_two_source_graph(i);
        let query_id = engine.submit_query(graph).unwrap();
        queries.push((query_id, s1_ctrl, s2_ctrl, sink_ctrl));
    }

    // Wait for all queries to be running
    for (query_id, s1_ctrl, s2_ctrl, _) in &queries {
        assert!(stats.wait_for_query_running_id(*query_id, DEFAULT_TIMEOUT));
        assert!(s1_ctrl.wait_started(DEFAULT_TIMEOUT));
        assert!(s2_ctrl.wait_started(DEFAULT_TIMEOUT));
    }

    // Inject data and send EOS for each query
    for (_, s1_ctrl, s2_ctrl, sink_ctrl) in &queries {
        s1_ctrl.inject_buffer(identifiable_buffer(1));
        s2_ctrl.inject_buffer(identifiable_buffer(2));

        // Wait for 2 buffers to arrive
        assert!(sink_ctrl.wait_for_buffers(2, DEFAULT_TIMEOUT));

        // Send EOS from both sources
        s1_ctrl.end_of_stream();
        s2_ctrl.end_of_stream();
    }

    // Wait for all queries to terminate
    for (query_id, _, _, _) in &queries {
        assert!(stats.wait_for_query_terminated_id(*query_id, DEFAULT_TIMEOUT));
    }

    // All sources should be stopped
    for (_, s1_ctrl, s2_ctrl, _) in &queries {
        assert!(s1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
        assert!(s2_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    }

    let _exec_stats = engine.shutdown();
}

/// Test 24: Source failure terminates only the failing query — per-query isolation.
///
/// When a source in query 0 fails, only query 0's pipelines are stopped.
/// Other queries continue running and can be individually stopped or
/// gracefully terminated via EOS.
#[test]
fn test_many_queries_source_failure_isolates_query() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    engine.start();

    let mut queries = Vec::new();

    for i in 0..3 {
        let (graph, s1_ctrl, s2_ctrl, _pipe_ctrl, sink_ctrl) = build_two_source_graph(i);
        let query_id = engine.submit_query(graph).unwrap();
        queries.push((query_id, s1_ctrl, s2_ctrl, sink_ctrl));
    }

    // Wait for all to be running
    for (query_id, s1_ctrl, s2_ctrl, _) in &queries {
        assert!(stats.wait_for_query_running_id(*query_id, DEFAULT_TIMEOUT));
        assert!(s1_ctrl.wait_started(DEFAULT_TIMEOUT));
        assert!(s2_ctrl.wait_started(DEFAULT_TIMEOUT));
    }

    // Inject initial data for all queries
    for (_, s1_ctrl, s2_ctrl, _) in &queries {
        s1_ctrl.inject_buffer(identifiable_buffer(1));
        s2_ctrl.inject_buffer(identifiable_buffer(2));
    }

    // Query 0 source 1 fails — only query 0 should be terminated
    queries[0].1.inject_error("Source failure");

    // Query 0 should be terminated
    assert!(stats.wait_for_query_terminated_id(queries[0].0, DEFAULT_TIMEOUT));

    // Query 0's sources should stop
    assert!(queries[0].1.wait_stopped(DEFAULT_TIMEOUT));
    assert!(queries[0].2.wait_stopped(DEFAULT_TIMEOUT));

    // Explicitly stop query 1
    let stopped = engine.stop_query(queries[1].0).unwrap();
    assert!(stopped, "Query 1 should have been stopped");
    assert!(queries[1].1.wait_stopped(DEFAULT_TIMEOUT));
    assert!(queries[1].2.wait_stopped(DEFAULT_TIMEOUT));

    // Gracefully terminate query 2 via EOS
    queries[2].1.end_of_stream();
    queries[2].2.end_of_stream();
    assert!(stats.wait_for_query_terminated_id(queries[2].0, DEFAULT_TIMEOUT));
    assert!(queries[2].1.wait_stopped(DEFAULT_TIMEOUT));
    assert!(queries[2].2.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}

/// Test 25: Pipeline failure terminates only the failing query — per-query isolation.
///
/// When a pipeline in query 1 fails, only query 1 is terminated.
/// Query 0 continues running and can be gracefully terminated via EOS.
#[test]
fn test_many_queries_pipeline_failure_isolates_query() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    engine.start();

    // Query 0: normal (no failure configured)
    let (graph0, s1_ctrl, s2_ctrl, _pipe_ctrl0, sink_ctrl0) = build_two_source_graph(0);
    let q0_id = engine.submit_query(graph0).unwrap();
    assert!(stats.wait_for_query_running_id(q0_id, DEFAULT_TIMEOUT));
    assert!(s1_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(s2_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Query 1: pipeline fails on 2nd invocation
    let (graph1, s3_ctrl, s4_ctrl, pipe_ctrl1, _sink_ctrl1) = build_two_source_graph(1);
    pipe_ctrl1.fail_on_execute_nth(2);
    let q1_id = engine.submit_query(graph1).unwrap();
    assert!(stats.wait_for_query_running_id(q1_id, DEFAULT_TIMEOUT));
    assert!(s3_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(s4_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject data to trigger the failure in query 1
    s3_ctrl.inject_buffer(identifiable_buffer(1));
    s4_ctrl.inject_buffer(identifiable_buffer(2));

    // Query 1 should be terminated
    assert!(stats.wait_for_query_terminated_id(q1_id, DEFAULT_TIMEOUT));
    assert!(s3_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(s4_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    // Query 0 should still be alive — verify by injecting data and receiving it
    s1_ctrl.inject_buffer(identifiable_buffer(10));
    assert!(sink_ctrl0.wait_for_buffers(1, DEFAULT_TIMEOUT));

    // Gracefully terminate query 0 via EOS
    s1_ctrl.end_of_stream();
    s2_ctrl.end_of_stream();
    assert!(stats.wait_for_query_terminated_id(q0_id, DEFAULT_TIMEOUT));
    assert!(s1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(s2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
}
