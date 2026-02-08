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

//! Pluggable task queue implementations.
//!
//! This module provides different task queue strategies that can be used
//! with the executor. Different implementations allow testing system behavior
//! under various scheduling scenarios.

use super::task::Task;
use std::collections::VecDeque;

/// Trait for task queue implementations.
///
/// Allows plugging different queue strategies into the executor to test
/// behavior under different scheduling scenarios.
pub trait TaskQueue: Send {
    /// Push a task onto the queue.
    fn push(&mut self, task: Task);

    /// Pop a task from the queue.
    ///
    /// Returns `None` if the queue is empty.
    fn pop(&mut self) -> Option<Task>;

    /// Get the number of tasks in the queue.
    fn len(&self) -> usize;

    /// Check if the queue is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// FIFO queue (First-In-First-Out) - deterministic ordering.
///
/// This is the default implementation that maintains strict FIFO ordering.
/// Tasks are processed in the exact order they were submitted.
///
/// # Examples
///
/// ```
/// use adaptive_engine::executor::queue::{TaskQueue, FifoQueue};
///
/// let mut queue = FifoQueue::new();
/// assert!(queue.is_empty());
/// ```
pub struct FifoQueue {
    inner: VecDeque<Task>,
}

impl FifoQueue {
    /// Create a new FIFO queue.
    pub fn new() -> Self {
        Self {
            inner: VecDeque::new(),
        }
    }
}

impl Default for FifoQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskQueue for FifoQueue {
    fn push(&mut self, task: Task) {
        self.inner.push_back(task);
    }

    fn pop(&mut self) -> Option<Task> {
        self.inner.pop_front()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// Random queue - non-deterministic ordering for stress testing.
///
/// This implementation randomly selects tasks when popping, which helps
/// test that the system handles out-of-order execution correctly.
///
/// **Use case**: Stress testing, detecting race conditions, verifying that
/// correctness doesn't depend on task ordering.
///
/// # Examples
///
/// ```
/// use adaptive_engine::executor::queue::{TaskQueue, RandomQueue};
///
/// let mut queue = RandomQueue::new();
/// // Tasks will be processed in random order
/// ```
pub struct RandomQueue {
    inner: Vec<Task>,
}

impl RandomQueue {
    /// Create a new random queue.
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }
}

impl Default for RandomQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskQueue for RandomQueue {
    fn push(&mut self, task: Task) {
        self.inner.push(task);
    }

    fn pop(&mut self) -> Option<Task> {
        if self.inner.is_empty() {
            return None;
        }

        // Pick a random index
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let idx = rng.gen_range(0..self.inner.len());
        Some(self.inner.swap_remove(idx))
    }

    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// LIFO queue (Last-In-First-Out) - stack-based ordering.
///
/// This implementation uses a stack (LIFO) ordering, which can expose
/// different execution patterns and potential issues.
///
/// **Use case**: Testing depth-first vs breadth-first execution patterns.
///
/// # Examples
///
/// ```
/// use adaptive_engine::executor::queue::{TaskQueue, LifoQueue};
///
/// let mut queue = LifoQueue::new();
/// // Most recent tasks processed first
/// ```
pub struct LifoQueue {
    inner: Vec<Task>,
}

impl LifoQueue {
    /// Create a new LIFO queue.
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }
}

impl Default for LifoQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskQueue for LifoQueue {
    fn push(&mut self, task: Task) {
        self.inner.push(task);
    }

    fn pop(&mut self) -> Option<Task> {
        self.inner.pop()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// Priority queue - prioritizes certain task types.
///
/// This implementation processes tasks based on priority:
/// 1. DeployGraph (highest priority - infrastructure)
/// 2. StartSource (source initialization)
/// 3. StopPipelineTask (cascading shutdown)
/// 4. EndOfStream (lifecycle management)
/// 5. WorkTask (actual work)
/// 6. Shutdown (lowest priority - cleanup)
///
/// **Use case**: Testing behavior when lifecycle operations are prioritized.
///
/// # Examples
///
/// ```
/// use adaptive_engine::executor::queue::{TaskQueue, PriorityQueue};
///
/// let mut queue = PriorityQueue::new();
/// // Deploy and lifecycle tasks processed before work tasks
/// ```
pub struct PriorityQueue {
    inner: Vec<Task>,
}

impl PriorityQueue {
    /// Create a new priority queue.
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Get priority for a task (lower number = higher priority).
    fn priority(task: &Task) -> u8 {
        match task {
            Task::DeployGraph { .. } => 0,
            Task::StartSource { .. } => 1,
            Task::SourceError { .. } => 2,
            Task::StopPipelineTask { .. } => 2,
            Task::EndOfStream { .. } => 3,
            Task::WorkTask { .. } => 4,
            Task::Shutdown => 5,
        }
    }
}

impl Default for PriorityQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskQueue for PriorityQueue {
    fn push(&mut self, task: Task) {
        self.inner.push(task);
    }

    fn pop(&mut self) -> Option<Task> {
        if self.inner.is_empty() {
            return None;
        }

        // Find highest priority task (lowest priority number)
        let (idx, _) = self
            .inner
            .iter()
            .enumerate()
            .min_by_key(|(_, task)| Self::priority(task))?;

        Some(self.inner.swap_remove(idx))
    }

    fn len(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Buffer, PipelineId};
    #[test]
    fn test_fifo_queue_ordering() {
        let mut queue = FifoQueue::new();

        // Push tasks
        queue.push(Task::Shutdown);
        queue.push(Task::EndOfStream {
            query_id: 0,
            source_id: PipelineId::new("src"),
            pipeline_id: PipelineId::new("p1"),
        });

        // Pop in FIFO order
        assert!(matches!(queue.pop(), Some(Task::Shutdown)));
        assert!(matches!(queue.pop(), Some(Task::EndOfStream { .. })));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_lifo_queue_ordering() {
        let mut queue = LifoQueue::new();

        // Push tasks
        queue.push(Task::Shutdown);
        queue.push(Task::EndOfStream {
            query_id: 0,
            source_id: PipelineId::new("src"),
            pipeline_id: PipelineId::new("p1"),
        });

        // Pop in LIFO order (reversed)
        assert!(matches!(queue.pop(), Some(Task::EndOfStream { .. })));
        assert!(matches!(queue.pop(), Some(Task::Shutdown)));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_priority_queue_ordering() {
        let mut queue = PriorityQueue::new();

        // Push tasks in arbitrary order
        queue.push(Task::WorkTask {
            query_id: 0,
            pipeline_id: PipelineId::new("p1"),
            buffer: Buffer::new(vec![1]),
        });
        queue.push(Task::Shutdown);
        queue.push(Task::EndOfStream {
            query_id: 0,
            source_id: PipelineId::new("src"),
            pipeline_id: PipelineId::new("p1"),
        });

        // Should pop EndOfStream first (higher priority than WorkTask)
        assert!(matches!(queue.pop(), Some(Task::EndOfStream { .. })));
        // Then WorkTask
        assert!(matches!(queue.pop(), Some(Task::WorkTask { .. })));
        // Finally Shutdown
        assert!(matches!(queue.pop(), Some(Task::Shutdown)));
    }

    #[test]
    fn test_random_queue_processes_all() {
        let mut queue = RandomQueue::new();

        // Push multiple tasks
        for i in 0..10 {
            queue.push(Task::WorkTask {
                query_id: 0,
                pipeline_id: PipelineId::new(format!("p{}", i)),
                buffer: Buffer::new(vec![i as u8]),
            });
        }

        assert_eq!(queue.len(), 10);

        // Pop all tasks (order doesn't matter, but count does)
        let mut count = 0;
        while queue.pop().is_some() {
            count += 1;
        }

        assert_eq!(count, 10);
        assert!(queue.is_empty());
    }
}
