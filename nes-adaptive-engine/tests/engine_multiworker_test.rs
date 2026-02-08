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

//! Multi-worker engine tests.
//!
//! Tests that the engine works correctly with multiple worker threads,
//! verifying correctness, parallelism, shutdown, error handling, and
//! data integrity.

mod common;

use adaptive_engine::engine::Engine;
use adaptive_engine::executor::PipelineExecutionContext;
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use common::capturing_sink::capturing_sink;
use common::controlled_pipeline::controlled_pipeline;
use common::controlled_source::controlled_source;
use common::stats_collector::StatsCollector;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn identifiable_buffer(id: u8) -> Buffer {
    let mut data = vec![0u8; 64];
    data[0] = id;
    Buffer::new(data)
}

/// Test 1: Multi-worker basic — deploy graph, emit buffers, verify all processed.
#[test]
fn test_multiworker_basic() {
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(4);
    let stats = StatsCollector::new(receiver);

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

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject many buffers
    for i in 0..100 {
        source_ctrl.inject_buffer(identifiable_buffer((i % 256) as u8));
    }

    // Wait for all buffers at sink
    assert!(sink_ctrl.wait_for_buffers(100, DEFAULT_TIMEOUT));
    assert_eq!(sink_ctrl.buffer_count(), 100);

    let exec_stats = engine.shutdown();
    assert!(source_ctrl.wait_stopped(DEFAULT_TIMEOUT));
    assert_eq!(
        exec_stats.buffers_processed + exec_stats.tasks_skipped,
        exec_stats
            .tasks_executed
            .saturating_sub(exec_stats.graphs_deployed)
            .saturating_sub(exec_stats.pipelines_stopped)
            .saturating_sub(1) // shutdown task
            .min(exec_stats.buffers_processed + exec_stats.tasks_skipped)
    );
    assert!(exec_stats.buffers_processed >= 100);
}

/// Test 2: Multi-worker shutdown — clean shutdown with N workers.
#[test]
fn test_multiworker_shutdown() {
    let (mut engine, _receiver) = Engine::with_worker_count_and_stats(4);
    engine.start();
    let exec_stats = engine.shutdown();
    assert_eq!(exec_stats.errors_encountered, 0);
}

/// Test 3: Multi-worker error handling — pipeline error triggers correct query termination.
#[test]
fn test_multiworker_error_handling() {
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(4);
    let _stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline, pipeline_ctrl) = controlled_pipeline("pipeline");
    let (sink, _sink_ctrl) = capturing_sink("sink");

    // Configure pipeline to fail on 3rd invocation
    pipeline_ctrl.fail_on_execute_nth(3);

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
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject enough buffers to trigger the error
    for _ in 0..10 {
        source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    }

    // Wait for the error to propagate and shutdown
    let exec_stats = engine.shutdown();
    assert!(exec_stats.has_errors());
    assert!(exec_stats.errors_encountered > 0);
}

/// Test 4: Multi-worker stop_query — stop query while workers are processing buffers.
#[test]
fn test_multiworker_stop_query() {
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(4);
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (sink, _sink_ctrl) = capturing_sink("sink");

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

    // Inject some buffers
    for _ in 0..20 {
        source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    }

    // Stop the query
    let stopped = engine.stop_query(query_id).unwrap();
    assert!(stopped);

    // Should shutdown cleanly
    let exec_stats = engine.shutdown();
    assert_eq!(exec_stats.errors_encountered, 0);
}

/// Test 5: Worker ID correctness — statistics events contain worker IDs in [0, N).
#[test]
fn test_multiworker_worker_id_correctness() {
    let worker_count = 4;
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(worker_count);
    let stats = StatsCollector::new(receiver);

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

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject buffers
    for _ in 0..50 {
        source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
    }

    assert!(sink_ctrl.wait_for_buffers(50, DEFAULT_TIMEOUT));

    source_ctrl.end_of_stream();

    let _exec_stats = engine.shutdown();

    // All worker IDs in task execution events should be in [0, N)
    // Stop the collector to ensure all events are drained from the channel
    let events = stats.get_events();
    for event in &events {
        use adaptive_engine::executor::stats::StatisticsEvent;
        match event {
            StatisticsEvent::TaskExecutionStart { worker_id, .. }
            | StatisticsEvent::TaskExecutionComplete { worker_id, .. } => {
                assert!(
                    (*worker_id as usize) < worker_count,
                    "worker_id {} out of range [0, {})",
                    worker_id,
                    worker_count
                );
            }
            _ => {}
        }
    }
}

/// Test 6: Stress test — high buffer count, verify no data loss or duplicate processing.
#[test]
fn test_multiworker_stress_no_data_loss() {
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(4);
    let _stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let _query_id = engine.submit_query(graph).unwrap();

    // Wait for source to start before injecting
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    let buffer_count = 1000;
    for i in 0..buffer_count {
        let mut data = vec![0u8; 64];
        // Encode buffer index in first 4 bytes
        let bytes = (i as u32).to_le_bytes();
        data[..4].copy_from_slice(&bytes);
        source_ctrl.inject_buffer(Buffer::new(data));
    }

    // Wait for all buffers
    assert!(sink_ctrl.wait_for_buffers(buffer_count, DEFAULT_TIMEOUT));

    // Signal end of stream for clean shutdown
    source_ctrl.end_of_stream();

    let exec_stats = engine.shutdown();
    assert!(!exec_stats.has_errors());

    // Verify no data loss: exactly buffer_count buffers received
    let buffers = sink_ctrl.take_buffers();
    assert_eq!(buffers.len(), buffer_count);

    // Verify all buffer indices are present (no duplicates, no loss)
    let mut seen = vec![false; buffer_count];
    for buf in &buffers {
        let idx = u32::from_le_bytes(buf.data()[..4].try_into().unwrap()) as usize;
        assert!(idx < buffer_count, "Buffer index {} out of range", idx);
        assert!(!seen[idx], "Duplicate buffer with index {}", idx);
        seen[idx] = true;
    }
    for (i, &was_seen) in seen.iter().enumerate() {
        assert!(was_seen, "Buffer with index {} was not received", i);
    }
}

/// Test 7: Multi-worker with pipeline chain — ensures data flows correctly through
/// a multi-stage pipeline with multiple workers.
#[test]
fn test_multiworker_pipeline_chain() {
    let (mut engine, receiver) = Engine::with_worker_count_and_stats(4);
    let stats = StatsCollector::new(receiver);

    let (source, source_ctrl) = controlled_source("source");
    let (pipeline1, _) = controlled_pipeline("p1");
    let (pipeline2, _) = controlled_pipeline("p2");
    let (sink, sink_ctrl) = capturing_sink("sink");

    let mut graph = PipelineGraph::new();
    graph.add_source(source).unwrap();
    graph.add_pipeline(pipeline1).unwrap();
    graph.add_pipeline(pipeline2).unwrap();
    graph.add_pipeline(sink).unwrap();
    graph
        .connect(&PipelineId::new("source"), &PipelineId::new("p1"))
        .unwrap();
    graph
        .connect(&PipelineId::new("p1"), &PipelineId::new("p2"))
        .unwrap();
    graph
        .connect(&PipelineId::new("p2"), &PipelineId::new("sink"))
        .unwrap();

    engine.start();
    let query_id = engine.submit_query(graph).unwrap();

    assert!(stats.wait_for_query_running_id(query_id, DEFAULT_TIMEOUT));
    assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

    // Inject buffers
    let num_buffers = 50;
    for i in 0..num_buffers {
        source_ctrl.inject_buffer(identifiable_buffer((i % 256) as u8));
    }

    // Wait for all buffers at sink
    assert!(sink_ctrl.wait_for_buffers(num_buffers, DEFAULT_TIMEOUT));

    // End of stream for graceful shutdown
    source_ctrl.end_of_stream();

    let exec_stats = engine.shutdown();
    assert!(!exec_stats.has_errors());
    assert!(exec_stats.buffers_processed >= num_buffers * 3); // p1 + p2 + sink
}

/// Slow pipeline that sleeps during execute to test parallelism.
struct SlowPipeline {
    id: PipelineId,
    sleep_ms: u64,
    execute_count: Arc<AtomicUsize>,
}

impl Pipeline for SlowPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        std::thread::sleep(Duration::from_millis(self.sleep_ms));
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![input])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Test 8: Parallel execution — pipeline with sleep in execute(), verify wall-clock speedup.
#[test]
fn test_multiworker_parallel_execution() {
    let sleep_ms = 50;
    let num_buffers = 8;
    let worker_count = 4;

    // Run with 1 worker
    let single_time = {
        let (mut engine, _) = Engine::with_worker_count_and_stats(1);
        let (source, source_ctrl) = controlled_source("source");
        let execute_count = Arc::new(AtomicUsize::new(0));
        let pipeline = Box::new(SlowPipeline {
            id: PipelineId::new("slow"),
            sleep_ms,
            execute_count: execute_count.clone(),
        });
        let (sink, sink_ctrl) = capturing_sink("sink");

        let mut graph = PipelineGraph::new();
        graph.add_source(source).unwrap();
        graph.add_pipeline(pipeline).unwrap();
        graph.add_pipeline(sink).unwrap();
        graph
            .connect(&PipelineId::new("source"), &PipelineId::new("slow"))
            .unwrap();
        graph
            .connect(&PipelineId::new("slow"), &PipelineId::new("sink"))
            .unwrap();

        engine.start();
        engine.submit_query(graph).unwrap();
        assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

        let start = std::time::Instant::now();
        for _ in 0..num_buffers {
            source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
        }
        assert!(sink_ctrl.wait_for_buffers(num_buffers, DEFAULT_TIMEOUT));
        let elapsed = start.elapsed();
        engine.shutdown();
        elapsed
    };

    // Run with N workers
    let multi_time = {
        let (mut engine, _) = Engine::with_worker_count_and_stats(worker_count);
        let (source, source_ctrl) = controlled_source("source");
        let execute_count = Arc::new(AtomicUsize::new(0));
        let pipeline = Box::new(SlowPipeline {
            id: PipelineId::new("slow"),
            sleep_ms,
            execute_count: execute_count.clone(),
        });
        let (sink, sink_ctrl) = capturing_sink("sink");

        let mut graph = PipelineGraph::new();
        graph.add_source(source).unwrap();
        graph.add_pipeline(pipeline).unwrap();
        graph.add_pipeline(sink).unwrap();
        graph
            .connect(&PipelineId::new("source"), &PipelineId::new("slow"))
            .unwrap();
        graph
            .connect(&PipelineId::new("slow"), &PipelineId::new("sink"))
            .unwrap();

        engine.start();
        engine.submit_query(graph).unwrap();
        assert!(source_ctrl.wait_started(DEFAULT_TIMEOUT));

        let start = std::time::Instant::now();
        for _ in 0..num_buffers {
            source_ctrl.inject_buffer(Buffer::new(vec![0; 64]));
        }
        assert!(sink_ctrl.wait_for_buffers(num_buffers, DEFAULT_TIMEOUT));
        let elapsed = start.elapsed();
        engine.shutdown();
        elapsed
    };

    // Multi-worker should be significantly faster (at least 1.5x speedup)
    // With 4 workers and 8 tasks of 50ms each:
    //   Single: ~400ms, Multi: ~100ms
    let speedup = single_time.as_millis() as f64 / multi_time.as_millis().max(1) as f64;
    assert!(
        speedup > 1.5,
        "Expected speedup > 1.5x, got {:.2}x (single: {:?}, multi: {:?})",
        speedup,
        single_time,
        multi_time
    );
}
