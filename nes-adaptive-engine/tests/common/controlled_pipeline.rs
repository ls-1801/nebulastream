//! Controllable pipeline for integration tests.
//!
//! Provides `ControlledPipeline` and `PipelineController`, matching the C++
//! `TestPipeline`/`TestPipelineController` pattern.

#![allow(dead_code)]

use adaptive_engine::executor::PipelineExecutionContext;
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// Controller for configuring pipeline behavior and checking lifecycle state.
pub struct PipelineController {
    fail_on_setup: AtomicBool,
    fail_on_execute_nth: AtomicUsize,
    fail_on_teardown: AtomicBool,
    repeat_count: AtomicUsize,
    repeat_count_during_stop: AtomicUsize,
    invocation_count: AtomicUsize,
    stop_call_count: AtomicUsize,
    setup_called: AtomicBool,
    teardown_called: AtomicBool,
    started: (Mutex<bool>, Condvar),
    stopped: (Mutex<bool>, Condvar),
    destroyed: (Mutex<bool>, Condvar),
}

impl PipelineController {
    fn new() -> Self {
        Self {
            fail_on_setup: AtomicBool::new(false),
            fail_on_execute_nth: AtomicUsize::new(usize::MAX),
            fail_on_teardown: AtomicBool::new(false),
            repeat_count: AtomicUsize::new(0),
            repeat_count_during_stop: AtomicUsize::new(0),
            invocation_count: AtomicUsize::new(0),
            stop_call_count: AtomicUsize::new(0),
            setup_called: AtomicBool::new(false),
            teardown_called: AtomicBool::new(false),
            started: (Mutex::new(false), Condvar::new()),
            stopped: (Mutex::new(false), Condvar::new()),
            destroyed: (Mutex::new(false), Condvar::new()),
        }
    }

    /// Configure the pipeline to fail during setup.
    pub fn fail_on_setup(&self) {
        self.fail_on_setup.store(true, Ordering::SeqCst);
    }

    /// Configure the pipeline to fail on the nth execute() call (1-indexed).
    pub fn fail_on_execute_nth(&self, n: usize) {
        self.fail_on_execute_nth.store(n, Ordering::SeqCst);
    }

    /// Configure the pipeline to fail during teardown.
    pub fn fail_on_teardown(&self) {
        self.fail_on_teardown.store(true, Ordering::SeqCst);
    }

    /// Set the number of times each buffer should be re-executed via repeat_task.
    pub fn set_repeat_count(&self, n: usize) {
        self.repeat_count.store(n, Ordering::SeqCst);
    }

    /// Set the number of times teardown (stop) should be repeated via repeat_task.
    pub fn set_repeat_count_during_stop(&self, n: usize) {
        self.repeat_count_during_stop.store(n, Ordering::SeqCst);
    }

    /// Get the number of times execute() was called.
    pub fn invocation_count(&self) -> usize {
        self.invocation_count.load(Ordering::SeqCst)
    }

    /// Get the number of times teardown (stop) was called.
    pub fn stop_call_count(&self) -> usize {
        self.stop_call_count.load(Ordering::SeqCst)
    }

    /// Check if setup() was called.
    pub fn was_setup_called(&self) -> bool {
        self.setup_called.load(Ordering::SeqCst)
    }

    /// Check if teardown() was called.
    pub fn was_teardown_called(&self) -> bool {
        self.teardown_called.load(Ordering::SeqCst)
    }

    /// Wait until setup() has been called.
    pub fn wait_for_start(&self, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &self.started;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Wait until teardown() has been called.
    pub fn wait_for_stop(&self, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &self.stopped;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Wait until the pipeline has been dropped.
    pub fn wait_for_destruction(&self, timeout: std::time::Duration) -> bool {
        let (lock, cvar) = &self.destroyed;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Returns true if teardown was NOT called within timeout (pipeline keeps running).
    pub fn keep_running(&self, timeout: std::time::Duration) -> bool {
        !self.wait_for_stop(timeout)
    }
}

/// A controllable pipeline for integration tests.
pub struct ControlledPipeline {
    id: PipelineId,
    controller: std::sync::Arc<PipelineController>,
}

/// Create a controlled pipeline and its controller.
pub fn controlled_pipeline(
    id: &str,
) -> (Box<ControlledPipeline>, std::sync::Arc<PipelineController>) {
    let controller = std::sync::Arc::new(PipelineController::new());
    let pipeline = Box::new(ControlledPipeline {
        id: PipelineId::new(id),
        controller: controller.clone(),
    });
    (pipeline, controller)
}

impl Pipeline for ControlledPipeline {
    fn setup(&self, _context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        self.controller.setup_called.store(true, Ordering::SeqCst);

        if self.controller.fail_on_setup.load(Ordering::SeqCst) {
            return Err(PipelineError::ExecutionFailed(
                "Configured to fail during setup".to_string(),
            ));
        }

        let (lock, cvar) = &self.controller.started;
        *lock.lock().unwrap() = true;
        cvar.notify_all();

        Ok(())
    }

    fn execute(
        &self,
        input: Buffer,
        context: &dyn PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let count = self
            .controller
            .invocation_count
            .fetch_add(1, Ordering::SeqCst)
            + 1;

        let fail_nth = self.controller.fail_on_execute_nth.load(Ordering::SeqCst);
        if count >= fail_nth {
            return Err(PipelineError::ExecutionFailed(format!(
                "Configured to fail on invocation {}",
                count
            )));
        }

        // Handle repeat_task
        let repeat = self.controller.repeat_count.load(Ordering::SeqCst);
        if repeat > 0 && count <= repeat {
            context.repeat_task(input.clone(), 0);
        }

        Ok(vec![input])
    }

    fn teardown(&self, context: &dyn PipelineExecutionContext) -> Result<(), PipelineError> {
        let call_count = self
            .controller
            .stop_call_count
            .fetch_add(1, Ordering::SeqCst)
            + 1;

        // Handle repeat_task during stop
        let repeat_stop = self
            .controller
            .repeat_count_during_stop
            .load(Ordering::SeqCst);
        if repeat_stop > 0 && call_count <= repeat_stop {
            context.repeat_task(Buffer::new(vec![]), 0);
            return Ok(());
        }

        self.controller
            .teardown_called
            .store(true, Ordering::SeqCst);

        if self.controller.fail_on_teardown.load(Ordering::SeqCst) {
            return Err(PipelineError::ExecutionFailed(
                "Configured to fail during teardown".to_string(),
            ));
        }

        let (lock, cvar) = &self.controller.stopped;
        *lock.lock().unwrap() = true;
        cvar.notify_all();

        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

impl Drop for ControlledPipeline {
    fn drop(&mut self) {
        let (lock, cvar) = &self.controller.destroyed;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
}
