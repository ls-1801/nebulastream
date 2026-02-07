//! US-021: Repeat Task Tests (4 tests)
//!
//! Port of C++ QueryEngineTest repeat_task tests.

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

/// Test 19: SingleQueryWithRepeatingSink — sink repeats buffer 3 times.
#[test]
fn test_repeating_sink() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (sink, sink_ctrl) = capturing_sink("sink");

    // Sink repeats each buffer 3 times
    sink_ctrl.set_repeat_count(3);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject 1 buffer then EOS
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Sink should have been invoked at least 4 times (1 initial + 3 repeats)
    assert!(sink_ctrl.invocation_count() >= 4);

    // Verify statistics
    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_stops() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);
    assert!(stats.count_pipeline_starts() >= 2);
    assert!(stats.count_pipeline_stops() >= 2);
    // 4 task execution starts (1 initial + 3 repeats on sink)
    assert!(stats.count_task_execution_starts() >= 4);
    assert!(stats.count_task_execution_completes() >= 4);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 20: SingleQueryWithRepeatingPipeline — pipeline repeats buffer 3 times.
#[test]
fn test_repeating_pipeline() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");

    // Pipeline repeats each buffer 3 times
    pipeline_ctrl.set_repeat_count(3);

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

    // Inject 1 buffer then EOS
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Pipeline invoked at least 4 times (1 initial + 3 repeats)
    assert!(pipeline_ctrl.invocation_count() >= 4);
    // Sink should get at least 1 buffer
    assert!(sink_ctrl.invocation_count() >= 1);

    // Verify statistics
    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_stops() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);
    assert!(stats.count_pipeline_starts() >= 3);
    assert!(stats.count_pipeline_stops() >= 3);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 21: SingleQueryWithRepeatingSinkDuringQueryStop — sink repeats during teardown.
#[test]
fn test_repeating_sink_during_stop() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");

    // Sink repeats stop 3 times
    sink_ctrl.set_repeat_count_during_stop(3);

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

    // Inject 1 buffer then EOS
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Sink stop should have been called at least 4 times (1 initial + 3 repeats)
    assert!(sink_ctrl.stop_call_count() >= 4);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 22: SingleQueryWithMultipleSinksDuringQueryStopOneIsRepeated
/// Source -> Pipeline -> Sink1(repeat_stop=2), Sink2
#[test]
fn test_multiple_sinks_one_repeating_during_stop() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink1, sink1_ctrl) = capturing_sink("sink1");
    let (sink2, sink2_ctrl) = capturing_sink("sink2");

    // Sink1 repeats stop 2 times
    sink1_ctrl.set_repeat_count_during_stop(2);

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink1).unwrap();
    graph.add_pipeline(sink2).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink1"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink2"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject 1 buffer then EOS
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Sink1 stop should have been called at least 3 times (1 + 2 repeats)
    assert!(sink1_ctrl.stop_call_count() >= 3);
    // Sink2 stop should have been called at least 1 time
    assert!(sink2_ctrl.stop_call_count() >= 1);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}
