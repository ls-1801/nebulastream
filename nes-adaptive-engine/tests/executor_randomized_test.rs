//! Randomized and stress tests for the execution engine.
//!
//! These tests use different queue implementations and large data volumes
//! to verify system correctness under various conditions.

use adaptive_engine::executor::{Executor, FifoQueue, LifoQueue, PriorityQueue, RandomQueue};
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::mocks::{FilterPipeline, MultibufferPipeline, SinkPipeline};
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineId};
use adaptive_engine::sequence::SequenceNumber;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Helper to create a random linear graph with N pipelines.
fn create_random_linear_graph(pipeline_count: usize) -> (PipelineGraph, Vec<PipelineId>) {
    let mut graph = PipelineGraph::new();
    let mut pipeline_ids = Vec::new();

    // Create pipelines
    for i in 0..pipeline_count {
        let pipeline_id = PipelineId::new(format!("pipeline_{}", i));
        let pipeline = if i % 2 == 0 {
            // Filter (passes through)
            Box::new(FilterPipeline::new(pipeline_id.clone(), |_| true)) as Box<dyn Pipeline>
        } else {
            // Multibuffer (emits 2 buffers)
            Box::new(MultibufferPipeline::new(pipeline_id.clone(), 2)) as Box<dyn Pipeline>
        };

        graph.add_pipeline(pipeline).unwrap();
        pipeline_ids.push(pipeline_id);
    }

    // Connect them linearly
    for i in 0..pipeline_count - 1 {
        graph
            .connect(&pipeline_ids[i], &pipeline_ids[i + 1])
            .unwrap();
    }

    (graph, pipeline_ids)
}

/// Test with RandomQueue - non-deterministic ordering.
#[test]
fn test_random_queue_execution() {
    let executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    // Build simple graph
    let mut graph = PipelineGraph::new();
    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();
    graph.add_pipeline(Box::new(sink)).unwrap();

    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start pipeline
    thread::sleep(Duration::from_millis(10));

    // Emit 50 buffers
    for i in 0..50 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        handle.emit(sink_id.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(100));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // All buffers should be processed despite random ordering
    assert_eq!(stats.buffers_processed, 50);
    assert_eq!(stats.errors_encountered, 0);
}

/// Test with LifoQueue - stack-based ordering.
#[test]
fn test_lifo_queue_execution() {
    let executor = Executor::with_queue(LifoQueue::new());
    let handle = executor.get_handle();

    // Build simple graph
    let mut graph = PipelineGraph::new();
    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();
    graph.add_pipeline(Box::new(sink)).unwrap();

    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start pipeline
    thread::sleep(Duration::from_millis(10));

    // Emit 30 buffers
    for i in 0..30 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        handle.emit(sink_id.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(100));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // All buffers should be processed (order doesn't matter for correctness)
    assert_eq!(stats.buffers_processed, 30);
    assert_eq!(stats.errors_encountered, 0);
}

/// Test with PriorityQueue - lifecycle operations prioritized.
#[test]
fn test_priority_queue_execution() {
    let executor = Executor::with_queue(PriorityQueue::new());
    let handle = executor.get_handle();

    // Build simple graph
    let mut graph = PipelineGraph::new();
    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();
    graph.add_pipeline(Box::new(sink)).unwrap();

    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start pipeline
    thread::sleep(Duration::from_millis(10));

    // Emit buffers
    for i in 0..20 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        handle.emit(sink_id.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(100));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // All buffers processed (priority doesn't affect correctness)
    assert_eq!(stats.buffers_processed, 20);
    assert_eq!(stats.errors_encountered, 0);
}

/// Stress test: Large number of buffers with RandomQueue.
#[test]
fn test_large_data_volume_random_queue() {
    let executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    // Build linear pipeline
    let (graph, pipeline_ids) = create_random_linear_graph(5);
    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start all pipelines
    for _id in &pipeline_ids {}
    thread::sleep(Duration::from_millis(10));

    // Emit 500 buffers to first pipeline
    let first_pipeline = &pipeline_ids[0];
    for i in 0..500 {
        let buffer = Buffer::new(vec![i as u8; 100], SequenceNumber::new(i));
        handle.emit(first_pipeline.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(500));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Should process many buffers (multibuffer pipelines create more)
    assert!(stats.buffers_processed >= 500);
    assert_eq!(stats.errors_encountered, 0);
}

/// Test: Concurrent sources with different queue implementations.
#[test]
fn test_concurrent_sources_random_queue() {
    let executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    // Build convergence graph
    let mut graph = PipelineGraph::new();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_pipeline(Box::new(sink)).unwrap();

    let mut source_ids = Vec::new();
    for i in 0..5 {
        let source = FilterPipeline::new(PipelineId::new(format!("source_{}", i)), |_| true);
        let source_id = source.id().clone();
        graph.add_pipeline(Box::new(source)).unwrap();
        graph.connect(&source_id, &sink_id).unwrap();
        source_ids.push(source_id);
    }

    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start all pipelines
    for _id in &source_ids {}
    thread::sleep(Duration::from_millis(10));

    // Spawn 5 source threads
    let mut threads = vec![];
    for (idx, source_id) in source_ids.iter().enumerate() {
        let handle_clone = handle.clone();
        let source_id_clone = source_id.clone();

        let t = thread::spawn(move || {
            for i in 0..100 {
                let buffer = Buffer::new(
                    vec![idx as u8, i as u8],
                    SequenceNumber::new((idx * 1000 + i) as u64),
                );
                handle_clone.emit(source_id_clone.clone(), buffer).unwrap();
                // Small random delay
                if i % 10 == 0 {
                    thread::sleep(Duration::from_micros(100));
                }
            }
        });
        threads.push(t);
    }

    // Wait for all sources
    for t in threads {
        t.join().unwrap();
    }

    thread::sleep(Duration::from_millis(200));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Should process 5 sources × 100 buffers = 500 through sources
    // Plus 500 through sink = 1000 total
    assert_eq!(stats.buffers_processed, 1000);
    assert_eq!(stats.errors_encountered, 0);
}

/// Property test: No data loss with RandomQueue.
#[test]
fn test_property_no_data_loss_random_queue() {
    let executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    // Track buffers using shared state
    let received_buffers = Arc::new(Mutex::new(Vec::new()));

    // Create custom sink that tracks received buffers
    struct TrackingSink {
        id: PipelineId,
        received: Arc<Mutex<Vec<u64>>>,
    }

    impl Pipeline for TrackingSink {
        fn execute(
            &self,
            input: Buffer,
            _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
        ) -> Result<Vec<Buffer>, adaptive_engine::pipeline::PipelineError> {
            // Extract sequence number
            let seq_str = input.sequence().to_string();
            if let Ok(seq_num) = seq_str.parse::<u64>() {
                self.received.lock().unwrap().push(seq_num);
            }
            Ok(vec![])
        }

        fn id(&self) -> &PipelineId {
            &self.id
        }
    }

    unsafe impl Send for TrackingSink {}
    unsafe impl Sync for TrackingSink {}

    // Build graph with tracking sink
    let mut graph = PipelineGraph::new();
    let sink = TrackingSink {
        id: PipelineId::new("sink"),
        received: Arc::clone(&received_buffers),
    };
    let sink_id = sink.id().clone();
    graph.add_pipeline(Box::new(sink)).unwrap();

    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start pipeline
    thread::sleep(Duration::from_millis(10));

    // Emit 200 buffers with unique sequence numbers
    let num_buffers = 200;
    for i in 0..num_buffers {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        handle.emit(sink_id.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(300));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Verify all buffers received
    let received = received_buffers.lock().unwrap();
    assert_eq!(received.len(), num_buffers as usize);

    // Verify no duplicates
    let unique: HashSet<_> = received.iter().cloned().collect();
    assert_eq!(unique.len(), num_buffers as usize);

    // Verify all sequence numbers present (0..num_buffers)
    for i in 0..num_buffers {
        assert!(unique.contains(&i), "Missing buffer with sequence {}", i);
    }

    assert_eq!(stats.buffers_processed, num_buffers as usize);
}

/// Stress test: Rapid graph replacement with RandomQueue.
#[test]
fn test_rapid_graph_replacement_random_queue() {
    let executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    // Deploy initial graph
    let mut graph1 = PipelineGraph::new();
    let sink1 = SinkPipeline::new("sink1");
    let sink1_id = sink1.id().clone();
    graph1.add_pipeline(Box::new(sink1)).unwrap();
    handle.deploy_graph(graph1).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start first pipeline
    thread::sleep(Duration::from_millis(10));

    // Emit to first graph
    for i in 0..50 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        handle.emit(sink1_id.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(50));

    // Deploy second graph (replacement)
    let mut graph2 = PipelineGraph::new();
    let sink2 = SinkPipeline::new("sink2");
    let sink2_id = sink2.id().clone();
    graph2.add_pipeline(Box::new(sink2)).unwrap();
    handle.deploy_graph(graph2).unwrap();

    thread::sleep(Duration::from_millis(10));

    // Start second pipeline
    thread::sleep(Duration::from_millis(10));

    // Emit to second graph
    for i in 0..50 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i + 50));
        handle.emit(sink2_id.clone(), buffer).unwrap();
    }

    thread::sleep(Duration::from_millis(50));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Should have processed 100 buffers across both graphs
    assert_eq!(stats.buffers_processed, 100);
    assert_eq!(stats.graphs_deployed, 2);
    assert_eq!(stats.pipelines_started, 2);
}

/// Test: All queue implementations produce same results.
#[test]
fn test_queue_implementations_equivalence() {
    let test_with_queue = |queue_name: &str, executor: Executor| -> usize {
        let handle = executor.get_handle();

        // Build simple graph
        let mut graph = PipelineGraph::new();
        let sink = SinkPipeline::new("sink");
        let sink_id = sink.id().clone();
        graph.add_pipeline(Box::new(sink)).unwrap();

        handle.deploy_graph(graph).unwrap();

        // Spawn execution thread
        let exec_handle = thread::spawn(move || executor.run());
        thread::sleep(Duration::from_millis(10));

        // Start pipeline
        thread::sleep(Duration::from_millis(10));

        // Emit 100 buffers
        for i in 0..100 {
            let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
            handle.emit(sink_id.clone(), buffer).unwrap();
        }

        thread::sleep(Duration::from_millis(150));

        // Shutdown
        handle.shutdown().unwrap();
        let stats = exec_handle.join().unwrap();

        println!(
            "{}: {} buffers processed",
            queue_name, stats.buffers_processed
        );

        stats.buffers_processed
    };

    // Test with all queue implementations
    let fifo_count = test_with_queue("FifoQueue", Executor::with_queue(FifoQueue::new()));
    let lifo_count = test_with_queue("LifoQueue", Executor::with_queue(LifoQueue::new()));
    let random_count = test_with_queue("RandomQueue", Executor::with_queue(RandomQueue::new()));
    let priority_count =
        test_with_queue("PriorityQueue", Executor::with_queue(PriorityQueue::new()));

    // All should process exactly 100 buffers
    assert_eq!(fifo_count, 100);
    assert_eq!(lifo_count, 100);
    assert_eq!(random_count, 100);
    assert_eq!(priority_count, 100);
}

/// Stress test: Complex graph with many pipelines and lots of data.
#[test]
fn test_stress_complex_graph() {
    let executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    // Build complex graph: 3 sources → 3 transforms → 1 sink
    let mut graph = PipelineGraph::new();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    let mut source_ids = Vec::new();
    let mut transform_ids = Vec::new();

    // Create sources
    for i in 0..3 {
        let source = FilterPipeline::new(PipelineId::new(format!("source_{}", i)), |_| true);
        let source_id = source.id().clone();
        graph.add_pipeline(Box::new(source)).unwrap();
        source_ids.push(source_id);
    }

    // Create transforms (multibuffer pipelines that amplify data)
    for i in 0..3 {
        let transform = MultibufferPipeline::new(PipelineId::new(format!("transform_{}", i)), 3);
        let transform_id = transform.id().clone();
        graph.add_pipeline(Box::new(transform)).unwrap();
        transform_ids.push(transform_id);
    }

    graph.add_pipeline(Box::new(sink)).unwrap();

    // Connect: each source → corresponding transform → sink
    for i in 0..3 {
        graph.connect(&source_ids[i], &transform_ids[i]).unwrap();
        graph.connect(&transform_ids[i], &sink_id).unwrap();
    }

    handle.deploy_graph(graph).unwrap();

    // Spawn execution thread
    let exec_handle = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Start all pipelines
    for _id in &source_ids {}
    for _id in &transform_ids {}
    thread::sleep(Duration::from_millis(10));

    // Emit 200 buffers to each source
    for source_id in &source_ids {
        for i in 0..200 {
            let buffer = Buffer::new(vec![i as u8; 50], SequenceNumber::new(i));
            handle.emit(source_id.clone(), buffer).unwrap();
        }
    }

    thread::sleep(Duration::from_millis(500));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // 3 sources × 200 buffers = 600 processed through sources
    // 600 buffers processed through transforms (input)
    // Each transform emits 3 buffers per input, so 600 × 3 = 1800 to sink
    // 1800 processed through sink
    // Total: 600 + 600 + 1800 = 3000 buffers
    assert_eq!(stats.buffers_processed, 3000);
    assert_eq!(stats.errors_encountered, 0);
}
