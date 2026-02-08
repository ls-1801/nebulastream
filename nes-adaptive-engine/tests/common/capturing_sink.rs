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

//! Capturing sink for integration tests.
//!
//! Provides `CapturingSink` and `SinkController`, matching the C++
//! `TestSink`/`TestSinkController` pattern.

#![allow(dead_code)]

use adaptive_engine::executor::PipelineExecutionContext;
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Controller for checking captured buffers and lifecycle state.
pub struct SinkController {
    buffers: Arc<(Mutex<Vec<Buffer>>, Condvar)>,
    invocation_count: Arc<AtomicUsize>,
    stop_call_count: Arc<AtomicUsize>,
    repeat_count: Arc<AtomicUsize>,
    repeat_count_during_stop: Arc<AtomicUsize>,
    started: Arc<(Mutex<bool>, Condvar)>,
    stopped: Arc<(Mutex<bool>, Condvar)>,
    destroyed: Arc<(Mutex<bool>, Condvar)>,
}

impl SinkController {
    /// Wait until at least `count` buffers have been captured, with a timeout.
    pub fn wait_for_buffers(&self, count: usize, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &*self.buffers;
        let guard = lock.lock().unwrap();
        if guard.len() >= count {
            return true;
        }
        let (guard, _) = cvar
            .wait_timeout_while(guard, timeout, |bufs| bufs.len() < count)
            .unwrap();
        guard.len() >= count
    }

    /// Take all captured buffers (drains the internal list).
    pub fn take_buffers(&self) -> Vec<Buffer> {
        let (lock, _) = &*self.buffers;
        let mut guard = lock.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// Get the current buffer count (non-blocking).
    pub fn buffer_count(&self) -> usize {
        let (lock, _) = &*self.buffers;
        lock.lock().unwrap().len()
    }

    /// Get the number of times execute() was called.
    pub fn invocation_count(&self) -> usize {
        self.invocation_count.load(Ordering::SeqCst)
    }

    /// Get the number of times teardown (stop) was called.
    pub fn stop_call_count(&self) -> usize {
        self.stop_call_count.load(Ordering::SeqCst)
    }

    /// Set the number of times each buffer should be re-executed via repeat_task.
    pub fn set_repeat_count(&self, n: usize) {
        self.repeat_count.store(n, Ordering::SeqCst);
    }

    /// Set the number of times teardown (stop) should be repeated via repeat_task.
    pub fn set_repeat_count_during_stop(&self, n: usize) {
        self.repeat_count_during_stop.store(n, Ordering::SeqCst);
    }

    /// Wait until setup() has been called.
    pub fn wait_for_start(&self, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &*self.started;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Wait until teardown() has been called.
    pub fn wait_for_stop(&self, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &*self.stopped;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Wait until the sink has been dropped.
    pub fn wait_for_destruction(&self, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &*self.destroyed;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Returns true if teardown was NOT called within timeout (sink keeps running).
    pub fn keep_running(&self, timeout: std::time::Duration) -> bool {
        !self.wait_for_stop(timeout)
    }
}

/// A capturing sink that stores all received buffers.
pub struct CapturingSink {
    id: PipelineId,
    buffers: Arc<(Mutex<Vec<Buffer>>, Condvar)>,
    invocation_count: Arc<AtomicUsize>,
    stop_call_count: Arc<AtomicUsize>,
    repeat_count: Arc<AtomicUsize>,
    repeat_count_during_stop: Arc<AtomicUsize>,
    started: Arc<(Mutex<bool>, Condvar)>,
    stopped: Arc<(Mutex<bool>, Condvar)>,
    destroyed: Arc<(Mutex<bool>, Condvar)>,
}

/// Create a capturing sink and its controller.
pub fn capturing_sink(id: &str) -> (Box<CapturingSink>, SinkController) {
    let buffers = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let invocation_count = Arc::new(AtomicUsize::new(0));
    let stop_call_count = Arc::new(AtomicUsize::new(0));
    let repeat_count = Arc::new(AtomicUsize::new(0));
    let repeat_count_during_stop = Arc::new(AtomicUsize::new(0));
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let stopped = Arc::new((Mutex::new(false), Condvar::new()));
    let destroyed = Arc::new((Mutex::new(false), Condvar::new()));

    let sink = Box::new(CapturingSink {
        id: PipelineId::new(id),
        buffers: buffers.clone(),
        invocation_count: invocation_count.clone(),
        stop_call_count: stop_call_count.clone(),
        repeat_count: repeat_count.clone(),
        repeat_count_during_stop: repeat_count_during_stop.clone(),
        started: started.clone(),
        stopped: stopped.clone(),
        destroyed: destroyed.clone(),
    });

    let controller = SinkController {
        buffers,
        invocation_count,
        stop_call_count,
        repeat_count,
        repeat_count_during_stop,
        started,
        stopped,
        destroyed,
    };

    (sink, controller)
}

impl Pipeline for CapturingSink {
    fn setup(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        let (lock, cvar) = &*self.started;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
        Ok(())
    }

    fn execute(
        &self,
        input: Buffer,
        context: &dyn PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let count = self.invocation_count.fetch_add(1, Ordering::SeqCst) + 1;

        // Capture the buffer
        {
            let (lock, cvar) = &*self.buffers;
            lock.lock().unwrap().push(input.clone());
            cvar.notify_all();
        }

        // Handle repeat_task
        let repeat = self.repeat_count.load(Ordering::SeqCst);
        if repeat > 0 && count <= repeat {
            context.repeat_task(input, 0);
        }

        // Sinks don't emit downstream
        Ok(vec![])
    }

    fn teardown(&self, context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        let call_count = self.stop_call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // Handle repeat_task during stop
        let repeat_stop = self.repeat_count_during_stop.load(Ordering::SeqCst);
        if repeat_stop > 0 && call_count <= repeat_stop {
            context.repeat_task(Buffer::new(vec![]), 0);
            return Ok(());
        }

        let (lock, cvar) = &*self.stopped;
        *lock.lock().unwrap() = true;
        cvar.notify_all();

        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

impl Drop for CapturingSink {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.destroyed;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
}
