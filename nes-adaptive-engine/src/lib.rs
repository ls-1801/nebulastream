//! # Adaptive Engine - Pipeline Builder API
//!
//! A type-safe API for constructing stream processing pipeline graphs.
//! This library enables testing without requiring the full query compiler.
//!
//! ## Features
//!
//! - Type-safe pipeline graph construction
//! - Hierarchical sequence number tracking
//! - Mock pipeline implementations for testing
//! - Ergonomic builder API with fluent chaining
//!
//! ## Quick Start
//!
//! ```ignore
//! use adaptive_engine::builder::PipelineGraphBuilder;
//! use adaptive_engine::pipeline::{Pipeline, PipelineId};
//!
//! // Create pipeline instances and add them to the builder
//! // (Actual pipeline implementations shown in examples)
//! ```

pub mod builder;
pub mod engine;
pub mod executor;
pub mod ffi;
pub mod graph;
pub mod pipeline;
pub mod sequence;
pub mod source;

// Re-export executor types
pub use executor::{ExecutionStats, Executor, ExecutorError, ExecutorHandle, QueryId};

// Re-export engine types
pub use engine::Engine;

// Re-export source types
pub use source::{
    GeneratorConfig, GeneratorSource, Source, SourceEmitHandle, SourceError, SourcePipeline,
    TestSource, TestSourceHandle,
};
