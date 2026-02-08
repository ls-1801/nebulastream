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

//! Unit tests for source functionality.

use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::mocks::SinkPipeline;
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineId};
use adaptive_engine::source::{GeneratorConfig, GeneratorSource, Source, TestSource};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_source_creation() {
    let (source, _handle) = TestSource::new(PipelineId::new("test"));
    assert_eq!(source.id().as_str(), "test");
}

#[test]
fn test_source_handle_control() {
    let (_source, handle) = TestSource::new(PipelineId::new("test-src"));

    // Initially not stopped
    assert!(!handle.is_stopped());

    // Can inject buffers without starting (they'll be queued)
    let buffer = Buffer::new(vec![1, 2, 3]);
    handle.inject_buffer(buffer);

    // Can inject error
    handle.inject_error("Test error".to_string());

    // Signal end of stream
    handle.end_of_stream();
}

#[test]
fn test_generator_source_config() {
    let config = GeneratorConfig::new(Duration::from_millis(100));
    assert_eq!(config.emit_interval, Duration::from_millis(100));
    assert_eq!(config.max_buffers, None);

    let config_limited = config.with_max_buffers(10);
    assert_eq!(config_limited.max_buffers, Some(10));
}

#[test]
fn test_generator_source_creation() {
    let config = GeneratorConfig::new(Duration::from_millis(50));
    let source = GeneratorSource::new(PipelineId::new("gen"), config, |seq| {
        Buffer::new(vec![seq as u8])
    });

    assert_eq!(source.id().as_str(), "gen");
    assert_eq!(source.buffer_count(), 0);
    assert!(!source.is_stopped());
}

#[test]
fn test_graph_rejects_source_with_predecessors() {
    let mut graph = PipelineGraph::new();

    // Create a source and a pipeline
    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let pipeline = SinkPipeline::new("pipeline");
    let pipeline_id = pipeline.id().clone();

    // Add both to graph
    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(pipeline)).unwrap();

    // Connect pipeline to source (backwards - should fail validation)
    graph.connect(&pipeline_id, &source_id).unwrap();

    // Validation should fail because source has a predecessor
    let result = graph.validate();
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Source"));
    assert!(err_msg.contains("predecessor"));
}

#[test]
fn test_graph_accepts_source_without_predecessors() {
    let mut graph = PipelineGraph::new();

    // Create a source and a sink
    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    // Add both to graph
    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();

    // Connect source to sink (correct direction)
    graph.connect(&source_id, &sink_id).unwrap();

    // Validation should succeed
    assert!(graph.validate().is_ok());
}

#[test]
fn test_is_source_check() {
    let mut graph = PipelineGraph::new();

    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    graph.add_source(Arc::new(source)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();

    // Check is_source
    assert!(graph.is_source(&source_id));
    assert!(!graph.is_source(&sink_id));
}

#[test]
fn test_get_source_pipeline() {
    let mut graph = PipelineGraph::new();

    let source = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );
    let source_id = source.id().clone();

    graph.add_source(Arc::new(source)).unwrap();

    // Should be able to get source pipeline
    let source_pipeline = graph.get_source_pipeline(&source_id);
    assert!(source_pipeline.is_some());

    // Non-existent source should return None
    let missing = graph.get_source_pipeline(&PipelineId::new("missing"));
    assert!(missing.is_none());
}

#[test]
fn test_test_source_lifecycle() {
    let (source, handle) = TestSource::new(PipelineId::new("test"));

    // Setup should succeed
    assert!(source.setup().is_ok());

    // Stop should succeed
    assert!(source.stop().is_ok());
    assert!(handle.is_stopped());

    // Teardown should succeed
    assert!(source.teardown().is_ok());
}

#[test]
fn test_generator_source_lifecycle() {
    let config = GeneratorConfig::new(Duration::from_millis(10));
    let source = GeneratorSource::new(PipelineId::new("gen"), config, |seq| {
        Buffer::new(vec![seq as u8])
    });

    // Setup should succeed (default implementation)
    assert!(source.setup().is_ok());

    // Stop should succeed
    assert!(source.stop().is_ok());
    assert!(source.is_stopped());

    // Teardown should succeed
    assert!(source.teardown().is_ok());
}

#[test]
fn test_source_duplicate_id_rejected() {
    let mut graph = PipelineGraph::new();

    let source1 = GeneratorSource::new(
        PipelineId::new("src"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );

    let source2 = GeneratorSource::new(
        PipelineId::new("src"), // Same ID
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![seq as u8]),
    );

    // First add should succeed
    assert!(graph.add_source(Arc::new(source1)).is_ok());

    // Second add should fail (duplicate ID)
    let result = graph.add_source(Arc::new(source2));
    assert!(result.is_err());
}

#[test]
fn test_multiple_sources_in_graph() {
    let mut graph = PipelineGraph::new();

    let source1 = GeneratorSource::new(
        PipelineId::new("src1"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![1, seq as u8]),
    );

    let source2 = GeneratorSource::new(
        PipelineId::new("src2"),
        GeneratorConfig::new(Duration::from_millis(10)),
        |seq| Buffer::new(vec![2, seq as u8]),
    );

    let sink = SinkPipeline::new("sink");
    let sink_id = sink.id().clone();

    // Add sources and sink
    graph.add_source(Arc::new(source1)).unwrap();
    graph.add_source(Arc::new(source2)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();

    // Connect both sources to sink
    graph.connect(&PipelineId::new("src1"), &sink_id).unwrap();
    graph.connect(&PipelineId::new("src2"), &sink_id).unwrap();

    // Should validate
    assert!(graph.validate().is_ok());

    // Both should be identified as sources
    assert!(graph.is_source(&PipelineId::new("src1")));
    assert!(graph.is_source(&PipelineId::new("src2")));
    assert!(!graph.is_source(&sink_id));
}
