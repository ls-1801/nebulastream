//! Integration tests for executor with internalized sources.

use adaptive_engine::executor::Executor;
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::mocks::{FilterPipeline, SinkPipeline};
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineId};
use adaptive_engine::source::{GeneratorConfig, GeneratorSource, Source, TestSource};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn test_single_source_to_sink() {
    // Create executor
    let executor = Executor::new();
    let handle = executor.get_handle();

    // Build graph: Source → Sink
    let mut graph = PipelineGraph::new();

    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(5)).with_max_buffers(5),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source_id, &sink_id).unwrap();

    // Deploy and run
    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    // Wait for source to complete
    thread::sleep(Duration::from_millis(100));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Verify: 5 buffers from source + 5 through sink = 10 total
    assert_eq!(stats.buffers_processed, 10);
    assert_eq!(stats.pipelines_started, 2); // source + sink
    assert_eq!(stats.errors_encountered, 0);
}

#[test]
fn test_multi_source_convergence() {
    // Create executor
    let executor = Executor::new();
    let handle = executor.get_handle();

    // Build graph: Source1 → Sink ← Source2
    let mut graph = PipelineGraph::new();

    let source1 = GeneratorSource::new(
        PipelineId::new("src1"),
        GeneratorConfig::new(Duration::from_millis(5)).with_max_buffers(3),
        |seq| Buffer::new(vec![1, seq as u8]),
    );

    let source2 = GeneratorSource::new(
        PipelineId::new("src2"),
        GeneratorConfig::new(Duration::from_millis(5)).with_max_buffers(3),
        |seq| Buffer::new(vec![2, seq as u8]),
    );

    let source1_id = source1.id().clone();
    let source2_id = source2.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source1)).unwrap();
    graph.add_source(Arc::new(source2)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source1_id, &sink_id).unwrap();
    graph.connect(&source2_id, &sink_id).unwrap();

    // Deploy and run
    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    // Wait for sources to complete
    thread::sleep(Duration::from_millis(100));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Verify: (3+3) buffers from sources + 6 through sink = 12 total
    assert_eq!(stats.buffers_processed, 12);
    assert_eq!(stats.pipelines_started, 3); // 2 sources + sink
    assert_eq!(stats.errors_encountered, 0);
}

#[test]
fn test_source_pipeline_sink_chain() {
    // Create executor
    let executor = Executor::new();
    let handle = executor.get_handle();

    // Build graph: Source → Filter → Sink
    let mut graph = PipelineGraph::new();

    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(5)).with_max_buffers(10),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    // Filter that passes everything
    let filter = FilterPipeline::new(PipelineId::new("filter"), |_| true);
    let filter_id = filter.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(filter)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source_id, &filter_id).unwrap();
    graph.connect(&filter_id, &sink_id).unwrap();

    // Deploy and run
    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    // Wait for source to complete
    thread::sleep(Duration::from_millis(150));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Verify: 10 from source + 10 through filter + 10 through sink = 30 total
    assert_eq!(stats.buffers_processed, 30);
    assert_eq!(stats.pipelines_started, 3); // source + filter + sink
    assert_eq!(stats.errors_encountered, 0);
}

#[test]
fn test_test_source_controlled_emission() {
    // Create executor
    let executor = Executor::new();
    let handle = executor.get_handle();

    // Build graph with TestSource
    let mut graph = PipelineGraph::new();

    let (source, source_handle) = TestSource::new(PipelineId::new("test-src"));
    let source_id = source.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source_id, &sink_id).unwrap();

    // Deploy and run
    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    // Give executor time to start source
    thread::sleep(Duration::from_millis(20));

    // Inject buffers via handle
    for i in 0..5 {
        let buffer = Buffer::new(vec![i]);
        source_handle.inject_buffer(buffer);
        thread::sleep(Duration::from_millis(5));
    }

    // Signal end of stream
    source_handle.end_of_stream();

    // Wait for processing
    thread::sleep(Duration::from_millis(50));

    // Shutdown
    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Verify: 5 buffers from source + 5 through sink = 10 total
    assert_eq!(stats.buffers_processed, 10);
    assert!(source_handle.is_stopped());
    assert_eq!(stats.errors_encountered, 0);
}

#[test]
fn test_source_starts_after_pipelines_setup() {
    // This test verifies that sources only start after successor pipelines are set up
    let executor = Executor::new();
    let handle = executor.get_handle();

    let mut graph = PipelineGraph::new();

    // Create a source that emits immediately
    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(1)).with_max_buffers(3),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source_id, &sink_id).unwrap();

    // Deploy
    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    // Wait for completion
    thread::sleep(Duration::from_millis(50));

    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // All buffers should be processed (no errors from unready pipelines)
    assert_eq!(stats.buffers_processed, 6); // 3 from source + 3 through sink
    assert_eq!(stats.errors_encountered, 0);
}

#[test]
fn test_generator_source_respects_max_buffers() {
    let executor = Executor::new();
    let handle = executor.get_handle();

    let mut graph = PipelineGraph::new();

    // Source configured to emit exactly 7 buffers
    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(5)).with_max_buffers(7),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source_id, &sink_id).unwrap();

    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    // Wait for source to complete
    thread::sleep(Duration::from_millis(100));

    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // Should process exactly 7 buffers from source + 7 through sink = 14 total
    assert_eq!(stats.buffers_processed, 14);
    assert_eq!(stats.errors_encountered, 0);
}

#[test]
fn test_sources_identified_correctly() {
    let mut graph = PipelineGraph::new();

    let source1 = GeneratorSource::new(
        PipelineId::new("src1"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );

    let filter = FilterPipeline::new(PipelineId::new("filter"), |_| true);
    let sink = SinkPipeline::new("sink");

    let source1_id = source1.id().clone();
    let filter_id = filter.id().clone();
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source1)).unwrap();
    graph.add_pipeline(Box::new(filter)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();

    // Check identification
    assert!(graph.is_source(&source1_id));
    assert!(!graph.is_source(&filter_id));
    assert!(!graph.is_source(&sink_id));
}

#[test]
fn test_diamond_topology_with_sources() {
    let executor = Executor::new();
    let handle = executor.get_handle();

    // Build diamond: Source → A → C
    //                        ↘ B ↗
    let mut graph = PipelineGraph::new();

    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(10)).with_max_buffers(3),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let a = FilterPipeline::new(PipelineId::new("a"), |_| true);
    let b = FilterPipeline::new(PipelineId::new("b"), |_| true);
    let c = SinkPipeline::new("c");

    let a_id = a.id().clone();
    let b_id = b.id().clone();
    let c_id = c.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(a)).unwrap();
    graph.add_pipeline(Box::new(b)).unwrap();
    graph.add_pipeline(Box::new(c)).unwrap();

    graph.connect(&source_id, &a_id).unwrap();
    graph.connect(&source_id, &b_id).unwrap();
    graph.connect(&a_id, &c_id).unwrap();
    graph.connect(&b_id, &c_id).unwrap();

    handle.deploy_graph(graph).unwrap();
    let exec_handle = thread::spawn(move || executor.run());

    thread::sleep(Duration::from_millis(100));

    handle.shutdown().unwrap();
    let stats = exec_handle.join().unwrap();

    // 3 from source, 3 through A, 3 through B, 6 through C = 15 total
    assert_eq!(stats.buffers_processed, 15);
    assert_eq!(stats.errors_encountered, 0);
}
