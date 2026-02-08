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

//! US-019: Source Failure Tests (4 tests)
//!
//! Port of C++ QueryEngineTest source failure tests.

mod common;

use adaptive_engine::engine::Engine;
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

/// Test 8: singleQueryWithSourceFailure — source injects error after data.
#[test]
fn test_source_failure() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject buffers
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));

    // Wait for at least 1 buffer
    assert!(sink_ctrl.wait_for_buffers(1, DEFAULT_TIMEOUT));

    // Inject error
    source_ctrl.inject_error("Simulated source failure");

    // Wait for query to terminate (failure cascade)
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Verify
    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);
    assert!(sink_ctrl.buffer_count() >= 1);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 9: singleQueryWithManySourcesOneOfThemFails — 10 sources, source 0 fails.
#[test]
fn test_many_sources_one_fails() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let mut source_ctrls = Vec::new();

    let mut graph = PipelineGraph::new();

    // Create 10 sources
    for i in 0..10 {
        let (source, ctrl) = controlled_source(&format!("source-{}", i));
        graph.add_source(source).unwrap();
        source_ctrls.push(ctrl);
    }

    // Add pipeline and sink
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, _sink_ctrl) = capturing_sink("sink");
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();

    // Connect all sources to pipeline
    for i in 0..10 {
        graph
            .connect(
                &PipelineId::new(format!("source-{}", i)),
                &PipelineId::new("pipeline"),
            )
            .unwrap();
    }
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));

    // Wait for all sources to start
    for ctrl in &source_ctrls {
        assert!(ctrl.wait_started(DEFAULT_TIMEOUT));
    }

    // Inject data from all sources
    for (i, ctrl) in source_ctrls.iter().enumerate() {
        for _ in 0..5 {
            ctrl.inject_buffer(identifiable_buffer((i + 1) as u8));
        }
    }

    // Source 0 injects error
    source_ctrls[0].inject_error("Source 0 failure");

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // All sources should eventually stop
    for ctrl in &source_ctrls {
        assert!(ctrl.wait_stopped(DEFAULT_TIMEOUT));
    }

    let _exec_stats = engine.shutdown();
}

/// Test 10: singleSourceWithMultipleSuccessorsSourceFailure — Source -> 3 pipelines -> sink.
#[test]
fn test_source_failure_fan_out() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe1, _pipe1_ctrl) = controlled_pipeline("pipe1");
    let (pipe2, _pipe2_ctrl) = controlled_pipeline("pipe2");
    let (pipe3, _pipe3_ctrl) = controlled_pipeline("pipe3");
    let (sink, _sink_ctrl) = capturing_sink("sink");

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

    // Inject buffers, then error
    for i in 1..=4 {
        source_ctrl.inject_buffer(identifiable_buffer(i));
    }

    // Give a moment for buffers to process
    std::thread::sleep(Duration::from_millis(100));

    source_ctrl.inject_error("Source failure in fan-out");

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 11: RaceBetweenFailureAndEOS — pipeline fails on 1st buffer, source sends EOS.
#[test]
fn test_race_between_failure_and_eos() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    // Pipeline fails on 1st invocation
    pipeline_ctrl.fail_on_execute_nth(1);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject 1 buffer (races with pipeline failure) and EOS
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.end_of_stream();

    // Query should terminate (either by failure or completion)
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}
