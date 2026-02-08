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
