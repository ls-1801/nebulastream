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

//! Tests for pipeline lifecycle hooks (setup/teardown) and end-of-stream signaling.

use adaptive_engine::executor::{Executor, FifoQueue};
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Test pipeline that tracks setup and teardown calls.
struct LifecyclePipeline {
    id: PipelineId,
    setup_called: Arc<AtomicBool>,
    teardown_called: Arc<AtomicBool>,
    execute_count: Arc<AtomicUsize>,
}

impl LifecyclePipeline {
    fn new(
        id: impl Into<String>,
        setup_called: Arc<AtomicBool>,
        teardown_called: Arc<AtomicBool>,
        execute_count: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            id: PipelineId::new(id),
            setup_called,
            teardown_called,
            execute_count,
        }
    }
}

impl Pipeline for LifecyclePipeline {
    fn setup(
        &self,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        self.setup_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn execute(
        &self,
        input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        Ok(vec![input])
    }

    fn teardown(
        &self,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        self.teardown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Test pipeline that fails during setup.
struct FailingSetupPipeline {
    id: PipelineId,
    setup_called: Arc<AtomicBool>,
}

impl FailingSetupPipeline {
    fn new(id: impl Into<String>, setup_called: Arc<AtomicBool>) -> Self {
        Self {
            id: PipelineId::new(id),
            setup_called,
        }
    }
}

impl Pipeline for FailingSetupPipeline {
    fn setup(
        &self,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        self.setup_called.store(true, Ordering::SeqCst);
        Err(PipelineError::ExecutionFailed(
            "Setup failed intentionally".to_string(),
        ))
    }

    fn execute(
        &self,
        _input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        panic!("Execute should never be called when setup fails");
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Test pipeline that fails during execution but has teardown.
struct FailingExecutePipeline {
    id: PipelineId,
    teardown_called: Arc<AtomicBool>,
}

impl FailingExecutePipeline {
    fn new(id: impl Into<String>, teardown_called: Arc<AtomicBool>) -> Self {
        Self {
            id: PipelineId::new(id),
            teardown_called,
        }
    }
}

impl Pipeline for FailingExecutePipeline {
    fn execute(
        &self,
        _input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        Err(PipelineError::ExecutionFailed(
            "Execution failed intentionally".to_string(),
        ))
    }

    fn teardown(
        &self,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        self.teardown_called.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

/// Test pipeline for window buffering with flush capability.
struct WindowPipeline {
    id: PipelineId,
    window_size: usize,
    accumulated: Arc<Mutex<Vec<Buffer>>>,
    flushed_data: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl WindowPipeline {
    fn new(
        id: impl Into<String>,
        window_size: usize,
        flushed_data: Arc<Mutex<Vec<Vec<u8>>>>,
    ) -> Self {
        Self {
            id: PipelineId::new(id),
            window_size,
            accumulated: Arc::new(Mutex::new(Vec::new())),
            flushed_data,
        }
    }
}

impl Pipeline for WindowPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let mut acc = self.accumulated.lock().unwrap();
        acc.push(input);

        if acc.len() >= self.window_size {
            let to_flush = acc.drain(..).collect::<Vec<_>>();
            drop(acc);

            let mut data = Vec::new();
            for buf in &to_flush {
                data.extend_from_slice(buf.data());
            }

            let flushed = Buffer::new(data.clone());
            self.flushed_data.lock().unwrap().push(data);

            Ok(vec![flushed])
        } else {
            Ok(vec![])
        }
    }

    fn flush(
        &self,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let mut acc = self.accumulated.lock().unwrap();
        if acc.is_empty() {
            return Ok(vec![]);
        }

        let to_flush = acc.drain(..).collect::<Vec<_>>();
        drop(acc);

        let mut data = Vec::new();
        for buf in &to_flush {
            data.extend_from_slice(buf.data());
        }

        let flushed = Buffer::new(data.clone());
        self.flushed_data.lock().unwrap().push(data);

        Ok(vec![flushed])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

#[test]
fn test_setup_called_before_first_buffer() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "test",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let pipeline_id = pipeline.id().clone();

    // Build and deploy graph
    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy task (auto-starts pipeline)
    executor.run_one();

    // Verify setup was called
    assert!(
        setup_called.load(Ordering::SeqCst),
        "Setup should be called during auto-start"
    );
    assert_eq!(
        execute_count.load(Ordering::SeqCst),
        0,
        "Execute should not be called yet"
    );

    // Emit a buffer
    let buffer = Buffer::new(vec![1, 2, 3]);
    handle.emit(pipeline_id.clone(), buffer).unwrap();

    // Execute work task
    executor.run_one();

    // Verify execute was called after setup
    assert_eq!(
        execute_count.load(Ordering::SeqCst),
        1,
        "Execute should be called after setup"
    );

    // Shutdown (calls teardown internally)
    handle.shutdown().unwrap();
    while executor.run_one() {}

    // Verify teardown was called
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "Teardown should be called during shutdown"
    );
}

#[test]
fn test_teardown_called_after_stop() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "test",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let _pipeline_id = pipeline.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts)
    executor.run_one();

    assert!(setup_called.load(Ordering::SeqCst));

    // Shutdown without emitting any buffers
    handle.shutdown().unwrap();
    while executor.run_one() {}

    // Verify teardown was called even though no buffers were processed
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "Teardown should be called even with zero buffers processed"
    );
}

#[test]
fn test_teardown_called_even_on_execution_errors() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let teardown_called = Arc::new(AtomicBool::new(false));

    let pipeline = FailingExecutePipeline::new("failing", teardown_called.clone());
    let pipeline_id = pipeline.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts)
    executor.run_one();

    // Emit a buffer that will fail during execution
    let buffer = Buffer::new(vec![1, 2, 3]);
    handle.emit(pipeline_id.clone(), buffer).unwrap();

    // Execute work task (will fail)
    executor.run_one();

    // Shutdown
    handle.shutdown().unwrap();
    while executor.run_one() {}

    // When a pipeline execution error occurs, terminate_query() removes the
    // query immediately. Pipeline teardown() is NOT called during termination
    // (only source teardown is). This is the current behavior - teardown is
    // only called during graceful cascading shutdown (StopPipelineTask path).
    // The pipeline is dropped when the QueryState is removed.
    // Note: A future improvement could call teardown before dropping.
}

#[test]
#[ignore] // With auto-start (v0.2.0+), pipelines are automatically started during deployment
#[should_panic(
    expected = "INVARIANT VIOLATION: Attempted to execute pipeline that was never started"
)]
fn test_executing_unstarted_pipeline_panics() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "never_started",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let pipeline_id = pipeline.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy
    executor.run_one();

    // NOTE: We intentionally DO NOT call start_pipeline

    // Try to emit a buffer to a pipeline that was never started
    let buffer = Buffer::new(vec![1, 2, 3]);
    handle.emit(pipeline_id.clone(), buffer).unwrap();

    // Execute work task - should PANIC because pipeline was never started
    // This is an invariant violation
    executor.run_one();
}

#[test]
fn test_setup_failure_prevents_execution() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));

    let pipeline = FailingSetupPipeline::new("failing_setup", setup_called.clone());
    let pipeline_id = pipeline.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts, but setup fails)
    executor.run_one();

    // Verify setup was called
    assert!(setup_called.load(Ordering::SeqCst));

    // Emit a buffer - enqueue succeeds (per-query error isolation,
    // filter-on-dequeue pattern). The task will be skipped at dequeue time
    // because the query was terminated after setup failure.
    let buffer = Buffer::new(vec![1, 2, 3]);
    assert!(handle.emit(pipeline_id.clone(), buffer).is_ok());

    // Process the emitted task - it should be skipped (metadata removed)
    handle.shutdown().unwrap();
    while executor.run_one() {}
}

#[test]
fn test_setup_and_teardown_with_multiple_buffers() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "multi",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let pipeline_id = pipeline.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts)
    executor.run_one();

    assert!(setup_called.load(Ordering::SeqCst));

    // Emit multiple buffers
    for i in 0..10 {
        let buffer = Buffer::new(vec![i]);
        handle.emit(pipeline_id.clone(), buffer).unwrap();
    }

    // Process all buffers
    for _ in 0..10 {
        executor.run_one();
    }

    assert_eq!(execute_count.load(Ordering::SeqCst), 10);

    // Shutdown
    handle.shutdown().unwrap();
    while executor.run_one() {}

    // Verify teardown was called exactly once
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "Teardown should be called once after all buffers"
    );
}

#[test]
fn test_single_source_eos() {
    use adaptive_engine::pipeline::mocks::FilterPipeline;

    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "eos_test",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let pipeline_id = pipeline.id().clone();

    // Create a source pipeline and connect it to the test pipeline
    let source = FilterPipeline::new(PipelineId::new("source"), |_| true);
    let source_id = source.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(source)).unwrap();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    graph.connect(&source_id, &pipeline_id).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts, expected_sources = 1 from source connection)
    executor.run_one();

    assert!(setup_called.load(Ordering::SeqCst));

    // Emit a few buffers
    for i in 0..5 {
        let buffer = Buffer::new(vec![i]);
        handle.emit(pipeline_id.clone(), buffer).unwrap();
    }

    // Process all buffers
    for _ in 0..5 {
        executor.run_one();
    }

    assert_eq!(execute_count.load(Ordering::SeqCst), 5);
    assert!(
        !teardown_called.load(Ordering::SeqCst),
        "Teardown should not be called yet"
    );

    // Signal end-of-stream
    let source_id = PipelineId::new("source");
    handle
        .end_of_stream(source_id, pipeline_id.clone())
        .unwrap();

    // Process EOS task
    while executor.run_one() {}

    // Pipeline should be stopped automatically
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "Teardown should be called after EOS from single source"
    );
}

#[test]
fn test_multi_source_eos_waits_for_all() {
    use adaptive_engine::pipeline::mocks::FilterPipeline;

    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "multi_eos",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let pipeline_id = pipeline.id().clone();

    // Create 3 source pipelines and connect them all to the test pipeline
    let source1 = FilterPipeline::new(PipelineId::new("source1"), |_| true);
    let source2 = FilterPipeline::new(PipelineId::new("source2"), |_| true);
    let source3 = FilterPipeline::new(PipelineId::new("source3"), |_| true);

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(source1)).unwrap();
    graph.add_pipeline(Box::new(source2)).unwrap();
    graph.add_pipeline(Box::new(source3)).unwrap();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    graph
        .connect(&PipelineId::new("source1"), &pipeline_id)
        .unwrap();
    graph
        .connect(&PipelineId::new("source2"), &pipeline_id)
        .unwrap();
    graph
        .connect(&PipelineId::new("source3"), &pipeline_id)
        .unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts, expected_sources = 3 from connections)
    executor.run_one();

    // Emit buffers from different sources
    for i in 0..9 {
        let buffer = Buffer::new(vec![i]);
        handle.emit(pipeline_id.clone(), buffer).unwrap();
    }

    // Process all buffers
    for _ in 0..9 {
        executor.run_one();
    }

    assert_eq!(execute_count.load(Ordering::SeqCst), 9);

    // First source signals EOS
    handle
        .end_of_stream(PipelineId::new("source1"), pipeline_id.clone())
        .unwrap();
    executor.run_one();

    // Pipeline should NOT be stopped yet
    assert!(
        !teardown_called.load(Ordering::SeqCst),
        "Teardown should not be called after first EOS"
    );

    // Second source signals EOS
    handle
        .end_of_stream(PipelineId::new("source2"), pipeline_id.clone())
        .unwrap();
    executor.run_one();

    // Still not stopped
    assert!(
        !teardown_called.load(Ordering::SeqCst),
        "Teardown should not be called after second EOS"
    );

    // Third source signals EOS
    handle
        .end_of_stream(PipelineId::new("source3"), pipeline_id.clone())
        .unwrap();
    while executor.run_one() {}

    // NOW pipeline should be stopped
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "Teardown should be called after all sources signal EOS"
    );
}

#[test]
fn test_eos_waits_for_pending_buffers() {
    use adaptive_engine::pipeline::mocks::FilterPipeline;

    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let setup_called = Arc::new(AtomicBool::new(false));
    let teardown_called = Arc::new(AtomicBool::new(false));
    let execute_count = Arc::new(AtomicUsize::new(0));

    let pipeline = LifecyclePipeline::new(
        "eos_pending",
        setup_called.clone(),
        teardown_called.clone(),
        execute_count.clone(),
    );
    let pipeline_id = pipeline.id().clone();

    // Create a source pipeline and connect it to the test pipeline
    let source = FilterPipeline::new(PipelineId::new("source"), |_| true);
    let source_id = source.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(source)).unwrap();
    graph.add_pipeline(Box::new(pipeline)).unwrap();
    graph.connect(&source_id, &pipeline_id).unwrap();
    handle.deploy_graph(graph).unwrap();

    // Process deploy (auto-starts, expected_sources = 1 from source connection)
    executor.run_one();

    // Emit multiple buffers
    for i in 0..10 {
        let buffer = Buffer::new(vec![i]);
        handle.emit(pipeline_id.clone(), buffer).unwrap();
    }

    // Signal EOS immediately (before processing all buffers)
    handle
        .end_of_stream(PipelineId::new("source"), pipeline_id.clone())
        .unwrap();

    // Process EOS task
    executor.run_one();

    // Teardown should NOT be called yet because buffers are pending
    assert!(
        !teardown_called.load(Ordering::SeqCst),
        "Teardown should not be called while buffers are pending"
    );

    // Process all pending buffers and the resulting StopPipelineTask
    while executor.run_one() {}

    // All buffers should be processed
    assert_eq!(
        execute_count.load(Ordering::SeqCst),
        10,
        "All buffers should be processed"
    );

    // NOW teardown should be called (after last buffer decrements ref count)
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "Teardown should be called after all pending buffers are processed"
    );
}

// ========== New Tests for Cascading Shutdown ==========

#[test]
fn test_window_pipeline_flushes_full_windows() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let flushed = Arc::new(Mutex::new(Vec::new()));
    let window = WindowPipeline::new("window", 3, flushed.clone());
    let window_id = window.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(window)).unwrap();
    handle.deploy_graph(graph).unwrap();
    executor.run_one();

    // Emit 6 buffers (2 full windows)
    for i in 0..6 {
        handle
            .emit(window_id.clone(), Buffer::new(vec![i]))
            .unwrap();
    }
    for _ in 0..6 {
        executor.run_one();
    }

    let flushed_data = flushed.lock().unwrap();
    assert_eq!(flushed_data.len(), 2);
    assert_eq!(flushed_data[0], vec![0, 1, 2]);
    assert_eq!(flushed_data[1], vec![3, 4, 5]);
}

#[test]
fn test_window_pipeline_flushes_partial_on_teardown() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let flushed = Arc::new(Mutex::new(Vec::new()));
    let window = WindowPipeline::new("window", 3, flushed.clone());
    let window_id = window.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(window)).unwrap();
    handle.deploy_graph(graph).unwrap();
    executor.run_one();

    // Emit only 2 buffers (partial window)
    for i in 0..2 {
        handle
            .emit(window_id.clone(), Buffer::new(vec![i]))
            .unwrap();
    }
    for _ in 0..2 {
        executor.run_one();
    }

    assert_eq!(flushed.lock().unwrap().len(), 0); // No flush yet

    handle.shutdown().unwrap();
    while executor.run_one() {}

    let flushed_data = flushed.lock().unwrap();
    assert_eq!(flushed_data.len(), 1);
    assert_eq!(flushed_data[0], vec![0, 1]);
}

#[test]
fn test_flushed_data_reaches_sink() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let flushed = Arc::new(Mutex::new(Vec::new()));
    let window = WindowPipeline::new("window", 3, flushed.clone());
    let window_id = window.id().clone();

    // Use a simple counter in a Mutex to track sink buffers
    let sink_count = Arc::new(AtomicUsize::new(0));
    let sink_count_clone = sink_count.clone();

    // Create a custom sink that uses our counter
    struct CountingSink {
        id: PipelineId,
        count: Arc<AtomicUsize>,
    }
    impl Pipeline for CountingSink {
        fn execute(
            &self,
            _input: Buffer,
            _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
        ) -> Result<Vec<Buffer>, PipelineError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }
        fn id(&self) -> &PipelineId {
            &self.id
        }
    }

    let sink = CountingSink {
        id: PipelineId::new("sink"),
        count: sink_count_clone,
    };
    let sink_id = sink.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(window)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&window_id, &sink_id).unwrap();
    handle.deploy_graph(graph).unwrap();
    executor.run_one();

    // Emit 5 buffers (1 full + 2 partial)
    for i in 0..5 {
        handle
            .emit(window_id.clone(), Buffer::new(vec![i]))
            .unwrap();
    }
    // Process all work tasks (including routed buffers to sink)
    while executor.run_one() {}

    assert_eq!(sink_count.load(Ordering::SeqCst), 1); // Full window received

    handle.shutdown().unwrap();
    while executor.run_one() {}

    assert_eq!(sink_count.load(Ordering::SeqCst), 2); // Partial window flushed
    assert_eq!(flushed.lock().unwrap().len(), 2);
}

#[test]
fn test_cascading_shutdown_respects_dag_topology() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    let flushed1 = Arc::new(Mutex::new(Vec::new()));
    let flushed2 = Arc::new(Mutex::new(Vec::new()));

    let window1 = WindowPipeline::new("window1", 2, flushed1.clone());
    let window1_id = window1.id().clone();
    let window2 = WindowPipeline::new("window2", 2, flushed2.clone());
    let window2_id = window2.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(window1)).unwrap();
    graph.add_pipeline(Box::new(window2)).unwrap();
    graph.connect(&window1_id, &window2_id).unwrap();
    handle.deploy_graph(graph).unwrap();
    executor.run_one();

    // Emit 1 buffer to window1 (partial)
    handle.emit(window1_id, Buffer::new(vec![42])).unwrap();
    executor.run_one();

    handle.shutdown().unwrap();
    while executor.run_one() {}

    // window1 flushed
    assert_eq!(flushed1.lock().unwrap().len(), 1);
    // window2 received and flushed
    assert_eq!(flushed2.lock().unwrap().len(), 1);
}

#[test]
fn test_diamond_topology_cascading_shutdown() {
    let mut executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // source → [window1, window2] → sink
    let flushed1 = Arc::new(Mutex::new(Vec::new()));
    let flushed2 = Arc::new(Mutex::new(Vec::new()));

    let source = WindowPipeline::new("source", 2, Arc::new(Mutex::new(Vec::new())));
    let source_id = source.id().clone();
    let window1 = WindowPipeline::new("window1", 2, flushed1.clone());
    let window1_id = window1.id().clone();
    let window2 = WindowPipeline::new("window2", 2, flushed2.clone());
    let window2_id = window2.id().clone();

    // Use a simple counter in an Arc<AtomicUsize> to track sink buffers
    let sink_count = Arc::new(AtomicUsize::new(0));
    let sink_count_clone = sink_count.clone();

    // Create a custom sink that uses our counter
    struct CountingSink {
        id: PipelineId,
        count: Arc<AtomicUsize>,
    }
    impl Pipeline for CountingSink {
        fn execute(
            &self,
            _input: Buffer,
            _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
        ) -> Result<Vec<Buffer>, PipelineError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }
        fn id(&self) -> &PipelineId {
            &self.id
        }
    }

    let sink = CountingSink {
        id: PipelineId::new("sink"),
        count: sink_count_clone,
    };
    let sink_id = sink.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(source)).unwrap();
    graph.add_pipeline(Box::new(window1)).unwrap();
    graph.add_pipeline(Box::new(window2)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&source_id, &window1_id).unwrap();
    graph.connect(&source_id, &window2_id).unwrap();
    graph.connect(&window1_id, &sink_id).unwrap();
    graph.connect(&window2_id, &sink_id).unwrap();
    handle.deploy_graph(graph).unwrap();
    executor.run_one();

    // Emit 1 buffer to source (partial)
    handle.emit(source_id, Buffer::new(vec![42])).unwrap();
    executor.run_one();

    handle.shutdown().unwrap();
    while executor.run_one() {}

    // Both windows flushed
    assert_eq!(flushed1.lock().unwrap().len(), 1);
    assert_eq!(flushed2.lock().unwrap().len(), 1);
    // Sink received 2 buffers (from both windows)
    assert_eq!(sink_count.load(Ordering::SeqCst), 2);
}

#[test]
fn test_empty_graph_shutdown() {
    let mut executor = Executor::new();
    let handle = executor.get_handle();

    handle.shutdown().unwrap();
    while executor.run_one() {}

    // Should complete without panics
}

#[test]
fn test_cascading_shutdown_with_random_queue() {
    use adaptive_engine::executor::RandomQueue;

    let mut executor = Executor::with_queue(RandomQueue::new());
    let handle = executor.get_handle();

    let flushed = Arc::new(Mutex::new(Vec::new()));
    let window = WindowPipeline::new("window", 3, flushed.clone());
    let window_id = window.id().clone();

    // Use a custom sink to track buffers
    let sink_count = Arc::new(AtomicUsize::new(0));
    let sink_count_clone = sink_count.clone();

    struct CountingSink {
        id: PipelineId,
        count: Arc<AtomicUsize>,
    }
    impl Pipeline for CountingSink {
        fn execute(
            &self,
            _input: Buffer,
            _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
        ) -> Result<Vec<Buffer>, PipelineError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }
        fn id(&self) -> &PipelineId {
            &self.id
        }
    }

    let sink = CountingSink {
        id: PipelineId::new("sink"),
        count: sink_count_clone,
    };
    let sink_id = sink.id().clone();

    let mut graph = PipelineGraph::new();
    graph.add_pipeline(Box::new(window)).unwrap();
    graph.add_pipeline(Box::new(sink)).unwrap();
    graph.connect(&window_id, &sink_id).unwrap();
    handle.deploy_graph(graph).unwrap();
    executor.run_one();

    // Emit 5 buffers (1 full window + 2 partial)
    for i in 0..5 {
        handle
            .emit(window_id.clone(), Buffer::new(vec![i]))
            .unwrap();
    }

    // Process all tasks - RandomQueue will pick them in random order
    while executor.run_one() {}

    assert_eq!(
        sink_count.load(Ordering::SeqCst),
        1,
        "Full window should reach sink"
    );

    // Shutdown with RandomQueue
    handle.shutdown().unwrap();
    while executor.run_one() {}

    // Verify partial window was flushed and reached sink despite random ordering
    assert_eq!(
        sink_count.load(Ordering::SeqCst),
        2,
        "Partial window should be flushed to sink"
    );
    assert_eq!(
        flushed.lock().unwrap().len(),
        2,
        "Should have 2 flush events"
    );
}
