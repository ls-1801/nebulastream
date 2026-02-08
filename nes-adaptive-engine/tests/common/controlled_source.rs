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

//! Controllable source for integration tests.
//!
//! Provides `ControlledSource` and `SourceController`, matching the C++
//! `TestSource`/`TestSourceController` pattern.

#![allow(dead_code)]

use adaptive_engine::pipeline::{Buffer, PipelineId};
use adaptive_engine::source::{Source, SourceEmitHandle, SourceError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Commands sent from controller to source worker thread.
enum SourceCommand {
    EmitBuffer(Buffer),
    EmitError(String),
    EndOfStream,
}

/// Controller handle for injecting data and checking lifecycle state.
pub struct SourceController {
    command_tx: Sender<SourceCommand>,
    stopped: Arc<(Mutex<bool>, Condvar)>,
    started: Arc<(Mutex<bool>, Condvar)>,
    buffers_emitted: Arc<AtomicUsize>,
}

impl SourceController {
    /// Inject a buffer to be emitted by the source.
    pub fn inject_buffer(&self, buffer: Buffer) {
        let _ = self.command_tx.send(SourceCommand::EmitBuffer(buffer));
    }

    /// Inject an error, causing the source to report a failure.
    pub fn inject_error(&self, msg: &str) {
        let _ = self
            .command_tx
            .send(SourceCommand::EmitError(msg.to_string()));
    }

    /// Signal end-of-stream (the source will stop cleanly).
    pub fn end_of_stream(&self) {
        let _ = self.command_tx.send(SourceCommand::EndOfStream);
    }

    /// Check if the source worker has stopped (non-blocking).
    pub fn is_stopped(&self) -> bool {
        *self.stopped.0.lock().unwrap()
    }

    /// Wait until the source worker has stopped, with a timeout.
    pub fn wait_stopped(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.stopped;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Wait until the source has been started (open() completed), with a timeout.
    pub fn wait_started(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.started;
        let guard = lock.lock().unwrap();
        if *guard {
            return true;
        }
        let (guard, _) = cvar.wait_timeout(guard, timeout).unwrap();
        *guard
    }

    /// Number of buffers emitted by this source.
    pub fn buffers_emitted(&self) -> usize {
        self.buffers_emitted.load(Ordering::SeqCst)
    }
}

/// A controllable source for integration tests.
pub struct ControlledSource {
    id: PipelineId,
    command_rx: Arc<Mutex<Receiver<SourceCommand>>>,
    stopped: Arc<(Mutex<bool>, Condvar)>,
    started: Arc<(Mutex<bool>, Condvar)>,
    thread_handle: Mutex<Option<thread::JoinHandle<()>>>,
    buffers_emitted: Arc<AtomicUsize>,
    fail_during_open: AtomicBool,
    fail_during_open_delay_ms: AtomicUsize,
}

impl ControlledSource {
    /// Configure the source to fail during open (setup/start) after an optional delay.
    pub fn set_fail_during_open(&self, delay_ms: u64) {
        self.fail_during_open.store(true, Ordering::SeqCst);
        self.fail_during_open_delay_ms
            .store(delay_ms as usize, Ordering::SeqCst);
    }
}

/// Create a controlled source and its controller.
pub fn controlled_source(id: &str) -> (Arc<ControlledSource>, SourceController) {
    let (command_tx, command_rx) = mpsc::channel();
    let stopped = Arc::new((Mutex::new(false), Condvar::new()));
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let buffers_emitted = Arc::new(AtomicUsize::new(0));

    let source = Arc::new(ControlledSource {
        id: PipelineId::new(id),
        command_rx: Arc::new(Mutex::new(command_rx)),
        stopped: stopped.clone(),
        started: started.clone(),
        thread_handle: Mutex::new(None),
        buffers_emitted: buffers_emitted.clone(),
        fail_during_open: AtomicBool::new(false),
        fail_during_open_delay_ms: AtomicUsize::new(0),
    });

    let controller = SourceController {
        command_tx,
        stopped,
        started,
        buffers_emitted,
    };

    (source, controller)
}

impl Source for ControlledSource {
    fn start(&self, emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
        // Check if we should fail during open
        if self.fail_during_open.load(Ordering::SeqCst) {
            let delay_ms = self.fail_during_open_delay_ms.load(Ordering::SeqCst);
            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms as u64));
            }
            // Signal started before failing so waits don't hang
            {
                let (lock, cvar) = &*self.started;
                *lock.lock().unwrap() = true;
                cvar.notify_all();
            }
            return Err(SourceError::StartFailed(
                "Configured to fail during open".to_string(),
            ));
        }

        let command_rx = self.command_rx.clone();
        let stopped = self.stopped.clone();
        let started = self.started.clone();
        let buffers_emitted = self.buffers_emitted.clone();

        let worker = thread::spawn(move || {
            // Signal that we've started
            {
                let (lock, cvar) = &*started;
                *lock.lock().unwrap() = true;
                cvar.notify_all();
            }

            loop {
                // Check if stop was requested by the executor
                if emit_handle.should_stop() {
                    break;
                }

                // Try to receive a command
                let command = {
                    let rx = command_rx.lock().unwrap();
                    rx.recv_timeout(Duration::from_millis(10))
                };

                match command {
                    Ok(SourceCommand::EmitBuffer(buffer)) => {
                        if let Err(_e) = emit_handle.emit(buffer) {
                            break;
                        }
                        buffers_emitted.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(SourceCommand::EmitError(msg)) => {
                        let _ = emit_handle.signal_error(&msg);
                        break;
                    }
                    Ok(SourceCommand::EndOfStream) => {
                        let _ = emit_handle.end_of_stream();
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }

            // Signal that we've stopped
            let (lock, cvar) = &*stopped;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        });

        *self.thread_handle.lock().unwrap() = Some(worker);
        Ok(())
    }

    fn stop(&self) -> Result<(), SourceError> {
        Ok(())
    }

    fn teardown(&self) -> Result<(), SourceError> {
        let handle = self.thread_handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        // Ensure stopped is signaled even if thread didn't run
        let (lock, cvar) = &*self.stopped;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}
