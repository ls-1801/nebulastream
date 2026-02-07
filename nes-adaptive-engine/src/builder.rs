//! Ergonomic builder API for constructing pipeline graphs.
//!
//! Provides a fluent, type-safe interface for creating and
//! connecting pipelines in test code.
//!
//! # Examples
//!
//! ```
//! use adaptive_engine::builder::PipelineGraphBuilder;
//! use adaptive_engine::pipeline::mocks::{FilterPipeline, MultibufferPipeline};
//! use adaptive_engine::pipeline::PipelineId;
//!
//! let graph = PipelineGraphBuilder::new()
//!     .add_pipeline(Box::new(FilterPipeline::new(
//!         PipelineId::new("filter"),
//!         |b| b.data().len() > 5
//!     )))
//!     .add_pipeline(Box::new(MultibufferPipeline::new(
//!         PipelineId::new("fanout"),
//!         3
//!     )))
//!     .connect(PipelineId::new("filter"), PipelineId::new("fanout"))
//!     .build()
//!     .expect("Failed to build pipeline graph");
//!
//! assert_eq!(graph.len(), 2);
//! ```

use crate::graph::{GraphError, PipelineGraph};
use crate::pipeline::{Pipeline, PipelineId};

/// Builder for constructing pipeline graphs with a fluent API.
///
/// `PipelineGraphBuilder` provides a convenient, chainable interface for
/// creating pipeline graphs. It automatically validates the graph structure
/// when `build()` is called.
///
/// # Design Philosophy
///
/// The builder is intentionally generic - it only accepts `Pipeline` trait
/// objects. This makes it fully extensible: anyone can create custom pipeline
/// types without modifying the builder.
///
/// # Examples
///
/// ```
/// use adaptive_engine::builder::PipelineGraphBuilder;
/// use adaptive_engine::pipeline::mocks::{FilterPipeline, MultibufferPipeline};
/// use adaptive_engine::pipeline::PipelineId;
///
/// let graph = PipelineGraphBuilder::new()
///     .add_pipeline(Box::new(FilterPipeline::new(
///         PipelineId::new("source"),
///         |b| b.data().len() > 10
///     )))
///     .add_pipeline(Box::new(MultibufferPipeline::new(
///         PipelineId::new("fanout"),
///         3
///     )))
///     .connect(PipelineId::new("source"), PipelineId::new("fanout"))
///     .build()
///     .expect("Failed to build graph");
/// ```
pub struct PipelineGraphBuilder {
    graph: PipelineGraph,
    connections: Vec<(PipelineId, PipelineId)>,
}

impl PipelineGraphBuilder {
    /// Create a new empty pipeline graph builder.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::builder::PipelineGraphBuilder;
    ///
    /// let builder = PipelineGraphBuilder::new();
    /// ```
    pub fn new() -> Self {
        Self {
            graph: PipelineGraph::new(),
            connections: Vec::new(),
        }
    }

    /// Add a pipeline to the graph.
    ///
    /// The pipeline must implement the `Pipeline` trait. This method is
    /// intentionally generic - it accepts any pipeline type, making the
    /// builder fully extensible without requiring modifications.
    ///
    /// # Arguments
    ///
    /// * `pipeline` - A boxed pipeline implementing the `Pipeline` trait
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::builder::PipelineGraphBuilder;
    /// use adaptive_engine::pipeline::mocks::FilterPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let builder = PipelineGraphBuilder::new()
    ///     .add_pipeline(Box::new(FilterPipeline::new(
    ///         PipelineId::new("filter"),
    ///         |b| b.data().len() > 5
    ///     )));
    /// ```
    pub fn add_pipeline(mut self, pipeline: Box<dyn Pipeline>) -> Self {
        // Store the result but don't fail yet - we'll validate in build()
        let _ = self.graph.add_pipeline(pipeline);
        self
    }

    /// Connect two pipelines with a directed edge.
    ///
    /// Creates an edge from the source pipeline to the sink pipeline,
    /// indicating that buffers flow from source to sink. The connection
    /// is stored and will be validated when `build()` is called.
    ///
    /// # Arguments
    ///
    /// * `source` - The ID of the source pipeline
    /// * `sink` - The ID of the sink pipeline
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::builder::PipelineGraphBuilder;
    /// use adaptive_engine::pipeline::mocks::{FilterPipeline, MultibufferPipeline};
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let builder = PipelineGraphBuilder::new()
    ///     .add_pipeline(Box::new(FilterPipeline::new(
    ///         PipelineId::new("p1"),
    ///         |b| true
    ///     )))
    ///     .add_pipeline(Box::new(MultibufferPipeline::new(
    ///         PipelineId::new("p2"),
    ///         2
    ///     )))
    ///     .connect(PipelineId::new("p1"), PipelineId::new("p2"));
    /// ```
    pub fn connect(mut self, source: PipelineId, sink: PipelineId) -> Self {
        self.connections.push((source, sink));
        self
    }

    /// Build and validate the pipeline graph.
    ///
    /// This method consumes the builder, applies all connections, validates
    /// the graph structure (checking for cycles and valid pipeline IDs),
    /// and returns the final `PipelineGraph`.
    ///
    /// # Returns
    ///
    /// A validated `PipelineGraph` on success.
    ///
    /// # Errors
    ///
    /// Returns `GraphError` if:
    /// - Any pipeline IDs in connections don't exist
    /// - The graph contains cycles (violates DAG property)
    /// - Any other validation failure
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::builder::PipelineGraphBuilder;
    /// use adaptive_engine::pipeline::mocks::FilterPipeline;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let graph = PipelineGraphBuilder::new()
    ///     .add_pipeline(Box::new(FilterPipeline::new(
    ///         PipelineId::new("filter"),
    ///         |b| true
    ///     )))
    ///     .build()
    ///     .expect("Failed to build graph");
    ///
    /// assert_eq!(graph.len(), 1);
    /// ```
    pub fn build(mut self) -> Result<PipelineGraph, GraphError> {
        // Apply all connections
        for (source, sink) in self.connections {
            self.graph.connect(&source, &sink)?;
        }

        // Validate the graph structure
        self.graph.validate()?;

        Ok(self.graph)
    }
}

impl Default for PipelineGraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::mocks::{FilterPipeline, MultibufferPipeline, OccasionalEmissionPipeline};

    #[test]
    fn test_builder_empty_graph() {
        let graph = PipelineGraphBuilder::new().build().unwrap();
        assert_eq!(graph.len(), 0);
    }

    #[test]
    fn test_builder_single_pipeline() {
        let graph = PipelineGraphBuilder::new()
            .add_pipeline(Box::new(FilterPipeline::new(
                PipelineId::new("filter"),
                |_| true,
            )))
            .build()
            .unwrap();

        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn test_builder_multiple_pipelines() {
        let graph = PipelineGraphBuilder::new()
            .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p1"), |_| {
                true
            })))
            .add_pipeline(Box::new(MultibufferPipeline::new(PipelineId::new("p2"), 3)))
            .add_pipeline(Box::new(OccasionalEmissionPipeline::new(
                PipelineId::new("p3"),
                5,
            )))
            .build()
            .unwrap();

        assert_eq!(graph.len(), 3);
    }

    #[test]
    fn test_builder_with_connections() {
        let graph = PipelineGraphBuilder::new()
            .add_pipeline(Box::new(FilterPipeline::new(
                PipelineId::new("filter"),
                |b| b.data().len() > 5,
            )))
            .add_pipeline(Box::new(MultibufferPipeline::new(
                PipelineId::new("fanout"),
                3,
            )))
            .connect(PipelineId::new("filter"), PipelineId::new("fanout"))
            .build()
            .unwrap();

        assert_eq!(graph.len(), 2);

        let successors = graph.get_successors(&PipelineId::new("filter"));
        assert_eq!(successors.len(), 1);
        assert_eq!(successors[0], PipelineId::new("fanout"));
    }

    #[test]
    fn test_builder_complex_graph() {
        // Create a more complex graph:
        // p1 -> p2 -> p4
        //   \-> p3 -/
        let graph = PipelineGraphBuilder::new()
            .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p1"), |_| {
                true
            })))
            .add_pipeline(Box::new(MultibufferPipeline::new(PipelineId::new("p2"), 2)))
            .add_pipeline(Box::new(MultibufferPipeline::new(PipelineId::new("p3"), 2)))
            .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p4"), |_| {
                true
            })))
            .connect(PipelineId::new("p1"), PipelineId::new("p2"))
            .connect(PipelineId::new("p1"), PipelineId::new("p3"))
            .connect(PipelineId::new("p2"), PipelineId::new("p4"))
            .connect(PipelineId::new("p3"), PipelineId::new("p4"))
            .build()
            .unwrap();

        assert_eq!(graph.len(), 4);

        // Verify connections
        let p1_successors = graph.get_successors(&PipelineId::new("p1"));
        assert_eq!(p1_successors.len(), 2);

        let p4_successors = graph.get_successors(&PipelineId::new("p4"));
        assert_eq!(p4_successors.len(), 0);
    }

    #[test]
    fn test_builder_invalid_connection() {
        let result = PipelineGraphBuilder::new()
            .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p1"), |_| {
                true
            })))
            .connect(PipelineId::new("p1"), PipelineId::new("nonexistent"))
            .build();

        assert!(matches!(result, Err(GraphError::PipelineNotFound(_))));
    }

    #[test]
    fn test_builder_cycle_detection() {
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
    fn test_builder_topological_sort() {
        let graph = PipelineGraphBuilder::new()
            .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p1"), |_| {
                true
            })))
            .add_pipeline(Box::new(MultibufferPipeline::new(PipelineId::new("p2"), 2)))
            .add_pipeline(Box::new(FilterPipeline::new(PipelineId::new("p3"), |_| {
                true
            })))
            .connect(PipelineId::new("p1"), PipelineId::new("p2"))
            .connect(PipelineId::new("p2"), PipelineId::new("p3"))
            .build()
            .unwrap();

        let sorted = graph.topological_sort().unwrap();
        assert_eq!(sorted.len(), 3);

        // Verify ordering
        let p1_pos = sorted
            .iter()
            .position(|id| id == &PipelineId::new("p1"))
            .unwrap();
        let p2_pos = sorted
            .iter()
            .position(|id| id == &PipelineId::new("p2"))
            .unwrap();
        let p3_pos = sorted
            .iter()
            .position(|id| id == &PipelineId::new("p3"))
            .unwrap();

        assert!(p1_pos < p2_pos);
        assert!(p2_pos < p3_pos);
    }
}
