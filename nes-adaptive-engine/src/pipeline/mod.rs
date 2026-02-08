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

//! Core pipeline traits and types.
//!
//! Defines the fundamental `Pipeline` trait that all pipeline
//! implementations must satisfy, along with buffer types and
//! pipeline identifiers.
//!
//! # Examples
//!
//! ```no_run
//! use adaptive_engine::executor::PipelineExecutionContext;
//! use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
//!
//! struct MyPipeline {
//!     id: PipelineId,
//! }
//!
//! impl Pipeline for MyPipeline {
//!     fn execute(
//!         &self,
//!         input: Buffer,
//!         _context: &dyn PipelineExecutionContext,
//!     ) -> Result<Vec<Buffer>, PipelineError> {
//!         // Process the input buffer
//!         Ok(vec![input])
//!     }
//!
//!     fn id(&self) -> &PipelineId {
//!         &self.id
//!     }
//! }
//! ```

pub mod compat;
pub mod mocks;

use std::fmt;
use thiserror::Error;

/// Unique identifier for a pipeline in the graph.
///
/// Pipeline IDs are used to reference and connect pipelines
/// within a pipeline graph. They must be unique within a graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipelineId(String);

impl PipelineId {
    /// Create a new pipeline ID from a string.
    ///
    /// # Arguments
    ///
    /// * `id` - The string identifier for the pipeline
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let id = PipelineId::new("my-pipeline");
    /// ```
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the string representation of this pipeline ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let id = PipelineId::new("my-pipeline");
    /// assert_eq!(id.as_str(), "my-pipeline");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PipelineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for PipelineId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for PipelineId {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Buffer containing data flowing through pipelines.
///
/// # Dual Mode Support
///
/// Buffers can be either:
/// - **Opaque**: C++ buffer via opaque handle. No data copy, no metadata on Rust side.
/// - **Owned**: Rust-owned Vec<u8> for unit tests / mock pipelines.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::Buffer;
///
/// // Create Rust-owned buffer
/// let buffer = Buffer::new(vec![1, 2, 3]);
/// assert_eq!(buffer.data(), &[1, 2, 3]);
/// ```
pub struct Buffer {
    inner: BufferInner,
}

/// Internal representation: either an opaque C++ handle or owned Rust data.
enum BufferInner {
    /// C++ buffer via opaque handle. No data copy, no metadata on Rust side.
    Opaque(OpaqueBufferHandle),
    /// Rust-owned data for unit tests / mock pipelines.
    Owned(Vec<u8>),
}

impl Clone for Buffer {
    fn clone(&self) -> Self {
        Self {
            inner: match &self.inner {
                BufferInner::Opaque(handle) => BufferInner::Opaque(handle.clone()),
                BufferInner::Owned(data) => BufferInner::Owned(data.clone()),
            },
        }
    }
}

/// An opaque reference to a C++ buffer handle.
///
/// Each handle is independently owned. Clone goes through FFI to
/// `buffer_handle_clone`, Drop goes through `buffer_handle_release`.
/// No Arc — C++ refcounting drives everything via virtual dispatch
/// on BufferHandleBase.
pub struct OpaqueBufferHandle {
    /// The opaque handle value (e.g., NesBufferWrapper* which inherits BufferHandleBase)
    handle: usize,
}

impl OpaqueBufferHandle {
    /// Create a new opaque buffer handle.
    pub fn new(handle: usize) -> Self {
        Self { handle }
    }

    /// Get the opaque handle value.
    pub fn handle(&self) -> usize {
        self.handle
    }
}

#[cfg(feature = "cpp-ffi")]
impl Clone for OpaqueBufferHandle {
    fn clone(&self) -> Self {
        let new_handle = unsafe { crate::ffi::callbacks::buffer_handle_clone(self.handle) };
        Self { handle: new_handle }
    }
}

#[cfg(not(feature = "cpp-ffi"))]
impl Clone for OpaqueBufferHandle {
    fn clone(&self) -> Self {
        panic!("OpaqueBufferHandle::clone() requires cpp-ffi feature");
    }
}

#[cfg(feature = "cpp-ffi")]
impl Drop for OpaqueBufferHandle {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe {
                crate::ffi::callbacks::buffer_handle_release(self.handle);
            }
        }
    }
}

#[cfg(not(feature = "cpp-ffi"))]
impl Drop for OpaqueBufferHandle {
    fn drop(&mut self) {
        if self.handle != 0 {
            eprintln!(
                "Warning: OpaqueBufferHandle::drop() called without cpp-ffi feature (handle={})",
                self.handle
            );
        }
    }
}

// SAFETY: OpaqueBufferHandle contains only a usize value (opaque pointer).
// The C++ objects they point to use virtual dispatch for clone/release
// which is thread-safe by contract.
unsafe impl Send for OpaqueBufferHandle {}
unsafe impl Sync for OpaqueBufferHandle {}

impl Buffer {
    /// Create a new buffer with owned data.
    ///
    /// # Arguments
    ///
    /// * `data` - The data payload
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3]);
    /// assert_eq!(buffer.data(), &[1, 2, 3]);
    /// ```
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            inner: BufferInner::Owned(data),
        }
    }

    /// Create a buffer wrapping an opaque C++ handle.
    ///
    /// The buffer treats the C++ handle as opaque — no data access,
    /// no metadata. Clone and Drop go through FFI.
    pub fn opaque(handle: OpaqueBufferHandle) -> Self {
        Self {
            inner: BufferInner::Opaque(handle),
        }
    }

    /// Get a reference to the buffer's data.
    ///
    /// Returns the owned data for Owned buffers, or an empty slice for Opaque buffers.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3]);
    /// assert_eq!(buffer.data(), &[1, 2, 3]);
    /// ```
    pub fn data(&self) -> &[u8] {
        match &self.inner {
            BufferInner::Owned(data) => data,
            BufferInner::Opaque(_) => &[],
        }
    }

    /// Get a mutable reference to the buffer's data.
    ///
    /// Only available for Owned buffers. Panics for Opaque buffers.
    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        match &mut self.inner {
            BufferInner::Owned(data) => data,
            BufferInner::Opaque(_) => panic!("Cannot get mutable data from opaque buffer"),
        }
    }

    /// Get the opaque C++ buffer handle, if this is an Opaque buffer.
    pub fn opaque_handle(&self) -> Option<&OpaqueBufferHandle> {
        match &self.inner {
            BufferInner::Opaque(handle) => Some(handle),
            BufferInner::Owned(_) => None,
        }
    }

    /// Check if this buffer is opaque (C++ handle).
    pub fn is_opaque(&self) -> bool {
        matches!(&self.inner, BufferInner::Opaque(_))
    }
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            BufferInner::Owned(data) => f
                .debug_struct("Buffer")
                .field("type", &"owned")
                .field("size", &data.len())
                .finish(),
            BufferInner::Opaque(handle) => f
                .debug_struct("Buffer")
                .field("type", &"opaque")
                .field("handle", &handle.handle)
                .finish(),
        }
    }
}

/// Errors that can occur during pipeline execution.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// Pipeline execution failed with a custom message.
    #[error("Pipeline execution failed: {0}")]
    ExecutionFailed(String),

    /// Pipeline not found in the graph.
    #[error("Pipeline not found: {0}")]
    NotFound(PipelineId),

    /// Invalid buffer state.
    #[error("Invalid buffer state: {0}")]
    InvalidBuffer(String),
}

/// Core pipeline trait - all pipeline types implement this.
///
/// The `Pipeline` trait defines the fundamental interface that all
/// pipeline implementations must satisfy. Pipelines are `Send + Sync`
/// to support work-stealing across threads.
///
/// # Lifecycle Contract
///
/// The executor enforces strict lifecycle invariants:
///
/// 1. `setup()` is called exactly once before any buffers are processed
/// 2. `execute()` is called zero or more times (only if setup succeeded)
/// 3. `teardown()` is called exactly once if setup succeeded (even on errors)
///
/// Violating these invariants (e.g., executing before setup) causes a panic.
///
/// # Examples
///
/// Simple style (return outputs):
/// ```
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
/// use adaptive_engine::executor::PipelineExecutionContext;
///
/// struct EchoPipeline {
///     id: PipelineId,
/// }
///
/// impl Pipeline for EchoPipeline {
///     fn execute(
///         &self,
///         input: Buffer,
///         _context: &dyn PipelineExecutionContext
///     ) -> Result<Vec<Buffer>, PipelineError> {
///         // Simply pass through the input buffer
///         Ok(vec![input])
///     }
///
///     fn id(&self) -> &PipelineId {
///         &self.id
///     }
/// }
/// ```
pub trait Pipeline: Send + Sync + std::any::Any {
    /// Called once when pipeline is started, before first buffer.
    fn setup(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        Ok(()) // Default: no-op
    }

    /// Process an input buffer, potentially emitting 0-N output buffers.
    fn execute(
        &self,
        input: Buffer,
        context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError>;

    /// Called during StopPipelineTask to emit final buffers before teardown.
    fn flush(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        Ok(vec![]) // Default: no-op
    }

    /// Called once when pipeline is stopped, after all buffers processed.
    fn teardown(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        Ok(()) // Default: no-op
    }

    /// Get the unique identifier for this pipeline.
    fn id(&self) -> &PipelineId;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_id_creation() {
        let id1 = PipelineId::new("test");
        let id2 = PipelineId::from("test");
        let id3 = PipelineId::from("test".to_string());

        assert_eq!(id1, id2);
        assert_eq!(id1, id3);
        assert_eq!(id1.as_str(), "test");
        assert_eq!(id1.to_string(), "test");
    }

    #[test]
    fn test_buffer_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let buffer = Buffer::new(data.clone());

        assert_eq!(buffer.data(), &data);
    }

    #[test]
    fn test_buffer_clone() {
        let buffer = Buffer::new(vec![1, 2, 3]);
        let cloned = buffer.clone();
        assert_eq!(cloned.data(), &[1, 2, 3]);
    }

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
    fn test_pipeline_trait() {
        use std::sync::mpsc::channel;

        // Create a mock context for testing
        let (tx, _rx) = channel();
        let context = crate::executor::ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);

        let pipeline = TestPipeline {
            id: PipelineId::new("test"),
        };

        let input = Buffer::new(vec![1, 2, 3]);
        let result = pipeline.execute(input, &context).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].data(), &[1, 2, 3]);
    }
}
