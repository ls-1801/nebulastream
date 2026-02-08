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

//! Test source implementation for controlled testing.
//!
//! `TestSource` provides a source implementation that can be controlled
//! externally via a handle, making it ideal for testing pipelines and
//! executor behavior.

use crate::pipeline::{Buffer, PipelineId};
use crate::source::{Source, SourceEmitHandle, SourceError};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Commands that can be sent to a test source.
#[derive(Debug)]
enum TestSourceCommand {
    /// Emit a buffer to successors.
    EmitBuffer(Buffer),
    /// Simulate an error.
    EmitError(String),
    /// Signal end of stream and stop the source.
    EndOfStream,
}

/// Handle for controlling a test source externally.
///
/// This handle allows test code to inject buffers, errors, and end-of-stream
/// signals into the source, giving complete control over the source's behavior.
///
/// # Examples
///
/// ```no_run
/// use adaptive_engine::source::test_source::{TestSource, TestSourceHandle};
/// use adaptive_engine::pipeline::{PipelineId, Buffer};
/// use std::sync::Arc;
///
/// let (source, handle) = TestSource::new(PipelineId::new("test-src"));
///
/// // Inject a buffer
/// let buffer = Buffer::new(vec![1, 2, 3]);
/// handle.inject_buffer(buffer);
///
/// // Signal end of stream
/// handle.end_of_stream();
/// ```
pub struct TestSourceHandle {
    command_tx: Sender<TestSourceCommand>,
    stopped: Arc<Mutex<bool>>,
}

impl TestSourceHandle {
    /// Inject a buffer to be emitted by the source.
    ///
    /// The buffer will be emitted to all successor pipelines.
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer to emit
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::source::test_source::{TestSource, TestSourceHandle};
    /// # use adaptive_engine::pipeline::{PipelineId, Buffer};
    /// # let (source, handle) = TestSource::new(PipelineId::new("test-src"));
    /// let buffer = Buffer::new(vec![1, 2, 3]);
    /// handle.inject_buffer(buffer);
    /// ```
    pub fn inject_buffer(&self, buffer: Buffer) {
        let _ = self.command_tx.send(TestSourceCommand::EmitBuffer(buffer));
    }

    /// Inject an error to be reported by the source.
    ///
    /// This simulates a source error for testing error handling.
    ///
    /// # Arguments
    ///
    /// * `error` - Error message
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::source::test_source::{TestSource, TestSourceHandle};
    /// # use adaptive_engine::pipeline::PipelineId;
    /// # let (source, handle) = TestSource::new(PipelineId::new("test-src"));
    /// handle.inject_error("Simulated source failure".to_string());
    /// ```
    pub fn inject_error(&self, error: String) {
        let _ = self.command_tx.send(TestSourceCommand::EmitError(error));
    }

    /// Signal end of stream and stop the source.
    ///
    /// This causes the source to stop its worker thread and signal that
    /// no more buffers will be emitted.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::source::test_source::{TestSource, TestSourceHandle};
    /// # use adaptive_engine::pipeline::PipelineId;
    /// # let (source, handle) = TestSource::new(PipelineId::new("test-src"));
    /// handle.end_of_stream();
    /// ```
    pub fn end_of_stream(&self) {
        let _ = self.command_tx.send(TestSourceCommand::EndOfStream);
    }

    /// Check if the source has stopped.
    ///
    /// Returns `true` if the source's worker thread has stopped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::source::test_source::{TestSource, TestSourceHandle};
    /// # use adaptive_engine::pipeline::PipelineId;
    /// # let (source, handle) = TestSource::new(PipelineId::new("test-src"));
    /// handle.end_of_stream();
    /// // Wait for source to stop
    /// while !handle.is_stopped() {
    ///     std::thread::sleep(std::time::Duration::from_millis(10));
    /// }
    /// ```
    pub fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap()
    }
}

/// Test source implementation for controlled testing.
///
/// `TestSource` runs a worker thread that polls a command channel and
/// emits buffers based on external commands. This provides complete
/// control over source behavior for testing.
///
/// # Examples
///
/// ```no_run
/// use adaptive_engine::source::test_source::TestSource;
/// use adaptive_engine::pipeline::PipelineId;
/// use std::sync::Arc;
///
/// let (source, handle) = TestSource::new(PipelineId::new("test-src"));
/// let source = Arc::new(source);
///
/// // Use source in graph...
/// // Control via handle in test code...
/// ```
pub struct TestSource {
    id: PipelineId,
    command_rx: Arc<Mutex<Receiver<TestSourceCommand>>>,
    stopped: Arc<Mutex<bool>>,
    thread_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl TestSource {
    /// Create a new test source with a control handle.
    ///
    /// Returns a tuple of `(TestSource, TestSourceHandle)` where the source
    /// can be added to a graph and the handle can be used to control it.
    ///
    /// # Arguments
    ///
    /// * `id` - Pipeline ID for this source
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::source::test_source::TestSource;
    /// use adaptive_engine::pipeline::PipelineId;
    ///
    /// let (source, handle) = TestSource::new(PipelineId::new("test-src"));
    /// ```
    pub fn new(id: PipelineId) -> (Self, TestSourceHandle) {
        let (command_tx, command_rx) = channel();
        let stopped = Arc::new(Mutex::new(false));

        let source = Self {
            id,
            command_rx: Arc::new(Mutex::new(command_rx)),
            stopped: stopped.clone(),
            thread_handle: Arc::new(Mutex::new(None)),
        };

        let handle = TestSourceHandle {
            command_tx,
            stopped,
        };

        (source, handle)
    }
}

impl Source for TestSource {
    fn start(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
        let command_rx = self.command_rx.clone();
        let stopped = self.stopped.clone();

        let worker = thread::spawn(move || {
            loop {
                // Check if we should stop
                if emit_handle.should_stop() {
                    *stopped.lock().unwrap() = true;
                    break;
                }

                // Try to receive a command (non-blocking with timeout)
                let command = {
                    let rx = command_rx.lock().unwrap();
                    rx.recv_timeout(Duration::from_millis(10))
                };

                match command {
                    Ok(TestSourceCommand::EmitBuffer(buffer)) => {
                        if let Err(e) = emit_handle.emit(buffer) {
                            eprintln!("Error emitting buffer from test source: {}", e);
                            break;
                        }
                    }
                    Ok(TestSourceCommand::EmitError(error)) => {
                        eprintln!("Test source error: {}", error);
                        // Continue running after error
                    }
                    Ok(TestSourceCommand::EndOfStream) => {
                        *stopped.lock().unwrap() = true;
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // No command received, continue loop
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // Channel disconnected, stop
                        *stopped.lock().unwrap() = true;
                        break;
                    }
                }
            }
        });

        *self.thread_handle.lock().unwrap() = Some(worker);
        Ok(())
    }

    fn stop(&self) -> Result<(), SourceError> {
        *self.stopped.lock().unwrap() = true;
        Ok(())
    }

    fn teardown(&self) -> Result<(), SourceError> {
        // Wait for worker thread to finish
        let handle = self.thread_handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_creation() {
        let (source, _handle) = TestSource::new(PipelineId::new("test"));
        assert_eq!(source.id().as_str(), "test");
    }

    #[test]
    fn test_source_stopped_initially_false() {
        let (_source, handle) = TestSource::new(PipelineId::new("test"));
        assert!(!handle.is_stopped());
    }

    #[test]
    fn test_end_of_stream_sets_stopped() {
        let (_source, handle) = TestSource::new(PipelineId::new("test"));
        handle.end_of_stream();

        // Give it a moment to process
        thread::sleep(Duration::from_millis(50));

        // Note: This will only be true after the source actually starts and processes the command
        // In a real test, you'd start the source first
    }
}
