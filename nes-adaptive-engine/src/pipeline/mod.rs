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
//! use adaptive_engine::sequence::SequenceNumber;
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

use crate::sequence::SequenceNumber;
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
/// Buffers carry both data payload and sequence number tracking
/// for lineage and exactly-once processing guarantees.
///
/// # Dual Mode Support
///
/// Buffers can be either:
/// - **Opaque**: Reference-counted C++ TupleBuffer (zero-copy, for NebulaStream integration)
/// - **Owned**: Rust-owned Vec<u8> (for Rust-native pipelines)
///
/// # NebulaStream Compatibility
///
/// This buffer includes metadata fields compatible with NebulaStream's TupleBuffer:
/// - `origin_id` - ID of the source that originated this buffer
/// - `watermark` - Event-time watermark for stream processing
/// - `number_of_tuples` - Number of tuples contained in the buffer
/// - `chunk_number` - For chunked buffers, the chunk index
/// - `is_last_chunk` - Whether this is the last chunk
///
/// These fields enable watermark tracking, provenance, and chunked data
/// handling while maintaining backward compatibility with simple buffers.
///
/// # Examples
///
/// ```
/// use adaptive_engine::pipeline::Buffer;
/// use adaptive_engine::sequence::SequenceNumber;
///
/// // Create Rust-owned buffer (backward compatible)
/// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
/// assert_eq!(buffer.data(), &[1, 2, 3]);
/// ```
#[derive(Clone)]
pub struct Buffer {
    inner: BufferInner,
    sequence: SequenceNumber,
}

/// An opaque reference to a C++ buffer handle that preserves the full buffer
/// structure (including child buffers for variable-sized data) across the FFI boundary.
///
/// When present, this handle is passed directly to C++ stages instead of copying
/// raw bytes, avoiding loss of child buffers.
///
/// Uses reference counting (Arc) so the handle can be shared across fan-out
/// routing without double-free. The C++ buffer is released when the last
/// reference is dropped.
#[derive(Clone)]
pub struct OpaqueBufferHandle {
    inner: std::sync::Arc<OpaqueBufferHandleInner>,
}

struct OpaqueBufferHandleInner {
    /// The C++ BufferProvider pointer (needed to release the handle)
    provider_ptr: usize,
    /// The opaque handle value (e.g., NesBufferWrapper*)
    handle: usize,
}

impl OpaqueBufferHandle {
    /// Create a new opaque buffer handle.
    pub fn new(provider_ptr: usize, handle: usize) -> Self {
        Self {
            inner: std::sync::Arc::new(OpaqueBufferHandleInner {
                provider_ptr,
                handle,
            }),
        }
    }

    /// Get the provider pointer.
    pub fn provider_ptr(&self) -> usize {
        self.inner.provider_ptr
    }

    /// Get the opaque handle value.
    pub fn handle(&self) -> usize {
        self.inner.handle
    }

    /// Check if this is the only reference to the handle.
    /// When true, the handle can be consumed (moved to a stage) without
    /// needing to keep the C++ buffer alive.
    pub fn is_unique(&self) -> bool {
        std::sync::Arc::strong_count(&self.inner) == 1
    }
}

impl Drop for OpaqueBufferHandleInner {
    fn drop(&mut self) {
        // Release the C++ buffer when the last reference is dropped.
        // This is safe because the C++ BufferProvider::release() deletes
        // the NesBufferWrapper which decrements the TupleBuffer ref count.
        if self.handle != 0 {
            unsafe {
                crate::ffi::callbacks::buffer_provider_release(self.provider_ptr, self.handle);
            }
        }
    }
}

// SAFETY: OpaqueBufferHandleInner contains only usize values (opaque pointers).
// The C++ objects they point to are managed by the BufferProvider which is
// thread-safe by contract.
unsafe impl Send for OpaqueBufferHandleInner {}
unsafe impl Sync for OpaqueBufferHandleInner {}

/// Internal representation of buffer data.
#[derive(Clone)]
struct BufferInner {
    data: Vec<u8>,
    origin_id: Option<u64>,
    watermark: Option<u64>,
    number_of_tuples: u64,
    chunk_number: Option<u64>,
    is_last_chunk: bool,
    /// Optional opaque handle to a C++ buffer. When set, this handle is passed
    /// directly to C++ stages to preserve child buffers for variable-sized data.
    opaque_handle: Option<OpaqueBufferHandle>,
}

impl Buffer {
    /// Create a new buffer with data and sequence number.
    ///
    /// This creates a simple buffer with default metadata values.
    /// Use builder methods to add metadata for NebulaStream compatibility.
    ///
    /// # Arguments
    ///
    /// * `data` - The data payload
    /// * `sequence` - The hierarchical sequence number
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
    /// assert_eq!(buffer.data(), &[1, 2, 3]);
    /// ```
    pub fn new(data: Vec<u8>, sequence: SequenceNumber) -> Self {
        Self {
            inner: BufferInner {
                data,
                origin_id: None,
                watermark: None,
                number_of_tuples: 0,
                chunk_number: None,
                is_last_chunk: false,
                opaque_handle: None,
            },
            sequence,
        }
    }

    /// Set the origin ID (source that originated this buffer).
    ///
    /// # Arguments
    ///
    /// * `origin_id` - The ID of the source node
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_origin(42);
    /// assert_eq!(buffer.origin_id(), Some(42));
    /// ```
    pub fn with_origin(mut self, origin_id: u64) -> Self {
        self.inner.origin_id = Some(origin_id);
        self
    }

    /// Set the watermark for event-time processing.
    ///
    /// # Arguments
    ///
    /// * `watermark` - The event-time watermark
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_watermark(1000);
    /// assert_eq!(buffer.watermark(), Some(1000));
    /// ```
    pub fn with_watermark(mut self, watermark: u64) -> Self {
        self.inner.watermark = Some(watermark);
        self
    }

    /// Set the number of tuples in this buffer.
    ///
    /// # Arguments
    ///
    /// * `count` - The number of tuples
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_tuple_count(10);
    /// assert_eq!(buffer.number_of_tuples(), 10);
    /// ```
    pub fn with_tuple_count(mut self, count: u64) -> Self {
        self.inner.number_of_tuples = count;
        self
    }

    /// Set chunking information for large buffers.
    ///
    /// # Arguments
    ///
    /// * `chunk_number` - The chunk index (0-based)
    /// * `is_last` - Whether this is the last chunk
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_chunk_info(0, false);
    /// assert_eq!(buffer.chunk_number(), Some(0));
    /// assert!(!buffer.is_last_chunk());
    /// ```
    pub fn with_chunk_info(mut self, chunk_num: u64, is_last: bool) -> Self {
        self.inner.chunk_number = Some(chunk_num);
        self.inner.is_last_chunk = is_last;
        self
    }

    /// Get the origin ID if set.
    ///
    /// # Returns
    ///
    /// The origin ID, or `None` if not set.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
    /// assert_eq!(buffer.origin_id(), None);
    ///
    /// let buffer = buffer.with_origin(42);
    /// assert_eq!(buffer.origin_id(), Some(42));
    /// ```
    pub fn origin_id(&self) -> Option<u64> {
        self.inner.origin_id
    }

    /// Get the watermark if set.
    ///
    /// # Returns
    ///
    /// The watermark, or `None` if not set.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_watermark(1000);
    /// assert_eq!(buffer.watermark(), Some(1000));
    /// ```
    pub fn watermark(&self) -> Option<u64> {
        self.inner.watermark
    }

    /// Get the number of tuples in this buffer.
    ///
    /// # Returns
    ///
    /// The number of tuples (0 if not set).
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_tuple_count(5);
    /// assert_eq!(buffer.number_of_tuples(), 5);
    /// ```
    pub fn number_of_tuples(&self) -> u64 {
        self.inner.number_of_tuples
    }

    /// Get the chunk number if this is a chunked buffer.
    ///
    /// # Returns
    ///
    /// The chunk number, or `None` if not chunked.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_chunk_info(2, false);
    /// assert_eq!(buffer.chunk_number(), Some(2));
    /// ```
    pub fn chunk_number(&self) -> Option<u64> {
        self.inner.chunk_number
    }

    /// Set the opaque C++ buffer handle.
    ///
    /// When set, the Rust executor will pass this handle directly to C++ stages
    /// instead of copying raw bytes, preserving child buffers for variable-sized data.
    pub fn with_opaque_handle(mut self, handle: OpaqueBufferHandle) -> Self {
        self.inner.opaque_handle = Some(handle);
        self
    }

    /// Get the opaque C++ buffer handle, if set.
    pub fn opaque_handle(&self) -> Option<&OpaqueBufferHandle> {
        self.inner.opaque_handle.as_ref()
    }

    /// Check if this is the last chunk.
    ///
    /// # Returns
    ///
    /// `true` if this is the last chunk, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_chunk_info(2, true);
    /// assert!(buffer.is_last_chunk());
    /// ```
    pub fn is_last_chunk(&self) -> bool {
        self.inner.is_last_chunk
    }

    /// Get a reference to the buffer's data.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
    /// assert_eq!(buffer.data(), &[1, 2, 3]);
    /// ```
    pub fn data(&self) -> &[u8] {
        &self.inner.data
    }

    /// Get a mutable reference to the buffer's data.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let mut buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
    /// buffer.data_mut()[0] = 42;
    /// assert_eq!(buffer.data()[0], 42);
    /// ```
    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.inner.data
    }

    /// Get a reference to the buffer's sequence number.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let seq = SequenceNumber::new(1);
    /// let buffer = Buffer::new(vec![1, 2, 3], seq.clone());
    /// assert_eq!(buffer.sequence(), &seq);
    /// ```
    pub fn sequence(&self) -> &SequenceNumber {
        &self.sequence
    }

    /// Create a child buffer with a new sequence number.
    ///
    /// This is used when a pipeline emits multiple buffers from
    /// a single input buffer. The child inherits all metadata from
    /// the parent (origin, watermark, etc.).
    ///
    /// # Arguments
    ///
    /// * `data` - The data for the child buffer
    /// * `offset` - The child offset for the sequence number
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let parent = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1))
    ///     .with_origin(42)
    ///     .with_watermark(1000);
    /// let child = parent.child_buffer(vec![4, 5, 6], 1);
    /// assert_eq!(child.sequence().to_string(), "1.1");
    /// assert_eq!(child.origin_id(), Some(42));
    /// assert_eq!(child.watermark(), Some(1000));
    /// ```
    pub fn child_buffer(&self, data: Vec<u8>, offset: u64) -> Self {
        Self {
            inner: BufferInner {
                data,
                // Inherit metadata from parent
                origin_id: self.origin_id(),
                watermark: self.watermark(),
                number_of_tuples: self.number_of_tuples(),
                chunk_number: self.chunk_number(),
                is_last_chunk: self.is_last_chunk(),
                // Child buffers don't inherit opaque handle
                opaque_handle: None,
            },
            sequence: self.sequence.child(offset),
        }
    }

    /// Consume this buffer and return its data and sequence number.
    ///
    /// # Returns
    ///
    /// A tuple of (data, sequence) from this buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::pipeline::Buffer;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
    /// let (data, seq) = buffer.into_parts();
    /// assert_eq!(data, vec![1, 2, 3]);
    /// assert_eq!(seq.to_string(), "1");
    /// ```
    pub fn into_parts(self) -> (Vec<u8>, SequenceNumber) {
        (self.inner.data, self.sequence)
    }
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Buffer")
            .field("sequence", &self.sequence)
            .field("size", &self.data().len())
            .field("origin_id", &self.origin_id())
            .field("watermark", &self.watermark())
            .field("number_of_tuples", &self.number_of_tuples())
            .field("chunk_number", &self.chunk_number())
            .field("is_last_chunk", &self.is_last_chunk())
            .finish()
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
/// # NebulaStream Compatibility
///
/// This trait now accepts a `PipelineExecutionContext` parameter in all lifecycle
/// methods, matching NebulaStream's adaptive_engine::PipelineStage interface. Pipelines
/// can use the context to:
/// - Emit buffers via `context.emit_buffer()` (NebulaStream style)
/// - Return buffers directly (simple style)
/// - Allocate buffers from the pool
/// - Access worker thread information
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
///
/// NebulaStream style (emit via context):
/// ```no_run
/// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
/// use adaptive_engine::executor::PipelineExecutionContext;
///
/// struct EmitPipeline {
///     id: PipelineId,
/// }
///
/// impl Pipeline for EmitPipeline {
///     fn execute(
///         &self,
///         input: Buffer,
///         context: &dyn PipelineExecutionContext
///     ) -> Result<Vec<Buffer>, PipelineError> {
///         // Process and emit via context
///         context.emit_buffer(input);
///         // Return empty when using emit
///         Ok(vec![])
///     }
///
///     fn id(&self) -> &PipelineId {
///         &self.id
///     }
/// }
/// ```
pub trait Pipeline: Send + Sync + std::any::Any {
    /// Called once when pipeline is started, before first buffer.
    ///
    /// Use this to:
    /// - Initialize connections
    /// - Allocate resources
    /// - Verify configuration
    /// - Set up state
    ///
    /// # Arguments
    ///
    /// * `context` - Execution context providing access to executor services
    ///
    /// # Errors
    ///
    /// If setup fails, the pipeline will not execute and the error is recorded.
    /// No buffers will be processed until setup succeeds.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    /// use adaptive_engine::executor::PipelineExecutionContext;
    ///
    /// struct DatabasePipeline {
    ///     id: PipelineId,
    ///     // connection: Option<DbConnection>,
    /// }
    ///
    /// impl Pipeline for DatabasePipeline {
    ///     fn setup(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
    ///         // Initialize database connection
    ///         // self.connection = Some(DbConnection::new()?);
    ///         Ok(())
    ///     }
    ///
    ///     fn execute(
    ///         &self,
    ///         input: Buffer,
    ///         _context: &dyn PipelineExecutionContext
    ///     ) -> Result<Vec<Buffer>, PipelineError> {
    ///         // Use connection to process data
    ///         Ok(vec![input])
    ///     }
    ///
    ///     fn id(&self) -> &PipelineId {
    ///         &self.id
    ///     }
    /// }
    /// ```
    fn setup(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        Ok(()) // Default: no-op
    }

    /// Process an input buffer, potentially emitting 0-N output buffers.
    ///
    /// The pipeline receives a single input buffer and may emit zero or more
    /// output buffers. This supports:
    /// - Filter pipelines (0 or 1 output)
    /// - Transform pipelines (1 output)
    /// - Fanout pipelines (N outputs)
    /// - Windowing pipelines (0 or 1 output, depending on window state)
    ///
    /// # Output Modes
    ///
    /// Pipelines can output buffers in two ways:
    /// 1. **Return style**: Return buffers directly (simple, backward compatible)
    /// 2. **Emit style**: Call `context.emit_buffer()` and return empty vec (NebulaStream compatible)
    ///
    /// # Arguments
    ///
    /// * `input` - The buffer to process
    /// * `context` - Execution context for emitting buffers and accessing services
    ///
    /// # Returns
    ///
    /// A vector of output buffers, which may be empty.
    ///
    /// # Errors
    ///
    /// Returns `PipelineError` if execution fails.
    fn execute(
        &self,
        input: Buffer,
        context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError>;

    /// Called during StopPipelineTask to emit final buffers before teardown.
    ///
    /// Use this to:
    /// - Flush accumulated state (e.g., partial windows, aggregations)
    /// - Emit final summary buffers
    /// - Output buffered data before shutdown
    ///
    /// Flushed buffers are routed to successors through the normal DAG,
    /// enabling cascading shutdown where data propagates downstream.
    ///
    /// # Arguments
    ///
    /// * `context` - Execution context for emitting buffers and accessing services
    ///
    /// # Default Implementation
    ///
    /// Returns empty vector (no flush). Override to provide custom flush logic.
    ///
    /// # Errors
    ///
    /// Returns `PipelineError` if flushing fails. Errors are logged but do not
    /// prevent teardown from being called.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    /// use adaptive_engine::executor::PipelineExecutionContext;
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// struct WindowPipeline {
    ///     id: PipelineId,
    ///     // accumulated: Vec<Buffer>,
    /// }
    ///
    /// impl Pipeline for WindowPipeline {
    ///     fn flush(&self, _context: &dyn PipelineExecutionContext) -> Result<Vec<Buffer>, PipelineError> {
    ///         // Flush partial window
    ///         // let data = self.accumulated.drain(..).collect();
    ///         // Ok(vec![Buffer::new(data, SequenceNumber::new(1))])
    ///         Ok(vec![])
    ///     }
    ///
    ///     fn execute(
    ///         &self,
    ///         input: Buffer,
    ///         _context: &dyn PipelineExecutionContext
    ///     ) -> Result<Vec<Buffer>, PipelineError> {
    ///         // Accumulate buffers
    ///         Ok(vec![])
    ///     }
    ///
    ///     fn id(&self) -> &PipelineId {
    ///         &self.id
    ///     }
    /// }
    /// ```
    fn flush(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        Ok(vec![]) // Default: no-op
    }

    /// Called once when pipeline is stopped, after all buffers processed.
    ///
    /// Use this to:
    /// - Close connections
    /// - Free resources
    /// - Write final state
    ///
    /// # Arguments
    ///
    /// * `context` - Execution context for accessing services during teardown
    ///
    /// # Guarantee
    ///
    /// Teardown is ALWAYS called if setup succeeded, even if:
    /// - Buffers failed during execution
    /// - Pipeline was stopped mid-stream
    /// - Errors occurred
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::pipeline::{Pipeline, Buffer, PipelineId, PipelineError};
    /// use adaptive_engine::executor::PipelineExecutionContext;
    ///
    /// struct FilePipeline {
    ///     id: PipelineId,
    ///     // file: Option<File>,
    /// }
    ///
    /// impl Pipeline for FilePipeline {
    ///     fn teardown(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
    ///         // Close file, flush buffers
    ///         // self.file.take().map(|f| f.flush());
    ///         Ok(())
    ///     }
    ///
    ///     fn execute(
    ///         &self,
    ///         input: Buffer,
    ///         _context: &dyn PipelineExecutionContext
    ///     ) -> Result<Vec<Buffer>, PipelineError> {
    ///         // Write to file
    ///         Ok(vec![])
    ///     }
    ///
    ///     fn id(&self) -> &PipelineId {
    ///         &self.id
    ///     }
    /// }
    /// ```
    fn teardown(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        Ok(()) // Default: no-op
    }

    /// Get the unique identifier for this pipeline.
    ///
    /// # Returns
    ///
    /// A reference to this pipeline's ID.
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
        let seq = SequenceNumber::new(1);
        let buffer = Buffer::new(data.clone(), seq.clone());

        assert_eq!(buffer.data(), &data);
        assert_eq!(buffer.sequence(), &seq);
    }

    #[test]
    fn test_buffer_child() {
        let parent = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
        let child = parent.child_buffer(vec![4, 5, 6], 1);

        assert_eq!(child.data(), &[4, 5, 6]);
        assert_eq!(child.sequence().to_string(), "1.1");
    }

    #[test]
    fn test_buffer_into_parts() {
        let data = vec![1, 2, 3];
        let seq = SequenceNumber::new(1);
        let buffer = Buffer::new(data.clone(), seq.clone());

        let (extracted_data, extracted_seq) = buffer.into_parts();
        assert_eq!(extracted_data, data);
        assert_eq!(extracted_seq, seq);
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

        let input = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
        let result = pipeline.execute(input, &context).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].data(), &[1, 2, 3]);
    }
}
