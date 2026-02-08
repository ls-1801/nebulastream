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

//! Statistics event collector for integration tests.
//!
//! Provides `StatsCollector` for collecting and querying statistics events,
//! matching the C++ `StatisticsListener` pattern.

#![allow(dead_code)]

use adaptive_engine::engine::StatsReceiver;
use adaptive_engine::executor::stats::StatisticsEvent;
use adaptive_engine::executor::QueryId;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Collects statistics events from a `StatsReceiver` in a background thread.
pub struct StatsCollector {
    events: Arc<(Mutex<Vec<StatisticsEvent>>, Condvar)>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    poll_thread: Option<thread::JoinHandle<()>>,
}

impl StatsCollector {
    /// Create a new stats collector that polls the given receiver.
    pub fn new(receiver: StatsReceiver) -> Self {
        let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let events_clone = events.clone();
        let stop_clone = stop_flag.clone();

        let poll_thread = thread::spawn(move || {
            while !stop_clone.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(event) = receiver.poll(Duration::from_millis(50)) {
                    let (lock, cvar) = &*events_clone;
                    lock.lock().unwrap().push(event);
                    cvar.notify_all();
                }
            }
            // Drain remaining events
            for event in receiver.drain() {
                let (lock, cvar) = &*events_clone;
                lock.lock().unwrap().push(event);
                cvar.notify_all();
            }
        });

        Self {
            events,
            stop_flag,
            poll_thread: Some(poll_thread),
        }
    }

    /// Stop polling and join the background thread.
    pub fn stop(&mut self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.poll_thread.take() {
            let _ = handle.join();
        }
    }

    /// Get a snapshot of all collected events.
    pub fn get_events(&self) -> Vec<StatisticsEvent> {
        let (lock, _) = &*self.events;
        lock.lock().unwrap().clone()
    }

    /// Wait for at least `n` total events, with a timeout.
    pub fn wait_for_events(&self, n: usize, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.events;
        let guard = lock.lock().unwrap();
        if guard.len() >= n {
            return true;
        }
        let (guard, _) = cvar
            .wait_timeout_while(guard, timeout, |evts| evts.len() < n)
            .unwrap();
        guard.len() >= n
    }

    /// Count events matching a predicate.
    fn count_matching<F: Fn(&StatisticsEvent) -> bool>(&self, pred: F) -> usize {
        let (lock, _) = &*self.events;
        lock.lock().unwrap().iter().filter(|e| pred(e)).count()
    }

    /// Wait until the predicate count reaches `n`, with a timeout.
    fn wait_for_count<F: Fn(&StatisticsEvent) -> bool>(
        &self,
        n: usize,
        pred: F,
        timeout: Duration,
    ) -> bool {
        let (lock, cvar) = &*self.events;
        let guard = lock.lock().unwrap();
        let count = guard.iter().filter(|e| pred(e)).count();
        if count >= n {
            return true;
        }
        let (guard, _) = cvar
            .wait_timeout_while(guard, timeout, |evts| {
                evts.iter().filter(|e| pred(e)).count() < n
            })
            .unwrap();
        guard.iter().filter(|e| pred(e)).count() >= n
    }

    // --- Event counting methods ---

    pub fn count_query_starts(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::QueryStart { .. }))
    }

    pub fn count_query_stops(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::QueryStop { .. }))
    }

    pub fn count_query_running(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::QueryRunning { .. }))
    }

    pub fn count_query_terminated(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::QueryTerminated { .. }))
    }

    pub fn count_pipeline_starts(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::PipelineStart { .. }))
    }

    pub fn count_pipeline_stops(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::PipelineStop { .. }))
    }

    pub fn count_task_execution_starts(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::TaskExecutionStart { .. }))
    }

    pub fn count_task_execution_completes(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::TaskExecutionComplete { .. }))
    }

    pub fn count_task_emits(&self) -> usize {
        self.count_matching(|e| matches!(e, StatisticsEvent::TaskEmit { .. }))
    }

    // --- Wait methods ---

    pub fn wait_for_query_running_id(&self, query_id: QueryId, timeout: Duration) -> bool {
        self.wait_for_count(
            1,
            |e| matches!(e, StatisticsEvent::QueryRunning { query_id: qid, .. } if *qid == query_id),
            timeout,
        )
    }

    pub fn wait_for_query_terminated_id(&self, query_id: QueryId, timeout: Duration) -> bool {
        self.wait_for_count(
            1,
            |e| matches!(e, StatisticsEvent::QueryTerminated { query_id: qid, .. } if *qid == query_id),
            timeout,
        )
    }

    pub fn wait_for_query_starts(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::QueryStart { .. }),
            timeout,
        )
    }

    pub fn wait_for_query_stops(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::QueryStop { .. }),
            timeout,
        )
    }

    pub fn wait_for_query_terminated(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::QueryTerminated { .. }),
            timeout,
        )
    }

    pub fn wait_for_pipeline_starts(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::PipelineStart { .. }),
            timeout,
        )
    }

    pub fn wait_for_pipeline_stops(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::PipelineStop { .. }),
            timeout,
        )
    }

    pub fn wait_for_task_execution_starts(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::TaskExecutionStart { .. }),
            timeout,
        )
    }

    pub fn wait_for_task_execution_completes(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::TaskExecutionComplete { .. }),
            timeout,
        )
    }

    pub fn wait_for_task_emits(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::TaskEmit { .. }),
            timeout,
        )
    }

    /// Expect exactly `n` events of a given type within the timeout.
    pub fn expect_query_starts(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_query_starts(n, timeout) && self.count_query_starts() == n
    }

    pub fn expect_query_running(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_count(
            n,
            |e| matches!(e, StatisticsEvent::QueryRunning { .. }),
            timeout,
        ) && self.count_query_running() == n
    }

    pub fn expect_pipeline_starts(&self, n: usize, timeout: Duration) -> bool {
        self.wait_for_pipeline_starts(n, timeout) && self.count_pipeline_starts() == n
    }
}

impl Drop for StatsCollector {
    fn drop(&mut self) {
        self.stop();
    }
}
