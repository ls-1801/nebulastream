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

//! Delayed task submitter for handling repeat_task functionality.
//!
//! The `DelayedTaskSubmitter` manages delayed task resubmission in a separate
//! thread. When a pipeline calls `repeat_task(delay_ms)`, the current task is
//! sent to this thread which sleeps for the specified delay and then pushes
//! the task back into the executor's task queue.
//!
//! # Threading Model
//!
//! The DelayedTaskSubmitter runs in its own thread, separate from the executor
//! thread. It receives delayed tasks via a channel and maintains its own sleep
//! loop for each task.
//!
//! # Shutdown Behavior
//!
//! On shutdown, the DelayedTaskSubmitter:
//! 1. Receives a shutdown signal via its channel
//! 2. Discards all pending delayed tasks (they won't be resubmitted)
//! 3. Terminates the thread

use super::queue::TaskQueue;
use super::task::Task;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Message sent to the DelayedTaskSubmitter thread.
pub enum DelayedMessage {
    /// A task to be resubmitted after the specified delay.
    DelayedTask {
        /// The task to resubmit (boxed to reduce enum size)
        task: Box<Task>,
        /// Delay in milliseconds before resubmitting
        delay_ms: u64,
    },
    /// Signal to shut down the DelayedTaskSubmitter thread.
    Shutdown,
}

/// Handle for communicating with the DelayedTaskSubmitter thread.
///
/// This handle is held by the executor and used to send delayed tasks
/// to the submitter thread. It can be cloned for use in multiple contexts.
#[derive(Clone)]
pub struct DelayedTaskSubmitterHandle {
    /// Channel sender for communicating with the submitter thread
    sender: std::sync::mpsc::Sender<DelayedMessage>,
    /// Shared shutdown signal to interrupt sleeping delays
    shutdown_signal: Arc<(Mutex<bool>, Condvar)>,
}

impl DelayedTaskSubmitterHandle {
    /// Submit a task for delayed resubmission.
    ///
    /// # Arguments
    ///
    /// * `task` - The task to resubmit
    /// * `delay_ms` - Delay in milliseconds before resubmitting
    ///
    /// # Returns
    ///
    /// `true` if the task was successfully sent to the submitter thread,
    /// `false` if the channel is closed (submitter has shut down).
    pub fn submit_delayed(&self, task: Task, delay_ms: u64) -> bool {
        self.sender
            .send(DelayedMessage::DelayedTask {
                task: Box::new(task),
                delay_ms,
            })
            .is_ok()
    }

    /// Signal the DelayedTaskSubmitter to shut down.
    ///
    /// This will cause the submitter thread to discard all pending tasks
    /// and terminate. Any in-progress delay will be interrupted immediately.
    pub fn shutdown(&self) -> bool {
        // Signal the condvar to wake up any sleeping delay
        {
            let mut guard = self.shutdown_signal.0.lock().unwrap();
            *guard = true;
        }
        self.shutdown_signal.1.notify_all();
        self.sender.send(DelayedMessage::Shutdown).is_ok()
    }
}

/// Manages delayed task resubmission in a separate thread.
///
/// The submitter receives tasks via a channel, sleeps for the specified delay,
/// and then pushes the task back into the executor's task queue.
pub struct DelayedTaskSubmitter {
    /// Handle to the submitter thread
    thread_handle: Option<JoinHandle<()>>,
    /// Handle for sending messages to the thread
    handle: DelayedTaskSubmitterHandle,
}

impl DelayedTaskSubmitter {
    /// Create and start a new DelayedTaskSubmitter.
    ///
    /// # Arguments
    ///
    /// * `task_queue` - The executor's task queue where delayed tasks will be pushed
    /// * `task_available` - Condvar to notify worker threads when a delayed task is pushed
    ///
    /// # Returns
    ///
    /// A new DelayedTaskSubmitter with a running background thread.
    pub fn new(
        task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
        task_available: Arc<std::sync::Condvar>,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<DelayedMessage>();
        let shutdown_signal = Arc::new((Mutex::new(false), Condvar::new()));
        let shutdown_signal_clone = Arc::clone(&shutdown_signal);

        let thread_handle = thread::spawn(move || {
            Self::run_loop(receiver, task_queue, shutdown_signal_clone, task_available);
        });

        Self {
            thread_handle: Some(thread_handle),
            handle: DelayedTaskSubmitterHandle {
                sender,
                shutdown_signal,
            },
        }
    }

    /// Get a cloneable handle for communicating with the submitter.
    ///
    /// The handle can be cloned and shared across threads.
    pub fn get_handle(&self) -> DelayedTaskSubmitterHandle {
        self.handle.clone()
    }

    /// The main loop running in the submitter thread.
    ///
    /// Receives delayed tasks from the channel, sleeps for the specified delay,
    /// and pushes them back into the executor's task queue.
    fn run_loop(
        receiver: std::sync::mpsc::Receiver<DelayedMessage>,
        task_queue: Arc<Mutex<Box<dyn TaskQueue>>>,
        shutdown_signal: Arc<(Mutex<bool>, Condvar)>,
        task_available: Arc<std::sync::Condvar>,
    ) {
        loop {
            // Wait for a message
            match receiver.recv() {
                Ok(DelayedMessage::DelayedTask { task, delay_ms }) => {
                    // Wait for the specified delay, but remain responsive to shutdown
                    if delay_ms > 0 {
                        let (lock, cvar) = &*shutdown_signal;
                        let guard = lock.lock().unwrap();
                        // Wait until either the delay expires or shutdown is signaled
                        let (guard, _) = cvar
                            .wait_timeout_while(
                                guard,
                                Duration::from_millis(delay_ms),
                                |&mut shutting_down| !shutting_down,
                            )
                            .unwrap();
                        if *guard {
                            // Shutdown was signaled during the delay - discard task
                            break;
                        }
                    }

                    // Push the task back into the executor's queue (unboxing)
                    if let Ok(mut queue) = task_queue.lock() {
                        queue.push(*task);
                    }
                    // Notify a worker that a task is available
                    task_available.notify_one();
                    // If lock fails, silently drop the task (executor is likely shutting down)
                }
                Ok(DelayedMessage::Shutdown) => {
                    // Shutdown requested - exit the loop
                    break;
                }
                Err(_) => {
                    // Channel closed - sender dropped, exit the loop
                    break;
                }
            }
        }
    }

    /// Shut down the DelayedTaskSubmitter and wait for the thread to finish.
    ///
    /// This method sends a shutdown signal and joins the thread.
    /// Any pending delayed tasks will be discarded.
    pub fn shutdown(mut self) {
        // Send shutdown signal (ignore error if already closed)
        let _ = self.handle.shutdown();

        // Join the thread
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    /// Shut down and wait for the thread to finish, returning any error from join.
    ///
    /// This is similar to `shutdown()` but allows handling thread panics.
    pub fn shutdown_and_join(mut self) -> Result<(), Box<dyn std::any::Any + Send>> {
        // Send shutdown signal (ignore error if already closed)
        let _ = self.handle.shutdown();

        // Join the thread
        if let Some(handle) = self.thread_handle.take() {
            handle.join()
        } else {
            Ok(())
        }
    }
}

impl Drop for DelayedTaskSubmitter {
    fn drop(&mut self) {
        // Send shutdown signal if thread is still running
        let _ = self.handle.shutdown();

        // Join the thread to prevent orphaned threads
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::queue::FifoQueue;
    use crate::pipeline::{Buffer, PipelineId};
    use std::sync::Weak;
    use std::time::Instant;

    fn make_condvar() -> Arc<std::sync::Condvar> {
        Arc::new(std::sync::Condvar::new())
    }

    #[test]
    fn test_delayed_submitter_creation() {
        let queue: Arc<Mutex<Box<dyn TaskQueue>>> =
            Arc::new(Mutex::new(Box::new(FifoQueue::new())));
        let submitter = DelayedTaskSubmitter::new(queue, make_condvar());

        // Should be able to get a handle
        let _handle = submitter.get_handle();

        // Shutdown should work
        submitter.shutdown();
    }

    #[test]
    fn test_immediate_resubmission() {
        let queue: Arc<Mutex<Box<dyn TaskQueue>>> =
            Arc::new(Mutex::new(Box::new(FifoQueue::new())));
        let submitter = DelayedTaskSubmitter::new(Arc::clone(&queue), make_condvar());
        let handle = submitter.get_handle();

        // Submit a task with 0 delay
        let task = Task::WorkTask {
            pipeline_id: PipelineId::new("test"),
            node: Weak::new(),
            buffer: Buffer::new(vec![1, 2, 3]),
        };
        assert!(handle.submit_delayed(task, 0));

        // Give thread time to process
        thread::sleep(Duration::from_millis(10));

        // Task should be in the queue
        {
            let mut q = queue.lock().unwrap();
            let task = q.pop();
            assert!(matches!(task, Some(Task::WorkTask { .. })));
        }

        submitter.shutdown();
    }

    #[test]
    fn test_delayed_resubmission() {
        let queue: Arc<Mutex<Box<dyn TaskQueue>>> =
            Arc::new(Mutex::new(Box::new(FifoQueue::new())));
        let submitter = DelayedTaskSubmitter::new(Arc::clone(&queue), make_condvar());
        let handle = submitter.get_handle();

        let start = Instant::now();

        // Submit a task with 50ms delay
        let task = Task::WorkTask {
            pipeline_id: PipelineId::new("test"),
            node: Weak::new(),
            buffer: Buffer::new(vec![1, 2, 3]),
        };
        assert!(handle.submit_delayed(task, 50));

        // Task should not be in queue immediately
        {
            let q = queue.lock().unwrap();
            assert!(q.is_empty());
        }

        // Wait for delay + some margin
        thread::sleep(Duration::from_millis(100));

        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(50));

        // Task should now be in the queue
        {
            let mut q = queue.lock().unwrap();
            let task = q.pop();
            assert!(matches!(task, Some(Task::WorkTask { .. })));
        }

        submitter.shutdown();
    }

    #[test]
    fn test_shutdown_discards_pending() {
        let queue: Arc<Mutex<Box<dyn TaskQueue>>> =
            Arc::new(Mutex::new(Box::new(FifoQueue::new())));
        let submitter = DelayedTaskSubmitter::new(Arc::clone(&queue), make_condvar());
        let handle = submitter.get_handle();

        // Submit a task with long delay
        let task = Task::WorkTask {
            pipeline_id: PipelineId::new("test"),
            node: Weak::new(),
            buffer: Buffer::new(vec![1, 2, 3]),
        };
        assert!(handle.submit_delayed(task, 10000)); // 10 second delay

        // Shutdown immediately
        submitter.shutdown();

        // Task should NOT be in the queue (was discarded)
        {
            let q = queue.lock().unwrap();
            assert!(q.is_empty());
        }
    }

    #[test]
    fn test_handle_cloning() {
        let queue: Arc<Mutex<Box<dyn TaskQueue>>> =
            Arc::new(Mutex::new(Box::new(FifoQueue::new())));
        let submitter = DelayedTaskSubmitter::new(Arc::clone(&queue), make_condvar());

        let handle1 = submitter.get_handle();
        let handle2 = handle1.clone();

        // Both handles should work
        let task1 = Task::WorkTask {
            pipeline_id: PipelineId::new("test1"),
            node: Weak::new(),
            buffer: Buffer::new(vec![1]),
        };
        let task2 = Task::WorkTask {
            pipeline_id: PipelineId::new("test2"),
            node: Weak::new(),
            buffer: Buffer::new(vec![2]),
        };

        assert!(handle1.submit_delayed(task1, 0));
        assert!(handle2.submit_delayed(task2, 0));

        // Give thread time to process
        thread::sleep(Duration::from_millis(10));

        // Both tasks should be in the queue
        {
            let mut q = queue.lock().unwrap();
            assert!(q.pop().is_some());
            assert!(q.pop().is_some());
        }

        submitter.shutdown();
    }
}
