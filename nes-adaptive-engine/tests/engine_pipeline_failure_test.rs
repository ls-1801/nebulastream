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

//! US-020: Pipeline Failure Tests (7 tests)
//!
//! Port of C++ QueryEngineTest pipeline failure tests.

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

/// Test 12: failureDuringPipelineStop — pipeline fails during teardown.
/// Topology: source -> good -> fail(teardown fails) -> succ -> sink
#[test]
fn test_failure_during_pipeline_teardown() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (good, good_ctrl) = controlled_pipeline("good");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail");
    let (succ, _succ_ctrl) = controlled_pipeline("succ");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    // Configure fail pipeline to fail during teardown
    fail_ctrl.fail_on_teardown();

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(good).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph.add_pipeline(succ).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("good"))
        .unwrap();
    graph
        .connect(&PipelineId::new("good"), &PipelineId::new("fail"))
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

    // EOS triggers cascading shutdown — "fail" throws during teardown
    source_ctrl.end_of_stream();

    // Wait for query to terminate (failure cascade from teardown error)
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // A QueryError event should have been emitted for the teardown failure
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));

    // Good should have been gracefully stopped (it teardowns before fail)
    assert!(good_ctrl.was_teardown_called());

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 13: failureDuringPipelineStopMultipleSources
/// Topology: source1 -> fail(teardown fails) -> succ -> sink, source2 -> pipe -> sink
#[test]
fn test_failure_during_teardown_multi_source() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source1, source1_ctrl) = controlled_source("source1");
    let (source2, source2_ctrl) = controlled_source("source2");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail");
    let (succ, _succ_ctrl) = controlled_pipeline("succ");
    let (pipe, pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    fail_ctrl.fail_on_teardown();

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph.add_pipeline(succ).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &PipelineId::new("fail"))
        .unwrap();
    graph
        .connect(&PipelineId::new("fail"), &PipelineId::new("succ"))
        .unwrap();
    graph
        .connect(&PipelineId::new("succ"), &PipelineId::new("sink"))
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

    // Source2 EOS first (pipe should stop gracefully)
    source2_ctrl.end_of_stream();
    // Give some time for pipe to stop
    std::thread::sleep(Duration::from_millis(200));

    // Source1 EOS — triggers teardown of fail, which throws
    source1_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // pipe should have been stopped gracefully (before the failure)
    assert!(pipe_ctrl.was_teardown_called());

    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 14: failureDuringPipelineStopMultipleSourcesRaceBetweenFailAndEoS
/// Topology: source1 -> fail(teardown fails) -> succ -> sink, source2 -> pipe -> sink
/// Only source1 sends EOS — source2 affected by query failure.
#[test]
fn test_teardown_failure_race_with_eos() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source1, source1_ctrl) = controlled_source("source1");
    let (source2, source2_ctrl) = controlled_source("source2");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail");
    let (succ, _succ_ctrl) = controlled_pipeline("succ");
    let (pipe, _pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    fail_ctrl.fail_on_teardown();

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph.add_pipeline(succ).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &PipelineId::new("fail"))
        .unwrap();
    graph
        .connect(&PipelineId::new("fail"), &PipelineId::new("succ"))
        .unwrap();
    graph
        .connect(&PipelineId::new("succ"), &PipelineId::new("sink"))
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

    // Only source1 sends EOS (fail throws during teardown, affecting entire query)
    source1_ctrl.end_of_stream();

    // Wait for query to terminate (query-level failure)
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 15: failureDuringPipelineStartWithMultiplePipelines
/// Source -> failing_pipeline + 100 OK pipelines -> sinks
#[test]
fn test_setup_failure_with_many_pipelines() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail-pipe");
    fail_ctrl.fail_on_setup();

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("fail-pipe"))
        .unwrap();

    // Add 10 OK pipelines (100 would be heavy for a test)
    for i in 0..10 {
        let (ok_pipe, _ok_ctrl) = controlled_pipeline(&format!("ok-pipe-{}", i));
        let (ok_sink, _ok_sink_ctrl) = capturing_sink(&format!("ok-sink-{}", i));
        graph.add_pipeline(ok_pipe).unwrap();
        graph.add_pipeline(ok_sink).unwrap();
        graph
            .connect(
                &PipelineId::new("source"),
                &PipelineId::new(format!("ok-pipe-{}", i)),
            )
            .unwrap();
        graph
            .connect(
                &PipelineId::new(format!("ok-pipe-{}", i)),
                &PipelineId::new(format!("ok-sink-{}", i)),
            )
            .unwrap();
    }

    let (fail_sink, _fail_sink_ctrl) = capturing_sink("fail-sink");
    graph.add_pipeline(fail_sink).unwrap();
    graph
        .connect(&PipelineId::new("fail-pipe"), &PipelineId::new("fail-sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    // Query should terminate due to setup failure
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 16: failureDuringPipelineStartWithMultipleSources
/// source1 -> fail(setup fails) -> fail_succ -> sink, source2 -> pipeline -> sink
#[test]
fn test_setup_failure_with_multi_source() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source1, source1_ctrl) = controlled_source("source1");
    let (source2, source2_ctrl) = controlled_source("source2");
    let (fail_pipe, fail_ctrl) = controlled_pipeline("fail");
    fail_ctrl.fail_on_setup();
    let (fail_succ, _fail_succ_ctrl) = controlled_pipeline("fail-succ");
    let (pipe, _pipe_ctrl) = controlled_pipeline("pipe");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(fail_pipe).unwrap();
    graph.add_pipeline(fail_succ).unwrap();
    graph.add_pipeline(pipe).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &PipelineId::new("fail"))
        .unwrap();
    graph
        .connect(&PipelineId::new("fail"), &PipelineId::new("fail-succ"))
        .unwrap();
    graph
        .connect(&PipelineId::new("fail-succ"), &PipelineId::new("sink"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source2"), &PipelineId::new("pipe"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipe"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    // Query should terminate due to setup failure
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 17: singleQueryWithPipelineFailure — pipeline fails on nth invocation.
#[test]
fn test_pipeline_execution_failure() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    // Fail on 2nd invocation
    pipeline_ctrl.fail_on_execute_nth(2);

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

    // Inject 4 buffers
    for i in 1..=4 {
        source_ctrl.inject_buffer(identifiable_buffer(i));
    }

    // Wait for query to terminate (pipeline failure on 2nd buffer)
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // A QueryError event should have been emitted for this query
    assert!(stats.wait_for_query_error_id(query_id, DEFAULT_TIMEOUT));
    assert!(stats.count_query_errors() >= 1);

    // Pipeline was invoked at least 2 times (failed on 2nd or later)
    assert!(pipeline_ctrl.invocation_count() >= 2);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 18: singleQueryWithSlowlyFailingSourceDuringEngineTermination
/// Source fails during open with a delay, shutdown happens before source fully opens.
#[test]
fn test_slow_source_failure_during_shutdown() {
    let (mut engine, receiver) = Engine::with_stats();
    let _stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    // Fail during open with 1 second delay
    source.set_fail_during_open(1000);

    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, _sink_ctrl) = capturing_sink("sink");

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
    let _query_id = engine.submit_query(graph).unwrap();

    // Wait a short time for the query to start deploying
    std::thread::sleep(Duration::from_millis(100));

    // Shutdown while source is still opening
    let _exec_stats = engine.shutdown();

    // Source should eventually stop (after the delay)
    assert!(source_ctrl.wait_stopped(Duration::from_secs(15)));
}
