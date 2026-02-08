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

//! Generator source implementation for production use.
//!
//! `GeneratorSource` provides a production-ready source that emits buffers
//! at a specified frequency using a configurable generator function.

use crate::pipeline::{Buffer, PipelineId};
use crate::source::{Source, SourceEmitHandle, SourceError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Configuration for a generator source.
///
/// Defines how often the source emits buffers and how many to emit before
/// stopping (if limited).
///
/// # Examples
///
/// ```
/// use adaptive_engine::source::generator_source::GeneratorConfig;
/// use std::time::Duration;
///
/// // Emit every 100ms, unlimited buffers
/// let config = GeneratorConfig::new(Duration::from_millis(100));
///
/// // Emit 10 buffers then stop
/// let config_limited = GeneratorConfig::new(Duration::from_millis(100))
///     .with_max_buffers(10);
/// ```
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    /// Time interval between buffer emissions.
    pub emit_interval: Duration,
    /// Maximum number of buffers to emit (None = unlimited).
    pub max_buffers: Option<u64>,
}

impl GeneratorConfig {
    /// Create a new generator configuration.
    ///
    /// # Arguments
    ///
    /// * `emit_interval` - Time between buffer emissions
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::source::generator_source::GeneratorConfig;
    /// use std::time::Duration;
    ///
    /// let config = GeneratorConfig::new(Duration::from_millis(100));
    /// ```
    pub fn new(emit_interval: Duration) -> Self {
        Self {
            emit_interval,
            max_buffers: None,
        }
    }

    /// Set the maximum number of buffers to emit.
    ///
    /// After emitting this many buffers, the source will stop automatically.
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum number of buffers
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::source::generator_source::GeneratorConfig;
    /// use std::time::Duration;
    ///
    /// let config = GeneratorConfig::new(Duration::from_millis(100))
    ///     .with_max_buffers(10);
    /// ```
    pub fn with_max_buffers(mut self, max: u64) -> Self {
        self.max_buffers = Some(max);
        self
    }
}

/// Generator source for producing buffers at a specified frequency.
///
/// `GeneratorSource` runs a worker thread that calls a generator function
/// at regular intervals to produce buffers. The generator function receives
/// a sequence number and returns a buffer.
///
/// # Examples
///
/// ```no_run
/// use adaptive_engine::source::generator_source::{GeneratorSource, GeneratorConfig};
/// use adaptive_engine::pipeline::{PipelineId, Buffer};
/// use std::time::Duration;
/// use std::sync::Arc;
///
/// // Create a source that emits buffers every 100ms
/// let config = GeneratorConfig::new(Duration::from_millis(100))
///     .with_max_buffers(10);
///
/// let generator = |seq| {
///     let data = vec![seq as u8];
///     Buffer::new(data)
/// };
///
/// let source = GeneratorSource::new(
///     PipelineId::new("gen-src"),
///     config,
///     generator
/// );
///
/// let source = Arc::new(source);
/// // Add to graph and start...
/// ```
pub struct GeneratorSource<F>
where
    F: Fn(u64) -> Buffer + Send + Sync + 'static,
{
    id: PipelineId,
    config: GeneratorConfig,
    generator: Arc<F>,
    stopped: Arc<AtomicBool>,
    buffer_count: Arc<AtomicU64>,
    thread_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl<F> GeneratorSource<F>
where
    F: Fn(u64) -> Buffer + Send + Sync + 'static,
{
    /// Create a new generator source.
    ///
    /// # Arguments
    ///
    /// * `id` - Pipeline ID for this source
    /// * `config` - Configuration for emission frequency and limits
    /// * `generator` - Function that generates buffers from sequence numbers
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use adaptive_engine::source::generator_source::{GeneratorSource, GeneratorConfig};
    /// use adaptive_engine::pipeline::{PipelineId, Buffer};
    /// use std::time::Duration;
    ///
    /// let config = GeneratorConfig::new(Duration::from_millis(100));
    /// let source = GeneratorSource::new(
    ///     PipelineId::new("gen"),
    ///     config,
    ///     |seq| Buffer::new(vec![seq as u8])
    /// );
    /// ```
    pub fn new(id: PipelineId, config: GeneratorConfig, generator: F) -> Self {
        Self {
            id,
            config,
            generator: Arc::new(generator),
            stopped: Arc::new(AtomicBool::new(false)),
            buffer_count: Arc::new(AtomicU64::new(0)),
            thread_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the number of buffers emitted so far.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::source::generator_source::{GeneratorSource, GeneratorConfig};
    /// # use adaptive_engine::pipeline::{PipelineId, Buffer};
    /// # use std::time::Duration;
    /// # let config = GeneratorConfig::new(Duration::from_millis(100));
    /// # let source = GeneratorSource::new(
    /// #     PipelineId::new("gen"),
    /// #     config,
    /// #     |seq| Buffer::new(vec![seq as u8])
    /// # );
    /// let count = source.buffer_count();
    /// ```
    pub fn buffer_count(&self) -> u64 {
        self.buffer_count.load(Ordering::SeqCst)
    }

    /// Check if the source has stopped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use adaptive_engine::source::generator_source::{GeneratorSource, GeneratorConfig};
    /// # use adaptive_engine::pipeline::{PipelineId, Buffer};
    /// # use std::time::Duration;
    /// # let config = GeneratorConfig::new(Duration::from_millis(100));
    /// # let source = GeneratorSource::new(
    /// #     PipelineId::new("gen"),
    /// #     config,
    /// #     |seq| Buffer::new(vec![seq as u8])
    /// # );
    /// if source.is_stopped() {
    ///     println!("Source has stopped");
    /// }
    /// ```
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

impl<F> Source for GeneratorSource<F>
where
    F: Fn(u64) -> Buffer + Send + Sync + 'static,
{
    fn start(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
        let generator = self.generator.clone();
        let stopped = self.stopped.clone();
        let buffer_count = self.buffer_count.clone();
        let config = self.config.clone();

        let worker = thread::spawn(move || {
            let mut seq: u64 = 1;

            loop {
                // Check if we should stop
                if emit_handle.should_stop() || stopped.load(Ordering::SeqCst) {
                    break;
                }

                // Check if we've reached max buffers
                if let Some(max) = config.max_buffers {
                    if seq > max {
                        stopped.store(true, Ordering::SeqCst);
                        break;
                    }
                }

                // Generate and emit buffer
                let buffer = generator(seq);
                if let Err(e) = emit_handle.emit(buffer) {
                    eprintln!("Error emitting buffer from generator source: {}", e);
                    break;
                }

                buffer_count.fetch_add(1, Ordering::SeqCst);
                seq += 1;

                // Sleep for the configured interval
                thread::sleep(config.emit_interval);
            }

            stopped.store(true, Ordering::SeqCst);
        });

        *self.thread_handle.lock().unwrap() = Some(worker);
        Ok(())
    }

    fn stop(&self) -> Result<(), SourceError> {
        self.stopped.store(true, Ordering::SeqCst);
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
    fn test_generator_config_creation() {
        let config = GeneratorConfig::new(Duration::from_millis(100));
        assert_eq!(config.emit_interval, Duration::from_millis(100));
        assert_eq!(config.max_buffers, None);
    }

    #[test]
    fn test_generator_config_with_max() {
        let config = GeneratorConfig::new(Duration::from_millis(100)).with_max_buffers(10);
        assert_eq!(config.max_buffers, Some(10));
    }

    #[test]
    fn test_generator_source_creation() {
        let config = GeneratorConfig::new(Duration::from_millis(100));
        let source = GeneratorSource::new(PipelineId::new("test"), config, |seq| {
            Buffer::new(vec![seq as u8])
        });
        assert_eq!(source.id().as_str(), "test");
        assert_eq!(source.buffer_count(), 0);
        assert!(!source.is_stopped());
    }

    #[test]
    fn test_generator_source_stop() {
        let config = GeneratorConfig::new(Duration::from_millis(100));
        let source = GeneratorSource::new(PipelineId::new("test"), config, |seq| {
            Buffer::new(vec![seq as u8])
        });

        assert!(source.stop().is_ok());
        assert!(source.is_stopped());
    }
}
