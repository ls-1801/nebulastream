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

/// Test 24: ManyQueriesWithTwoSourcesOneSourceFails — query 0 fails, query 1 stopped, rest EOS.
#[test]
fn test_many_queries_source_failure_isolation() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    engine.start();

    let mut queries = Vec::new();

    for i in 0..10 {
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

    // Query 0: source 1 fails
    queries[0].1.inject_error("Query 0 source 1 failure");

    // Query 1: stopped via stop_query
    let stopped = engine.stop_query(queries[1].0).unwrap();
    assert!(stopped);

    // Queries 2-9: graceful via EOS
    for i in 2..10 {
        queries[i].1.end_of_stream();
        queries[i].2.end_of_stream();
    }

    // Wait for queries 0 and 1 to terminate
    assert!(stats.wait_for_query_terminated_id(queries[0].0, DEFAULT_TIMEOUT));
    assert!(stats.wait_for_query_terminated_id(queries[1].0, DEFAULT_TIMEOUT));

    // Wait for queries 2-9 to terminate
    for i in 2..10 {
        assert!(stats.wait_for_query_terminated_id(queries[i].0, DEFAULT_TIMEOUT));
    }

    // All sources should stop
    for (_, s1_ctrl, s2_ctrl, _) in &queries {
        assert!(s1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
        assert!(s2_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    }

    let _exec_stats = engine.shutdown();
}

/// Test 25: ManyQueriesWithTwoSourcesAndPipelineFailures
/// Query 0: no failure (EOS), Queries 1-9: pipeline fails on 2nd invocation.
#[test]
fn test_many_queries_pipeline_failure_isolation() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    engine.start();

    let mut queries = Vec::new();

    for i in 0..10 {
        let (source1, s1_ctrl) = controlled_source(&format!("q{}-source1", i));
        let (source2, s2_ctrl) = controlled_source(&format!("q{}-source2", i));
        let (pipeline, pipe_ctrl) = controlled_pipeline(&format!("q{}-pipeline", i));
        let (sink, sink_ctrl) = capturing_sink(&format!("q{}-sink", i));

        // Queries 1-9: fail on 2nd invocation
        if i > 0 {
            pipe_ctrl.fail_on_execute_nth(2);
        }

        let mut graph = PipelineGraph::new();
        graph.add_source(source1).unwrap();
        graph.add_source(source2).unwrap();
        graph.add_pipeline(pipeline).unwrap();
        graph.add_pipeline(sink).unwrap();
        graph
            .connect(
                &PipelineId::new(format!("q{}-source1", i)),
                &PipelineId::new(format!("q{}-pipeline", i)),
            )
            .unwrap();
        graph
            .connect(
                &PipelineId::new(format!("q{}-source2", i)),
                &PipelineId::new(format!("q{}-pipeline", i)),
            )
            .unwrap();
        graph
            .connect(
                &PipelineId::new(format!("q{}-pipeline", i)),
                &PipelineId::new(format!("q{}-sink", i)),
            )
            .unwrap();

        let query_id = engine.submit_query(graph).unwrap();
        queries.push((query_id, s1_ctrl, s2_ctrl, pipe_ctrl, sink_ctrl));
    }

    // Wait for all to be running
    for (query_id, s1, s2, _, _) in &queries {
        assert!(stats.wait_for_query_running_id(*query_id, DEFAULT_TIMEOUT));
        assert!(s1.wait_started(DEFAULT_TIMEOUT));
        assert!(s2.wait_started(DEFAULT_TIMEOUT));
    }

    // Inject data: 4 buffers from each source
    for (_, s1, s2, _, _) in &queries {
        for j in 1..=4 {
            s1.inject_buffer(identifiable_buffer(j));
            s2.inject_buffer(identifiable_buffer(j + 10));
        }
    }

    // Queries 1-9 should fail after pipeline invoked 2 times
    for i in 1..10 {
        assert!(stats.wait_for_query_terminated_id(queries[i].0, DEFAULT_TIMEOUT));
    }

    // Query 0: send EOS
    queries[0].1.end_of_stream();
    queries[0].2.end_of_stream();
    assert!(stats.wait_for_query_terminated_id(queries[0].0, DEFAULT_TIMEOUT));

    // All sources should stop
    for (_, s1, s2, _, _) in &queries {
        assert!(s1.wait_stopped(DEFAULT_TIMEOUT));
        assert!(s2.wait_stopped(DEFAULT_TIMEOUT));
    }

    let _exec_stats = engine.shutdown();
}
