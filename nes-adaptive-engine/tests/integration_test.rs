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

//! Integration tests for the pipeline builder API.
//!
//! These tests verify that the pipeline builder API works correctly
//! for constructing and validating various pipeline graph configurations.

mod common;

use adaptive_engine::builder::PipelineGraphBuilder;
use adaptive_engine::executor::ExecutorContext;
use adaptive_engine::graph::GraphError;
use adaptive_engine::pipeline::PipelineId;
use adaptive_engine::pipeline::mocks::{
    FilterPipeline, MultibufferPipeline, OccasionalEmissionPipeline, StatefulPipeline,
};
use common::generate_test_buffers;

// Helper function to create a test context
fn create_test_context() -> ExecutorContext {
    use std::sync::mpsc::channel;
    let (tx, _rx) = channel();
    ExecutorContext::new(PipelineId::new("test"), 0, 1, tx)
}

#[test]
fn test_simple_filter_pipeline() {
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(FilterPipeline::new(
            PipelineId::new("filter"),
            |b| b.data().len() > 5,
        )))
        .build()
        .expect("Failed to build graph");

    assert_eq!(graph.len(), 1);

    let filter = graph.get_pipeline(&PipelineId::new("filter")).unwrap();
    let buffers = generate_test_buffers(10, 8);
    let context = create_test_context();

    for buffer in buffers {
        let result = filter.execute(buffer, &context).unwrap();
        assert_eq!(result.len(), 1); // All should pass (size 8 > 5)
    }
}

#[test]
fn test_filter_blocks_small_buffers() {
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(FilterPipeline::new(
            PipelineId::new("filter"),
            |b| b.data().len() > 10,
        )))
        .build()
        .expect("Failed to build graph");

    let filter = graph.get_pipeline(&PipelineId::new("filter")).unwrap();
    let buffers = generate_test_buffers(5, 8); // Size 8 < 10
    let context = create_test_context();

    for buffer in buffers {
        let result = filter.execute(buffer, &context).unwrap();
        assert_eq!(result.len(), 0); // All should be filtered out
    }
}

#[test]
fn test_multibuffer_fanout() {
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(MultibufferPipeline::new(
            PipelineId::new("fanout"),
            5,
        )))
        .build()
        .expect("Failed to build graph");

    let fanout = graph.get_pipeline(&PipelineId::new("fanout")).unwrap();
    let input = generate_test_buffers(1, 10).into_iter().next().unwrap();
    let context = create_test_context();

    let outputs = fanout.execute(input, &context).unwrap();
    assert_eq!(outputs.len(), 5);
}

#[test]
fn test_occasional_emission() {
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(OccasionalEmissionPipeline::new(
            PipelineId::new("window"),
            3,
        )))
        .build()
        .expect("Failed to build graph");

    let window = graph.get_pipeline(&PipelineId::new("window")).unwrap();
    let buffers = generate_test_buffers(10, 8);
    let context = create_test_context();

    let mut emission_count = 0;
    for buffer in buffers {
        let result = window.execute(buffer, &context).unwrap();
        if !result.is_empty() {
            emission_count += 1;
        }
    }

    // Should emit every 3rd buffer: buffers 3, 6, 9 = 3 emissions
    assert_eq!(emission_count, 3);
}

#[test]
fn test_stateful_pipeline() {
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(StatefulPipeline::new(PipelineId::new("state"))))
        .build()
        .expect("Failed to build graph");

    let stateful = graph.get_pipeline(&PipelineId::new("state")).unwrap();
    let input = generate_test_buffers(1, 10).into_iter().next().unwrap();
    let context = create_test_context();

    let output = stateful.execute(input, &context).unwrap();
    assert_eq!(output.len(), 1);
}

#[test]
fn test_linear_pipeline() {
    // Create a linear pipeline: filter -> fanout -> window
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(FilterPipeline::new(
            PipelineId::new("filter"),
            |b| b.data().len() > 5,
        )))
        .add_pipeline(Box::new(MultibufferPipeline::new(
            PipelineId::new("fanout"),
            2,
        )))
        .add_pipeline(Box::new(OccasionalEmissionPipeline::new(
            PipelineId::new("window"),
            2,
        )))
        .connect(PipelineId::new("filter"), PipelineId::new("fanout"))
        .connect(PipelineId::new("fanout"), PipelineId::new("window"))
        .build()
        .expect("Failed to build graph");

    assert_eq!(graph.len(), 3);

    // Verify connections
    let filter_successors = graph.get_successors(&PipelineId::new("filter"));
    assert_eq!(filter_successors.len(), 1);
    assert_eq!(filter_successors[0], PipelineId::new("fanout"));

    let fanout_successors = graph.get_successors(&PipelineId::new("fanout"));
    assert_eq!(fanout_successors.len(), 1);
    assert_eq!(fanout_successors[0], PipelineId::new("window"));

    // Verify topological ordering
    let topo = graph.topological_sort().unwrap();
    let filter_pos = topo
        .iter()
        .position(|id| id == &PipelineId::new("filter"))
        .unwrap();
    let fanout_pos = topo
        .iter()
        .position(|id| id == &PipelineId::new("fanout"))
        .unwrap();
    let window_pos = topo
        .iter()
        .position(|id| id == &PipelineId::new("window"))
        .unwrap();

    assert!(filter_pos < fanout_pos);
    assert!(fanout_pos < window_pos);
}

#[test]
fn test_dag_with_multiple_paths() {
    // Create a DAG with multiple paths:
    //   source -> fanout1 -> sink
    //         \-> fanout2 -/
    let graph = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(FilterPipeline::new(
            PipelineId::new("source"),
            |_| true,
        )))
        .add_pipeline(Box::new(MultibufferPipeline::new(
            PipelineId::new("fanout1"),
            2,
        )))
        .add_pipeline(Box::new(MultibufferPipeline::new(
            PipelineId::new("fanout2"),
            3,
        )))
        .add_pipeline(Box::new(FilterPipeline::new(
            PipelineId::new("sink"),
            |_| true,
        )))
        .connect(PipelineId::new("source"), PipelineId::new("fanout1"))
        .connect(PipelineId::new("source"), PipelineId::new("fanout2"))
        .connect(PipelineId::new("fanout1"), PipelineId::new("sink"))
        .connect(PipelineId::new("fanout2"), PipelineId::new("sink"))
        .build()
        .expect("Failed to build graph");

    assert_eq!(graph.len(), 4);

    // Verify source has 2 successors
    let source_successors = graph.get_successors(&PipelineId::new("source"));
    assert_eq!(source_successors.len(), 2);

    // Verify sink has no successors
    let sink_successors = graph.get_successors(&PipelineId::new("sink"));
    assert_eq!(sink_successors.len(), 0);
}

#[test]
fn test_cycle_detection_fails() {
    let result = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p1"), |_| {
            true
        })))
        .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p2"), |_| {
            true
        })))
        .connect(PipelineId::new("p1"), PipelineId::new("p2"))
        .connect(PipelineId::new("p2"), PipelineId::new("p1"))
        .build();

    assert!(matches!(result, Err(GraphError::CycleDetected(_))));
}

#[test]
fn test_nonexistent_pipeline_connection_fails() {
    let result = PipelineGraphBuilder::new()
        .add_pipeline(Box::new(FilterPipeline::new(
            PipelineId::new("existing"),
            |_| true,
        )))
        .connect(PipelineId::new("existing"), PipelineId::new("nonexistent"))
        .build();

    assert!(matches!(result, Err(GraphError::PipelineNotFound(_))));
}
