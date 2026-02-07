//! US-023: Scalability and Edge Case Tests (4 tests)
//!
//! Port of C++ QueryEngineTest scalability tests.

mod common;

use adaptive_engine::engine::Engine;
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, PipelineId};
use common::capturing_sink::capturing_sink;
use common::controlled_pipeline::controlled_pipeline;
use common::controlled_source::controlled_source;
use common::stats_collector::StatsCollector;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

fn identifiable_buffer(id: u8) -> Buffer {
    let mut data = vec![0u8; 64];
    data[0] = id;
    Buffer::new(data)
}

/// Test 26: singleQueryWithManySources — 100 sources -> pipeline -> sink.
#[test]
fn test_many_sources() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let mut graph = PipelineGraph::new();
    let mut source_ctrls = Vec::new();

    // Create 100 sources
    for i in 0..100 {
        let (source, ctrl) = controlled_source(&format!("source-{}", i));
        graph.add_source(source).unwrap();
        source_ctrls.push(ctrl);
    }

    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();

    // Connect all sources to pipeline
    for i in 0..100 {
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

    // Inject 2 buffers per source (200 total)
    for (i, ctrl) in source_ctrls.iter().enumerate() {
        ctrl.inject_buffer(identifiable_buffer(((i + 1) % 256) as u8));
        ctrl.inject_buffer(identifiable_buffer(((i + 1) % 256) as u8));
    }

    // Wait for at least 200 buffers at sink
    assert!(sink_ctrl.wait_for_buffers(200, DEFAULT_TIMEOUT));

    // EOS from all sources
    for ctrl in &source_ctrls {
        ctrl.end_of_stream();
    }

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // All sources should stop
    for ctrl in &source_ctrls {
        assert!(ctrl.wait_stopped(DEFAULT_TIMEOUT));
    }

    let _exec_stats = engine.shutdown();
}

/// Test 27: singleSourceWithMultipleSuccessors — source fans out to 3 pipelines.
#[test]
fn test_single_source_fan_out() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipe1, pipe1_ctrl) = controlled_pipeline("pipe1");
    let (pipe2, pipe2_ctrl) = controlled_pipeline("pipe2");
    let (pipe3, pipe3_ctrl) = controlled_pipeline("pipe3");
    let (sink, sink_ctrl) = capturing_sink("sink");

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

    // Inject 4 buffers
    for i in 1..=4 {
        source_ctrl.inject_buffer(identifiable_buffer(i));
    }

    // Each buffer goes to 3 pipelines, each emits to sink = 12 buffers at sink
    assert!(sink_ctrl.wait_for_buffers(12, DEFAULT_TIMEOUT));

    // Send EOS
    source_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Verify statistics
    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_stops() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);
    assert!(stats.count_pipeline_starts() >= 5); // source + 3 pipes + sink
    assert!(stats.count_pipeline_stops() >= 5);

    // Verify data: 4 buffers * 3 pipelines = 12 task executions on each pipe,
    // 12 task executions on sink, + 12 emits from pipes to sink
    assert!(stats.count_task_execution_starts() >= 24); // 12 pipe + 12 sink
    assert!(stats.count_task_emits() >= 12);

    // All pipelines gracefully stopped
    assert!(pipe1_ctrl.was_teardown_called());
    assert!(pipe2_ctrl.was_teardown_called());
    assert!(pipe3_ctrl.was_teardown_called());

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 28: singleQueryWithTwoSourceExternalStop — two sources, stopped via stop_query.
#[test]
fn test_two_sources_external_stop() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    let (source1, source1_ctrl) = controlled_source("source1");
    let (source2, source2_ctrl) = controlled_source("source2");
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source1).unwrap();
    graph.add_source(source2).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source2"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source1_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject 3 buffers total
    source1_ctrl.inject_buffer(identifiable_buffer(1));
    source1_ctrl.inject_buffer(identifiable_buffer(2));
    source2_ctrl.inject_buffer(identifiable_buffer(3));

    assert!(sink_ctrl.wait_for_buffers(3, DEFAULT_TIMEOUT));

    // Stop the query via API
    let stopped = engine.stop_query(query_id).unwrap();
    assert!(stopped);

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_stops() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);

    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 29: singleQueryWithSlowlyFailingSourceDuringQueryPlanTermination
/// Source fails during open with 10s delay, stop_query called first.
#[test]
fn test_slow_failing_source_during_termination() {
    let (mut engine, receiver) = Engine::with_stats();
    let _stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    // Source fails during open with a delay (use shorter delay for tests)
    source.set_fail_during_open(2000);

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
    let query_id = engine.submit_query(graph).unwrap();

    // Give a moment for query to start deploying
    std::thread::sleep(Duration::from_millis(200));

    // Stop the query while source is still opening
    let _ = engine.stop_query(query_id);

    // Source will eventually fail (after delay) and query should terminate
    assert!(source_ctrl.wait_stopped(Duration::from_secs(15)));

    let _exec_stats = engine.shutdown();
}
