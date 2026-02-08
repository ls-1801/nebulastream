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

//! Per-pipeline error isolation tests at the executor/graph layer.
//!
//! These tests use the executor directly (no QueryEngine) to verify that:
//! - A pipeline failure only marks that pipeline as failed
//! - Other pipelines in the same graph continue processing
//! - Source errors are isolated to the failing source
//! - Graceful EOS from one source does not block other sources

mod common;

use adaptive_engine::executor::Executor;
use adaptive_engine::executor::stats::StatisticsSender;
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, PipelineId};
use common::capturing_sink::capturing_sink;
use common::controlled_pipeline::controlled_pipeline;
use common::controlled_source::controlled_source;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

fn identifiable_buffer(id: u8) -> Buffer {
    let mut data = vec![0u8; 64];
    data[0] = id;
    Buffer::new(data)
}

/// Two independent pipelines deployed in a single graph.
/// When pipeline A fails, pipeline B continues processing because the
/// executor has no query concept — it only marks the failed pipeline.
#[test]
fn test_pipeline_failure_does_not_affect_other_pipelines() {
    let (tx, _rx) = mpsc::channel();
    let sender = StatisticsSender::new(tx);
    let executor = Executor::with_worker_count_and_stats(1, sender);
    let handle = executor.get_handle();

    // Path A: source_a -> pipeline_a -> sink_a (pipeline_a will fail)
    let (source_a, source_a_ctrl) = controlled_source("source_a");
    let (pipeline_a, pipeline_a_ctrl) = controlled_pipeline("pipeline_a");
    let (sink_a, _sink_a_ctrl) = capturing_sink("sink_a");

    // Path B: source_b -> pipeline_b -> sink_b (healthy)
    let (source_b, source_b_ctrl) = controlled_source("source_b");
    let (pipeline_b, _pipeline_b_ctrl) = controlled_pipeline("pipeline_b");
    let (sink_b, sink_b_ctrl) = capturing_sink("sink_b");

    // Configure pipeline_a to fail on 2nd invocation
    pipeline_a_ctrl.fail_on_execute_nth(2);

    let mut graph = PipelineGraph::new();
    graph.add_source(source_a).unwrap();
    graph.add_pipeline(pipeline_a).unwrap();
    graph.add_pipeline(sink_a).unwrap();
    graph.add_source(source_b).unwrap();
    graph.add_pipeline(pipeline_b).unwrap();
    graph.add_pipeline(sink_b).unwrap();
    graph
        .connect(&PipelineId::new("source_a"), &PipelineId::new("pipeline_a"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline_a"), &PipelineId::new("sink_a"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source_b"), &PipelineId::new("pipeline_b"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline_b"), &PipelineId::new("sink_b"))
        .unwrap();

    handle.deploy_graph(graph).unwrap();
    let exec_thread = thread::spawn(move || executor.run());

    assert!(source_a_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source_b_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Trigger failure in pipeline_a (2nd invocation)
    source_a_ctrl.inject_buffer(identifiable_buffer(1));
    source_a_ctrl.inject_buffer(identifiable_buffer(2));

    // Give time for the error to be processed
    thread::sleep(Duration::from_millis(200));

    // Path B should still work — inject and receive buffers AFTER path A failed
    for i in 0..5 {
        source_b_ctrl.inject_buffer(identifiable_buffer(10 + i));
    }

    assert!(
        sink_b_ctrl.wait_for_buffers(5, DEFAULT_TIMEOUT),
        "Path B should continue receiving buffers after path A's pipeline failed"
    );

    // Gracefully terminate both paths
    source_a_ctrl.end_of_stream();
    source_b_ctrl.end_of_stream();

    // Wait for both sources to stop
    assert!(source_a_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source_b_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    handle.shutdown().unwrap();
    let exec_stats = exec_thread.join().unwrap();
    assert!(exec_stats.has_errors());
}

/// Source error marks only that source as failed.
/// Other sources feeding the same downstream pipeline continue working.
#[test]
fn test_source_error_does_not_affect_other_sources() {
    let (tx, _rx) = mpsc::channel();
    let sender = StatisticsSender::new(tx);
    let executor = Executor::with_worker_count_and_stats(1, sender);
    let handle = executor.get_handle();

    // Both sources feed the same pipeline → sink
    let (source_a, source_a_ctrl) = controlled_source("source_a");
    let (source_b, source_b_ctrl) = controlled_source("source_b");
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source_a).unwrap();
    graph.add_source(source_b).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source_a"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source_b"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink"))
        .unwrap();

    handle.deploy_graph(graph).unwrap();
    let exec_thread = thread::spawn(move || executor.run());

    assert!(source_a_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source_b_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Source A emits some data then fails
    source_a_ctrl.inject_buffer(identifiable_buffer(1));
    source_a_ctrl.inject_error("Source A failure");

    assert!(source_a_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    // Source B should still work — inject buffers AFTER source A failed
    for i in 0..5 {
        source_b_ctrl.inject_buffer(identifiable_buffer(10 + i));
    }

    assert!(
        sink_ctrl.wait_for_buffers(5, DEFAULT_TIMEOUT),
        "Sink should receive buffers from source_b after source_a failed"
    );

    source_b_ctrl.end_of_stream();
    assert!(source_b_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    handle.shutdown().unwrap();
    let exec_stats = exec_thread.join().unwrap();
    assert!(exec_stats.has_errors());
}

/// When one source sends EOS, its successors still receive data from other
/// active sources.
///
/// Graph: source_a → pipeline → sink
///        source_b ↗
#[test]
fn test_eos_predecessor_does_not_block_successors() {
    let (tx, _rx) = mpsc::channel();
    let sender = StatisticsSender::new(tx);
    let executor = Executor::with_worker_count_and_stats(1, sender);
    let handle = executor.get_handle();

    let (source_a, source_a_ctrl) = controlled_source("source_a");
    let (source_b, source_b_ctrl) = controlled_source("source_b");
    let (pipeline, _pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source_a).unwrap();
    graph.add_source(source_b).unwrap();
    graph.add_pipeline(pipeline).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source_a"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("source_b"), &PipelineId::new("pipeline"))
        .unwrap();
    graph
        .connect(&PipelineId::new("pipeline"), &PipelineId::new("sink"))
        .unwrap();

    handle.deploy_graph(graph).unwrap();
    let exec_thread = thread::spawn(move || executor.run());

    assert!(source_a_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source_b_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Source A sends EOS (graceful stop for source A only)
    source_a_ctrl.end_of_stream();

    // Source B continues emitting — pipeline + sink should still process
    for i in 0..5 {
        source_b_ctrl.inject_buffer(identifiable_buffer(10 + i));
    }

    assert!(
        sink_ctrl.wait_for_buffers(5, DEFAULT_TIMEOUT),
        "Sink should receive buffers from source_b after source_a sent EOS"
    );

    // Now gracefully terminate
    source_b_ctrl.end_of_stream();
    assert!(source_b_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    handle.shutdown().unwrap();
    let exec_stats = exec_thread.join().unwrap();
    assert!(!exec_stats.has_errors());
}

/// Two separate graph deployments. A pipeline failure in one does not
/// affect the other — the executor processes a single merged graph, and
/// a pipeline failure is scoped to that pipeline only.
#[test]
fn test_separate_deployments_isolated() {
    let (tx, _rx) = mpsc::channel();
    let sender = StatisticsSender::new(tx);
    let executor = Executor::with_worker_count_and_stats(1, sender);
    let handle = executor.get_handle();

    // Graph 1: source1 -> pipeline1 -> sink1 (pipeline1 will fail)
    let (source1, source1_ctrl) = controlled_source("g1-source");
    let (pipeline1, pipeline1_ctrl) = controlled_pipeline("g1-pipeline");
    let (sink1, _sink1_ctrl) = capturing_sink("g1-sink");

    pipeline1_ctrl.fail_on_execute_nth(1); // Fail on first buffer

    let mut graph1 = PipelineGraph::new();
    graph1.add_source(source1).unwrap();
    graph1.add_pipeline(pipeline1).unwrap();
    graph1.add_pipeline(sink1).unwrap();
    graph1
        .connect(
            &PipelineId::new("g1-source"),
            &PipelineId::new("g1-pipeline"),
        )
        .unwrap();
    graph1
        .connect(&PipelineId::new("g1-pipeline"), &PipelineId::new("g1-sink"))
        .unwrap();

    // Graph 2: source2 -> pipeline2 -> sink2 (healthy)
    let (source2, source2_ctrl) = controlled_source("g2-source");
    let (pipeline2, _pipeline2_ctrl) = controlled_pipeline("g2-pipeline");
    let (sink2, sink2_ctrl) = capturing_sink("g2-sink");

    let mut graph2 = PipelineGraph::new();
    graph2.add_source(source2).unwrap();
    graph2.add_pipeline(pipeline2).unwrap();
    graph2.add_pipeline(sink2).unwrap();
    graph2
        .connect(
            &PipelineId::new("g2-source"),
            &PipelineId::new("g2-pipeline"),
        )
        .unwrap();
    graph2
        .connect(&PipelineId::new("g2-pipeline"), &PipelineId::new("g2-sink"))
        .unwrap();

    handle.deploy_graph(graph1).unwrap();
    handle.deploy_graph(graph2).unwrap();
    let exec_thread = thread::spawn(move || executor.run());

    assert!(source1_ctrl.wait_started(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Trigger failure in graph 1
    source1_ctrl.inject_buffer(identifiable_buffer(1));

    // Give time for the error to be processed
    thread::sleep(Duration::from_millis(200));

    // Graph 2 should still be fully functional
    for i in 0..5 {
        source2_ctrl.inject_buffer(identifiable_buffer(10 + i));
    }

    assert!(
        sink2_ctrl.wait_for_buffers(5, DEFAULT_TIMEOUT),
        "Graph 2 should continue working after graph 1's pipeline failed"
    );

    // Gracefully terminate both
    source1_ctrl.end_of_stream();
    source2_ctrl.end_of_stream();
    assert!(source1_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert!(source2_ctrl.wait_stopped(DEFAULT_TIMEOUT));

    handle.shutdown().unwrap();
    let exec_stats = exec_thread.join().unwrap();
    assert!(exec_stats.has_errors());
}
