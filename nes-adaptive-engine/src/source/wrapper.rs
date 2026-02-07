//! Pipeline wrapper for source nodes.
//!
//! This module provides `SourcePipeline`, a wrapper that allows `Source`
//! implementations to be used as `Pipeline` nodes in the graph. The wrapper
//! delegates lifecycle methods to the underlying source and enforces the
//! invariant that sources don't process input buffers.

use crate::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use crate::source::{Source, SourceEmitHandle, SourceError};
use std::sync::Arc;

/// A pipeline wrapper around a source node.
///
/// This wrapper allows sources to be added to pipeline graphs, which expect
/// all nodes to implement the `Pipeline` trait. It delegates lifecycle methods
/// (`setup`, `teardown`, `id`) to the underlying source and enforces the
/// invariant that `execute()` is never called on source nodes.
///
/// # Invariant
///
/// Sources **never** receive input buffers. If `execute()` is called on a
/// `SourcePipeline`, it indicates a serious bug in the executor and will
/// cause an immediate panic.
///
/// # Examples
///
/// ```no_run
/// use adaptive_engine::source::{Source, SourcePipeline, SourceEmitHandle, SourceError};
/// use adaptive_engine::pipeline::{Pipeline, PipelineId};
/// use std::sync::Arc;
///
/// struct MySource {
///     id: PipelineId,
/// }
///
/// impl Source for MySource {
///     fn start(&self, _emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
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
/// let source = Arc::new(MySource { id: PipelineId::new("my-source") });
/// let wrapped = SourcePipeline::new(source);
/// ```
pub struct SourcePipeline {
    source: Arc<dyn Source>,
}

impl SourcePipeline {
    /// Create a new pipeline wrapper for a source.
    ///
    /// # Arguments
    ///
    /// * `source` - The source implementation to wrap
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::source::{Source, SourcePipeline, SourceEmitHandle, SourceError};
    /// use adaptive_engine::pipeline::PipelineId;
    /// use std::sync::Arc;
    ///
    /// struct SimpleSource {
    ///     id: PipelineId,
    /// }
    ///
    /// impl Source for SimpleSource {
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
    /// let source = Arc::new(SimpleSource { id: PipelineId::new("src") });
    /// let wrapper = SourcePipeline::new(source);
    /// ```
    pub fn new(source: Arc<dyn Source>) -> Self {
        Self { source }
    }

    /// Get a reference to the underlying source.
    ///
    /// This is used internally by the executor to call source-specific
    /// methods like `start()` and `stop()`.
    ///
    /// # Returns
    ///
    /// Returns a reference to the wrapped source.
    pub fn source(&self) -> &Arc<dyn Source> {
        &self.source
    }

    /// Start the underlying source.
    ///
    /// This is called by the executor when processing a `StartSource` task.
    ///
    /// # Arguments
    ///
    /// * `emit_handle` - Handle for the source to emit buffers
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the source started successfully.
    ///
    /// # Errors
    ///
    /// Returns an error if the source fails to start.
    pub fn start_source(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
        self.source.start(emit_handle)
    }

    /// Stop the underlying source.
    ///
    /// This is called by the executor during cascading shutdown.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the source stopped successfully.
    ///
    /// # Errors
    ///
    /// Returns an error if the source fails to stop.
    pub fn stop_source(&self) -> Result<(), SourceError> {
        self.source.stop()
    }
}

impl Pipeline for SourcePipeline {
    fn setup(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        self.source
            .setup()
            .map_err(|e| PipelineError::ExecutionFailed(format!("Source setup failed: {}", e)))
    }

    fn execute(
        &self,
        _input: Buffer,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        // INVARIANT VIOLATION: Sources should never receive input buffers.
        // If this is called, there is a serious bug in the executor's routing logic.
        panic!(
            "INVARIANT VIOLATION: execute() called on source '{}'. \
             Sources generate data and should never receive input buffers. \
             This indicates a bug in the executor.",
            self.source.id()
        );
    }

    fn flush(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        // Sources don't accumulate state to flush
        Ok(vec![])
    }

    fn teardown(
        &self,
        _context: &dyn crate::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        self.source
            .teardown()
            .map_err(|e| PipelineError::ExecutionFailed(format!("Source teardown failed: {}", e)))
    }

    fn id(&self) -> &PipelineId {
        self.source.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::SequenceNumber;

    struct MockSource {
        id: PipelineId,
    }

    impl Source for MockSource {
        fn start(&self, _emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
            Ok(())
        }

        fn stop(&self) -> Result<(), SourceError> {
            Ok(())
        }

        fn id(&self) -> &PipelineId {
            &self.id
        }
    }

    #[test]
    fn test_source_pipeline_delegates_id() {
        let source = Arc::new(MockSource {
            id: PipelineId::new("test-source"),
        });
        let wrapper = SourcePipeline::new(source);

        assert_eq!(wrapper.id().as_str(), "test-source");
    }

    #[test]
    fn test_source_pipeline_setup_success() {
        use crate::executor::ExecutorContext;
        use std::sync::mpsc::channel;

        let source = Arc::new(MockSource {
            id: PipelineId::new("test-source"),
        });
        let wrapper = SourcePipeline::new(source);

        let (tx, _rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);
        assert!(wrapper.setup(&context).is_ok());
    }

    #[test]
    fn test_source_pipeline_teardown_success() {
        use crate::executor::ExecutorContext;
        use std::sync::mpsc::channel;

        let source = Arc::new(MockSource {
            id: PipelineId::new("test-source"),
        });
        let wrapper = SourcePipeline::new(source);

        let (tx, _rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);
        assert!(wrapper.teardown(&context).is_ok());
    }

    #[test]
    fn test_source_pipeline_flush_returns_empty() {
        use crate::executor::ExecutorContext;
        use std::sync::mpsc::channel;

        let source = Arc::new(MockSource {
            id: PipelineId::new("test-source"),
        });
        let wrapper = SourcePipeline::new(source);

        let (tx, _rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);
        let result = wrapper.flush(&context).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    #[should_panic(expected = "INVARIANT VIOLATION: execute() called on source")]
    fn test_source_pipeline_execute_panics() {
        use crate::executor::ExecutorContext;
        use std::sync::mpsc::channel;

        let source = Arc::new(MockSource {
            id: PipelineId::new("test-source"),
        });
        let wrapper = SourcePipeline::new(source);

        let (tx, _rx) = channel();
        let context = ExecutorContext::new(PipelineId::new("test"), 0, 1, tx);
        let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(1));
        let _ = wrapper.execute(buffer, &context); // Should panic
    }

    #[test]
    fn test_source_wrapper_access() {
        let source = Arc::new(MockSource {
            id: PipelineId::new("test-source"),
        });
        let wrapper = SourcePipeline::new(source.clone());

        assert_eq!(wrapper.source().id().as_str(), "test-source");
    }
}
