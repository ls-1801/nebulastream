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

//! US-018: Basic Engine Lifecycle Tests (7 tests)
//!
//! Port of C++ QueryEngineTest lifecycle tests to Rust-native tests
//! using the Engine API directly (no FFI).

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

/// Test 1: simpleTest — engine starts and stops without queries.
#[test]
fn test_engine_start_shutdown() {
    let (mut engine, receiver) = Engine::with_stats();
    let _stats = StatsCollector::new(receiver);
    engine.start();
    let exec_stats = engine.shutdown();
    assert_eq!(exec_stats.buffers_processed, 0);
}

/// Test 2: singleQueryWithShutdown — Source -> Sink, inject data, shutdown.
#[test]
fn test_single_query_with_shutdown() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    // Build: source -> sink
    let (source, source_ctrl) = controlled_source("source");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();
    assert!(query_id > 0);

    // Wait for query to be running
    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));

    // Wait for source to start
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject 4 buffers
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));

    // Wait for all buffers at sink
    assert!(sink_ctrl.wait_for_buffers(4, DEFAULT_TIMEOUT));

    // Shutdown (non-graceful)
    let _exec_stats = engine.shutdown();

    // Verify statistics
    assert!(stats.wait_for_query_starts(1, DEFAULT_TIMEOUT));
    assert_eq!(stats.count_query_starts(), 1);
    assert_eq!(stats.count_query_running(), 1);
    assert!(stats.count_pipeline_starts() >= 2);
    assert!(stats.count_task_execution_starts() >= 4);
    assert!(stats.count_task_execution_completes() >= 4);

    // Verify source lifecycle
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));
}

/// Test 3: singleQueryWithSystemShutdown — Source -> Pipeline -> Sink.
#[test]
fn test_single_query_with_system_shutdown() {
    let (mut engine, receiver) = Engine::with_stats();
    let stats = StatsCollector::new(receiver);

    // Build: source -> pipeline -> sink
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

    // Inject 4 buffers
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));

    // Wait for all buffers at sink
    assert!(sink_ctrl.wait_for_buffers(4, DEFAULT_TIMEOUT));

    // Verify first buffer has identifier
    let buffers = sink_ctrl.take_buffers();
    assert_eq!(buffers[0].data()[0], 1);

    // Shutdown
    let _exec_stats = engine.shutdown();

    // Verify statistics
    assert_eq!(stats.count_query_starts(), 1);
    assert_eq!(stats.count_query_running(), 1);
    assert!(stats.count_pipeline_starts() >= 3); // source + pipeline + sink
    assert!(stats.count_task_execution_starts() >= 8); // 4 pipeline + 4 sink
    assert!(stats.count_task_emits() >= 4); // pipeline emits to sink

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));
}

/// Test 4: singleQueryWithExternalStop — Source -> Pipeline -> Sink with EOS.
#[test]
fn test_single_query_with_external_stop() {
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

    // Inject 4 buffers then EOS
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));

    // Wait for buffers
    assert!(sink_ctrl.wait_for_buffers(4, DEFAULT_TIMEOUT));

    // Signal end of stream (graceful termination)
    source_ctrl.end_of_stream();

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Verify statistics
    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_stops() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);
    assert!(stats.count_pipeline_starts() >= 3);
    assert!(stats.count_pipeline_stops() >= 3);

    // Verify data
    let buffers = sink_ctrl.take_buffers();
    assert!(buffers.len() >= 4);
    assert_eq!(buffers[0].data()[0], 1);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 5: singleQueryWithSystemStop — stop via stop_query().
#[test]
fn test_single_query_with_system_stop() {
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

    // Inject 4 buffers
    source_ctrl.inject_buffer(identifiable_buffer(1));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));

    // Wait for at least 1 buffer
    assert!(sink_ctrl.wait_for_buffers(1, DEFAULT_TIMEOUT));

    // Stop the query
    let stopped = engine.stop_query(query_id).unwrap();
    assert!(stopped);

    // Wait for query to terminate
    assert!(stats.wait_for_query_terminated_id(query_id, DEFAULT_TIMEOUT));

    // Verify statistics (ranges due to race with stop)
    assert!(stats.count_query_starts() >= 1);
    assert!(stats.count_query_stops() >= 1);
    assert!(stats.count_query_running() >= 1);
    assert!(stats.count_query_terminated() >= 1);
    assert!(stats.count_pipeline_starts() >= 3);
    assert!(stats.count_pipeline_stops() >= 3);

    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    let _exec_stats = engine.shutdown();
}

/// Test 6: singleQueryWithTwoSourcesShutdown — two sources merge into pipeline -> sink.
#[test]
fn test_single_query_two_sources_shutdown() {
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

    // Inject buffers from both sources (2 each, identified)
    source1_ctrl.inject_buffer(identifiable_buffer(1));
    source2_ctrl.inject_buffer(identifiable_buffer(2));
    source1_ctrl.inject_buffer(identifiable_buffer(3));
    source2_ctrl.inject_buffer(identifiable_buffer(4));

    // Wait for all 4 buffers at sink
    assert!(sink_ctrl.wait_for_buffers(4, DEFAULT_TIMEOUT));

    // Shutdown
    let _exec_stats = engine.shutdown();

    // Verify
    assert_eq!(stats.count_query_starts(), 1);
    assert_eq!(stats.count_query_running(), 1);
    assert!(stats.count_pipeline_starts() >= 4); // 2 sources + pipeline + sink
    assert!(stats.count_task_execution_starts() >= 8); // 4 pipeline + 4 sink

    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));
}

/// Test 7: singleQueryWithTwoSourcesWaitingForTwoStops — two sources, both send EOS.
#[test]
fn test_single_query_two_sources_eos() {
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

    // Inject initial buffers (2 from each)
    source1_ctrl.inject_buffer(identifiable_buffer(1));
    source2_ctrl.inject_buffer(identifiable_buffer(2));
    source1_ctrl.inject_buffer(identifiable_buffer(3));
    source2_ctrl.inject_buffer(identifiable_buffer(4));

    assert!(sink_ctrl.wait_for_buffers(4, DEFAULT_TIMEOUT));

    // Source1 sends EOS
    source1_ctrl.end_of_stream();

    // Inject 2 more from source2
    source2_ctrl.inject_buffer(identifiable_buffer(5));
    source2_ctrl.inject_buffer(identifiable_buffer(6));

    assert!(sink_ctrl.wait_for_buffers(6, DEFAULT_TIMEOUT));

    // Source2 sends EOS
    source2_ctrl.end_of_stream();

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
