//! Backward compatibility adapters for legacy pipelines.
//!
//! This module provides adapters and utilities for maintaining backward
//! compatibility with pipelines that were written before the context parameter
//! was added to the Pipeline trait.
//!
//! # Note
//!
//! As of the current implementation, all Pipeline trait methods accept a
//! context parameter. Pipelines that don't need the context can simply
//! ignore it using the `_context` parameter naming convention:
//!
//! ```no_run
//! use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
//! use adaptive_engine::executor::PipelineExecutionContext;
//!
//! struct SimplePipeline {
//!     id: PipelineId,
//! }
//!
//! impl Pipeline for SimplePipeline {
//!     // Just ignore the context parameter
//!     fn execute(
//!         &self,
//!         input: Buffer,
//!         _context: &dyn PipelineExecutionContext,
//!     ) -> Result<Vec<Buffer>, PipelineError> {
//!         Ok(vec![input])
//!     }
//!
//!     fn id(&self) -> &PipelineId {
//!         &self.id
//!     }
//! }
//! ```
//!
//! # Migration Guide
//!
//! If you have existing pipeline implementations from before this change:
//!
//! **Before (old API)**:
//! ```ignore
//! fn execute(&self, input: Buffer) -> Result<Vec<Buffer>, PipelineError>
//! ```
//!
//! **After (new API)**:
//! ```ignore
//! fn execute(
//!     &self,
//!     input: Buffer,
//!     _context: &dyn PipelineExecutionContext,
//! ) -> Result<Vec<Buffer>, PipelineError>
//! ```
//!
//! Simply add the context parameter to all lifecycle methods:
//! - `setup(_context: &dyn PipelineExecutionContext)`
//! - `execute(&self, input: Buffer, _context: &dyn PipelineExecutionContext)`
//! - `flush(_context: &dyn PipelineExecutionContext)`
//! - `teardown(_context: &dyn PipelineExecutionContext)`
//!
//! If your pipeline doesn't need the context, prefix the parameter with `_`
//! to indicate it's intentionally unused.
//!
//! # Using the Context
//!
//! For NebulaStream-compatible pipelines, use the context to emit buffers:
//!
//! ```no_run
//! use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
//! use adaptive_engine::executor::PipelineExecutionContext;
//!
//! struct EmittingPipeline {
//!     id: PipelineId,
//! }
//!
//! impl Pipeline for EmittingPipeline {
//!     fn execute(
//!         &self,
//!         input: Buffer,
//!         context: &dyn PipelineExecutionContext,
//!     ) -> Result<Vec<Buffer>, PipelineError> {
//!         // Process the input
//!         let output = Buffer::new(input.data().to_vec());
//!
//!         // Emit via context (NebulaStream style)
//!         context.emit_buffer(output);
//!
//!         // Return empty when using context emission
//!         Ok(vec![])
//!     }
//!
//!     fn id(&self) -> &PipelineId {
//!         &self.id
//!     }
//! }
//! ```
