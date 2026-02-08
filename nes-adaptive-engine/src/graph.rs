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

//! Pipeline graph data structures and validation.
//!
//! Implements DAG-based pipeline graphs with cycle detection
//! and topological sorting for execution ordering.
//!
//! # Examples
//!
//! ```
//! use adaptive_engine::graph::PipelineGraph;
//! use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
//! use std::collections::HashSet;
//!
//! // Define a simple test pipeline
//! struct TestPipeline {
//!     id: PipelineId,
//! }
//!
//! impl Pipeline for TestPipeline {
//!     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
//!         Ok(vec![input])
//!     }
//!
//!     fn id(&self) -> &PipelineId {
//!         &self.id
//!     }
//! }
//!
//! let mut graph = PipelineGraph::new();
//!
//! // Add pipelines
//! let p1 = Box::new(TestPipeline { id: PipelineId::new("p1") });
//! let p2 = Box::new(TestPipeline { id: PipelineId::new("p2") });
//!
//! graph.add_pipeline(p1).unwrap();
//! graph.add_pipeline(p2).unwrap();
//!
//! // Connect them
//! graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
//!
//! // Validate the graph
//! graph.validate().unwrap();
//! ```

use crate::executor::metadata::PipelineMetadata;
use crate::pipeline::{Pipeline, PipelineId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use thiserror::Error;

/// Errors that can occur during graph operations.
#[derive(Error, Debug)]
pub enum GraphError {
    /// Pipeline ID already exists in the graph.
    #[error("Pipeline ID already exists: {0}")]
    DuplicateId(PipelineId),

    /// Pipeline ID not found in the graph.
    #[error("Pipeline not found: {0}")]
    PipelineNotFound(PipelineId),

    /// Graph contains a cycle, violating DAG property.
    #[error("Graph contains a cycle involving pipeline: {0}")]
    CycleDetected(PipelineId),

    /// Invalid graph structure.
    #[error("Invalid graph structure: {0}")]
    InvalidStructure(String),
}

/// A pipeline bundled with its runtime metadata in a single allocation.
///
/// `PipelineNode` wraps a pipeline implementation and its associated
/// `PipelineMetadata` so that hot-path operations can access both through
/// a single `Arc` pointer -- eliminating the need for a global metadata
/// `HashMap` lookup.
pub struct PipelineNode {
    pipeline: Box<dyn Pipeline>,
    metadata: PipelineMetadata,
}

impl PipelineNode {
    /// Create a new pipeline node with default metadata.
    pub fn new(pipeline: Box<dyn Pipeline>) -> Self {
        Self {
            pipeline,
            metadata: PipelineMetadata::new(),
        }
    }

    /// Get a reference to the underlying pipeline.
    pub fn pipeline(&self) -> &dyn Pipeline {
        &*self.pipeline
    }

    /// Get a reference to this node's metadata.
    pub fn metadata(&self) -> &PipelineMetadata {
        &self.metadata
    }

    /// Get this node's pipeline ID.
    pub fn id(&self) -> &PipelineId {
        self.pipeline.id()
    }
}

/// Directed acyclic graph (DAG) of pipelines.
///
/// `PipelineGraph` maintains a collection of pipelines and their
/// connections, ensuring DAG properties through validation.
///
/// Sources are special nodes with no predecessors that generate data.
/// They are tracked separately to enable source-specific operations.
pub struct PipelineGraph {
    nodes: HashMap<PipelineId, Arc<PipelineNode>>,
    edges: HashMap<PipelineId, Vec<PipelineId>>,
    reverse_edges: HashMap<PipelineId, Vec<PipelineId>>,
    /// Set of pipeline IDs that are sources (nodes that generate data).
    sources: HashSet<PipelineId>,
}

impl PipelineGraph {
    /// Create a new empty pipeline graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    ///
    /// let graph = PipelineGraph::new();
    /// ```
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            sources: HashSet::new(),
        }
    }

    /// Add a pipeline to the graph.
    ///
    /// The pipeline's ID must be unique within the graph.
    ///
    /// # Arguments
    ///
    /// * `pipeline` - The pipeline to add
    ///
    /// # Errors
    ///
    /// Returns `GraphError::DuplicateId` if a pipeline with the same ID
    /// already exists in the graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// struct TestPipeline { id: PipelineId }
    /// impl Pipeline for TestPipeline {
    ///     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    ///         Ok(vec![input])
    ///     }
    ///     fn id(&self) -> &PipelineId { &self.id }
    /// }
    ///
    /// let mut graph = PipelineGraph::new();
    /// let pipeline = Box::new(TestPipeline { id: PipelineId::new("test") });
    /// graph.add_pipeline(pipeline).unwrap();
    /// ```
    pub fn add_pipeline(&mut self, pipeline: Box<dyn Pipeline>) -> Result<(), GraphError> {
        let id = pipeline.id().clone();

        if self.nodes.contains_key(&id) {
            return Err(GraphError::DuplicateId(id));
        }

        self.nodes
            .insert(id.clone(), Arc::new(PipelineNode::new(pipeline)));
        self.edges.insert(id.clone(), Vec::new());
        self.reverse_edges.insert(id, Vec::new());

        Ok(())
    }

    /// Connect two pipelines with a directed edge.
    ///
    /// Creates an edge from the source pipeline to the sink pipeline,
    /// indicating that buffers flow from source to sink.
    ///
    /// # Arguments
    ///
    /// * `source` - The ID of the source pipeline
    /// * `sink` - The ID of the sink pipeline
    ///
    /// # Errors
    ///
    /// Returns `GraphError::PipelineNotFound` if either pipeline ID
    /// does not exist in the graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    /// graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
    /// ```
    pub fn connect(&mut self, source: &PipelineId, sink: &PipelineId) -> Result<(), GraphError> {
        if !self.nodes.contains_key(source) {
            return Err(GraphError::PipelineNotFound(source.clone()));
        }
        if !self.nodes.contains_key(sink) {
            return Err(GraphError::PipelineNotFound(sink.clone()));
        }

        self.edges.get_mut(source).unwrap().push(sink.clone());
        self.reverse_edges
            .get_mut(sink)
            .unwrap()
            .push(source.clone());

        Ok(())
    }

    /// Validate that the graph is a valid DAG (no cycles).
    ///
    /// Uses depth-first search with cycle detection to ensure
    /// the graph maintains DAG properties.
    ///
    /// # Errors
    ///
    /// Returns `GraphError::CycleDetected` if a cycle is found.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    /// graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
    /// assert!(graph.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), GraphError> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        for node_id in self.nodes.keys() {
            if !visited.contains(node_id) {
                self.detect_cycle_dfs(node_id, &mut visited, &mut rec_stack)?;
            }
        }

        // Validate sources have no predecessors
        self.validate_sources()?;

        Ok(())
    }

    /// Depth-first search for cycle detection.
    fn detect_cycle_dfs(
        &self,
        node: &PipelineId,
        visited: &mut HashSet<PipelineId>,
        rec_stack: &mut HashSet<PipelineId>,
    ) -> Result<(), GraphError> {
        visited.insert(node.clone());
        rec_stack.insert(node.clone());

        if let Some(neighbors) = self.edges.get(node) {
            for neighbor in neighbors {
                if !visited.contains(neighbor) {
                    self.detect_cycle_dfs(neighbor, visited, rec_stack)?;
                } else if rec_stack.contains(neighbor) {
                    return Err(GraphError::CycleDetected(neighbor.clone()));
                }
            }
        }

        rec_stack.remove(node);
        Ok(())
    }

    /// Get a topological ordering of the pipeline graph.
    ///
    /// Returns pipeline IDs in an order such that for every edge (u, v),
    /// u appears before v in the ordering. This is useful for determining
    /// execution order.
    ///
    /// # Errors
    ///
    /// Returns `GraphError::CycleDetected` if the graph contains a cycle.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    /// graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
    ///
    /// let order = graph.topological_sort().unwrap();
    /// assert_eq!(order.len(), 2);
    /// ```
    pub fn topological_sort(&self) -> Result<Vec<PipelineId>, GraphError> {
        // First validate the graph
        self.validate()?;

        // Calculate in-degrees
        let mut in_degree: HashMap<PipelineId, usize> = HashMap::new();
        for node_id in self.nodes.keys() {
            in_degree.insert(node_id.clone(), 0);
        }

        for neighbors in self.edges.values() {
            for neighbor in neighbors {
                *in_degree.get_mut(neighbor).unwrap() += 1;
            }
        }

        // Kahn's algorithm for topological sorting
        let mut queue: VecDeque<PipelineId> = in_degree
            .iter()
            .filter(|(_, &degree)| degree == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut result = Vec::new();

        while let Some(node) = queue.pop_front() {
            result.push(node.clone());

            if let Some(neighbors) = self.edges.get(&node) {
                for neighbor in neighbors {
                    let degree = in_degree.get_mut(neighbor).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            return Err(GraphError::CycleDetected(PipelineId::new("unknown")));
        }

        Ok(result)
    }

    /// Get a reference to a pipeline by ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The pipeline ID to look up
    ///
    /// # Returns
    ///
    /// An optional reference to the pipeline.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// let id = PipelineId::new("test");
    /// graph.add_pipeline(Box::new(TestPipeline { id: id.clone() })).unwrap();
    ///
    /// assert!(graph.get_pipeline(&id).is_some());
    /// ```
    pub fn get_pipeline(&self, id: &PipelineId) -> Option<&dyn Pipeline> {
        self.nodes.get(id).map(|node| node.pipeline())
    }

    /// Get a reference to the Arc-wrapped pipeline node by ID.
    ///
    /// This is used by the executor to obtain `Arc<PipelineNode>` references
    /// for creating `Weak` pointers in `WorkTask`s, avoiding global metadata lookups.
    pub fn get_node(&self, id: &PipelineId) -> Option<&Arc<PipelineNode>> {
        self.nodes.get(id)
    }

    /// Remove a pipeline node from the graph, returning it if it existed.
    ///
    /// Once removed, all `Weak<PipelineNode>` references to this node will
    /// fail to upgrade once all remaining `Arc` clones are dropped.
    pub fn remove_node(&mut self, id: &PipelineId) -> Option<Arc<PipelineNode>> {
        self.nodes.remove(id)
    }

    /// Get the successors (outgoing edges) for a pipeline.
    ///
    /// # Arguments
    ///
    /// * `id` - The pipeline ID to get successors for
    ///
    /// # Returns
    ///
    /// A slice of pipeline IDs that are successors, or an empty slice
    /// if the pipeline has no successors or doesn't exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    /// graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
    ///
    /// let successors = graph.get_successors(&PipelineId::new("p1"));
    /// assert_eq!(successors.len(), 1);
    /// ```
    pub fn get_successors(&self, id: &PipelineId) -> &[PipelineId] {
        self.edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get the number of pipelines in the graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// assert_eq!(graph.len(), 0);
    ///
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// assert_eq!(graph.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the graph is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    ///
    /// let graph = PipelineGraph::new();
    /// assert!(graph.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Get predecessors (incoming edges) for a pipeline.
    ///
    /// Returns the list of pipeline IDs that have edges pointing to the
    /// specified pipeline.
    ///
    /// # Arguments
    ///
    /// * `id` - The pipeline ID to get predecessors for
    ///
    /// # Returns
    ///
    /// A vector of pipeline IDs that are predecessors, or an empty vector
    /// if the pipeline has no predecessors or doesn't exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    /// graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
    ///
    /// let predecessors = graph.get_predecessors(&PipelineId::new("p2"));
    /// assert_eq!(predecessors.len(), 1);
    /// ```
    pub fn get_predecessors(&self, id: &PipelineId) -> Vec<PipelineId> {
        self.reverse_edges.get(id).cloned().unwrap_or_default()
    }

    /// Count the number of predecessors (incoming edges) for a pipeline.
    ///
    /// This is useful for determining the number of expected sources
    /// for end-of-stream coordination.
    ///
    /// # Arguments
    ///
    /// * `id` - The pipeline ID to count predecessors for
    ///
    /// # Returns
    ///
    /// The number of predecessors, or 0 if the pipeline has no predecessors
    /// or doesn't exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    /// graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")).unwrap();
    ///
    /// assert_eq!(graph.count_predecessors(&PipelineId::new("p1")), 0);
    /// assert_eq!(graph.count_predecessors(&PipelineId::new("p2")), 1);
    /// ```
    pub fn count_predecessors(&self, id: &PipelineId) -> usize {
        self.reverse_edges.get(id).map(|v| v.len()).unwrap_or(0)
    }

    /// Identify source pipelines (nodes with no predecessors).
    ///
    /// Source pipelines are entry points in the graph where data
    /// originates.
    ///
    /// # Returns
    ///
    /// A vector of pipeline IDs that have no incoming edges.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("source") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("sink") })).unwrap();
    /// graph.connect(&PipelineId::new("source"), &PipelineId::new("sink")).unwrap();
    ///
    /// let sources = graph.get_sources();
    /// assert_eq!(sources.len(), 1);
    /// assert_eq!(sources[0], PipelineId::new("source"));
    /// ```
    pub fn get_sources(&self) -> Vec<PipelineId> {
        self.nodes
            .keys()
            .filter(|id| {
                self.reverse_edges
                    .get(id)
                    .map(|v| v.is_empty())
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Identify sink pipelines (nodes with no successors).
    ///
    /// Sink pipelines are terminal nodes in the graph where data
    /// processing ends.
    ///
    /// # Returns
    ///
    /// A vector of pipeline IDs that have no outgoing edges.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("source") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("sink") })).unwrap();
    /// graph.connect(&PipelineId::new("source"), &PipelineId::new("sink")).unwrap();
    ///
    /// let sinks = graph.get_sinks();
    /// assert_eq!(sinks.len(), 1);
    /// assert_eq!(sinks[0], PipelineId::new("sink"));
    /// ```
    pub fn get_sinks(&self) -> Vec<PipelineId> {
        self.nodes
            .keys()
            .filter(|id| self.edges.get(id).map(|v| v.is_empty()).unwrap_or(true))
            .cloned()
            .collect()
    }

    /// Get all pipeline IDs in the graph.
    ///
    /// # Returns
    ///
    /// A vector of all pipeline IDs in the graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    ///
    /// # struct TestPipeline { id: PipelineId }
    /// # impl Pipeline for TestPipeline {
    /// #     fn execute(&self, input: Buffer, _context: &dyn adaptive_engine::executor::PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    /// #         Ok(vec![input])
    /// #     }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p1") })).unwrap();
    /// graph.add_pipeline(Box::new(TestPipeline { id: PipelineId::new("p2") })).unwrap();
    ///
    /// let ids = graph.get_all_pipeline_ids();
    /// assert_eq!(ids.len(), 2);
    /// ```
    pub fn get_all_pipeline_ids(&self) -> Vec<PipelineId> {
        self.nodes.keys().cloned().collect()
    }

    /// Add a source to the graph.
    ///
    /// Sources are special pipeline nodes that generate data and have no
    /// predecessors. They are wrapped in a `SourcePipeline` and added to
    /// the graph.
    ///
    /// # Arguments
    ///
    /// * `source` - The source implementation to add
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the source was added successfully.
    ///
    /// # Errors
    ///
    /// Returns `GraphError::DuplicateId` if a pipeline with the same ID
    /// already exists in the graph.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::source::{Source, SourceEmitHandle, SourceError};
    /// use adaptive_engine::pipeline::PipelineId;
    /// use std::sync::Arc;
    ///
    /// struct MySource {
    ///     id: PipelineId,
    /// }
    ///
    /// impl Source for MySource {
    ///     fn start(&self, _: SourceEmitHandle) -> Result<(), SourceError> {
    ///         Ok(())
    ///     }
    ///
    ///     fn stop(&self) -> Result<(), SourceError> {
    ///         Ok(())
    ///     }
    ///
    ///     fn id(&self) -> &PipelineId {
    ///         &self.id
    ///     }
    /// }
    ///
    /// let mut graph = PipelineGraph::new();
    /// let source = Arc::new(MySource { id: PipelineId::new("src") });
    /// graph.add_source(source).unwrap();
    /// ```
    pub fn add_source(
        &mut self,
        source: std::sync::Arc<dyn crate::source::Source>,
    ) -> Result<(), GraphError> {
        use crate::source::wrapper::SourcePipeline;

        let id = source.id().clone();

        if self.nodes.contains_key(&id) {
            return Err(GraphError::DuplicateId(id));
        }

        // Mark this node as a source
        self.sources.insert(id.clone());

        // Wrap the source in a SourcePipeline and add it as a pipeline node
        let wrapper = Box::new(SourcePipeline::new(source));
        self.nodes
            .insert(id.clone(), Arc::new(PipelineNode::new(wrapper)));
        self.edges.insert(id.clone(), Vec::new());
        self.reverse_edges.insert(id, Vec::new());

        Ok(())
    }

    /// Check if a pipeline ID is a source node.
    ///
    /// # Arguments
    ///
    /// * `id` - The pipeline ID to check
    ///
    /// # Returns
    ///
    /// Returns `true` if the pipeline is a source node, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::source::{Source, SourceEmitHandle, SourceError};
    /// use adaptive_engine::pipeline::PipelineId;
    /// use std::sync::Arc;
    ///
    /// # struct MySource { id: PipelineId }
    /// # impl Source for MySource {
    /// #     fn start(&self, _: SourceEmitHandle) -> Result<(), SourceError> { Ok(()) }
    /// #     fn stop(&self) -> Result<(), SourceError> { Ok(()) }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// let source = Arc::new(MySource { id: PipelineId::new("src") });
    /// graph.add_source(source).unwrap();
    ///
    /// assert!(graph.is_source(&PipelineId::new("src")));
    /// ```
    pub fn is_source(&self, id: &PipelineId) -> bool {
        self.sources.contains(id)
    }

    /// Get a source pipeline by ID, if it exists.
    ///
    /// This method attempts to downcast the pipeline to a `SourcePipeline`.
    /// It will only succeed if the pipeline is actually a source node.
    ///
    /// # Arguments
    ///
    /// * `id` - The pipeline ID to look up
    ///
    /// # Returns
    ///
    /// Returns `Some(&SourcePipeline)` if the pipeline exists and is a source,
    /// `None` otherwise.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::graph::PipelineGraph;
    /// use adaptive_engine::source::{Source, SourceEmitHandle, SourceError};
    /// use adaptive_engine::pipeline::PipelineId;
    /// use std::sync::Arc;
    ///
    /// # struct MySource { id: PipelineId }
    /// # impl Source for MySource {
    /// #     fn start(&self, _: SourceEmitHandle) -> Result<(), SourceError> { Ok(()) }
    /// #     fn stop(&self) -> Result<(), SourceError> { Ok(()) }
    /// #     fn id(&self) -> &PipelineId { &self.id }
    /// # }
    /// let mut graph = PipelineGraph::new();
    /// let source = Arc::new(MySource { id: PipelineId::new("src") });
    /// graph.add_source(source).unwrap();
    ///
    /// let source_pipeline = graph.get_source_pipeline(&PipelineId::new("src"));
    /// assert!(source_pipeline.is_some());
    /// ```
    pub fn get_source_pipeline(
        &self,
        id: &PipelineId,
    ) -> Option<&crate::source::wrapper::SourcePipeline> {
        self.nodes.get(id).and_then(|node| {
            // Try to downcast to SourcePipeline
            let pipeline_ref: &dyn Pipeline = node.pipeline();
            let any_ref = pipeline_ref as &dyn std::any::Any;
            any_ref.downcast_ref::<crate::source::wrapper::SourcePipeline>()
        })
    }

    /// Validate that all source nodes have no predecessors.
    ///
    /// Sources must have no incoming edges in the graph. This method
    /// checks that invariant.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if all sources are valid (no predecessors).
    ///
    /// # Errors
    ///
    /// Returns `GraphError::InvalidStructure` if any source has predecessors.
    fn validate_sources(&self) -> Result<(), GraphError> {
        for source_id in &self.sources {
            let predecessor_count = self.count_predecessors(source_id);
            if predecessor_count > 0 {
                return Err(GraphError::InvalidStructure(format!(
                    "Source '{}' has {} predecessor(s), but sources must have no predecessors",
                    source_id, predecessor_count
                )));
            }
        }
        Ok(())
    }
}

impl Default for PipelineGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Buffer, PipelineError};

    struct TestPipeline {
        id: PipelineId,
    }

    impl Pipeline for TestPipeline {
        fn execute(
            &self,
            input: Buffer,
            _context: &dyn crate::executor::PipelineExecutionContext,
        ) -> Result<Vec<Buffer>, PipelineError> {
            Ok(vec![input])
        }

        fn id(&self) -> &PipelineId {
            &self.id
        }
    }

    #[test]
    fn test_add_pipeline() {
        let mut graph = PipelineGraph::new();
        let pipeline = Box::new(TestPipeline {
            id: PipelineId::new("test"),
        });

        assert!(graph.add_pipeline(pipeline).is_ok());
        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn test_duplicate_pipeline_id() {
        let mut graph = PipelineGraph::new();
        let p1 = Box::new(TestPipeline {
            id: PipelineId::new("test"),
        });
        let p2 = Box::new(TestPipeline {
            id: PipelineId::new("test"),
        });

        graph.add_pipeline(p1).unwrap();
        assert!(matches!(
            graph.add_pipeline(p2),
            Err(GraphError::DuplicateId(_))
        ));
    }

    #[test]
    fn test_connect_pipelines() {
        let mut graph = PipelineGraph::new();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p1"),
            }))
            .unwrap();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p2"),
            }))
            .unwrap();

        assert!(graph
            .connect(&PipelineId::new("p1"), &PipelineId::new("p2"))
            .is_ok());

        let successors = graph.get_successors(&PipelineId::new("p1"));
        assert_eq!(successors.len(), 1);
        assert_eq!(successors[0], PipelineId::new("p2"));
    }

    #[test]
    fn test_connect_nonexistent_pipeline() {
        let mut graph = PipelineGraph::new();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p1"),
            }))
            .unwrap();

        assert!(matches!(
            graph.connect(&PipelineId::new("p1"), &PipelineId::new("p2")),
            Err(GraphError::PipelineNotFound(_))
        ));
    }

    #[test]
    fn test_validate_acyclic_graph() {
        let mut graph = PipelineGraph::new();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p1"),
            }))
            .unwrap();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p2"),
            }))
            .unwrap();
        graph
            .connect(&PipelineId::new("p1"), &PipelineId::new("p2"))
            .unwrap();

        assert!(graph.validate().is_ok());
    }

    #[test]
    fn test_detect_cycle() {
        let mut graph = PipelineGraph::new();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p1"),
            }))
            .unwrap();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p2"),
            }))
            .unwrap();

        graph
            .connect(&PipelineId::new("p1"), &PipelineId::new("p2"))
            .unwrap();
        graph
            .connect(&PipelineId::new("p2"), &PipelineId::new("p1"))
            .unwrap();

        assert!(matches!(
            graph.validate(),
            Err(GraphError::CycleDetected(_))
        ));
    }

    #[test]
    fn test_topological_sort() {
        let mut graph = PipelineGraph::new();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p1"),
            }))
            .unwrap();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p2"),
            }))
            .unwrap();
        graph
            .add_pipeline(Box::new(TestPipeline {
                id: PipelineId::new("p3"),
            }))
            .unwrap();

        graph
            .connect(&PipelineId::new("p1"), &PipelineId::new("p2"))
            .unwrap();
        graph
            .connect(&PipelineId::new("p2"), &PipelineId::new("p3"))
            .unwrap();

        let sorted = graph.topological_sort().unwrap();
        assert_eq!(sorted.len(), 3);

        // p1 should come before p2, and p2 before p3
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

    #[test]
    fn test_get_pipeline() {
        let mut graph = PipelineGraph::new();
        let id = PipelineId::new("test");
        graph
            .add_pipeline(Box::new(TestPipeline { id: id.clone() }))
            .unwrap();

        assert!(graph.get_pipeline(&id).is_some());
        assert!(graph
            .get_pipeline(&PipelineId::new("nonexistent"))
            .is_none());
    }
}
