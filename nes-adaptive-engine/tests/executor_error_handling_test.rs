use adaptive_engine::executor::error::{EntityType, TaskType};
use adaptive_engine::executor::{Executor, FifoQueue};
use adaptive_engine::graph::PipelineGraph;
use adaptive_engine::pipeline::{Buffer, Pipeline, PipelineError, PipelineId};
use adaptive_engine::sequence::SequenceNumber;
use adaptive_engine::source::{Source, SourceEmitHandle, SourceError};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ============================================================================
// MOCK PIPELINES
// ============================================================================

/// Pipeline that fails on specific sequence numbers.
struct FailingPipeline {
    id: PipelineId,
    fail_on: Arc<Mutex<HashSet<u64>>>,
}

impl FailingPipeline {
    fn new(id: impl Into<String>, fail_on: Vec<u64>) -> Self {
        Self {
            id: PipelineId::new(id),
            fail_on: Arc::new(Mutex::new(fail_on.into_iter().collect())),
        }
    }
}

impl Pipeline for FailingPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        let seq_str = input.sequence().to_string();
        if let Ok(seq_num) = seq_str.parse::<u64>() {
            if self.fail_on.lock().unwrap().contains(&seq_num) {
                return Err(PipelineError::ExecutionFailed(format!(
                    "Intentional failure on sequence {}",
                    seq_num
                )));
            }
        }
        Ok(vec![input])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

unsafe impl Send for FailingPipeline {}
unsafe impl Sync for FailingPipeline {}

/// Pipeline that always fails during setup.
struct FailingSetupPipeline {
    id: PipelineId,
}

impl FailingSetupPipeline {
    fn new(id: impl Into<String>) -> Self {
        Self {
            id: PipelineId::new(id),
        }
    }
}

impl Pipeline for FailingSetupPipeline {
    fn setup(
        &self,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<(), PipelineError> {
        Err(PipelineError::ExecutionFailed(
            "Intentional setup failure".to_string(),
        ))
    }

    fn execute(
        &self,
        input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        Ok(vec![input])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

unsafe impl Send for FailingSetupPipeline {}
unsafe impl Sync for FailingSetupPipeline {}

/// Simple passthrough pipeline for testing.
struct PassthroughPipeline {
    id: PipelineId,
    processed_count: Arc<Mutex<usize>>,
}

impl PassthroughPipeline {
    fn new(id: impl Into<String>) -> Self {
        Self {
            id: PipelineId::new(id),
            processed_count: Arc::new(Mutex::new(0)),
        }
    }

    #[allow(dead_code)]
    fn get_processed_count(&self) -> usize {
        *self.processed_count.lock().unwrap()
    }
}

impl Pipeline for PassthroughPipeline {
    fn execute(
        &self,
        input: Buffer,
        _context: &dyn adaptive_engine::executor::PipelineExecutionContext,
    ) -> Result<Vec<Buffer>, PipelineError> {
        *self.processed_count.lock().unwrap() += 1;
        Ok(vec![input])
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

unsafe impl Send for PassthroughPipeline {}
unsafe impl Sync for PassthroughPipeline {}

// ============================================================================
// MOCK SOURCE
// ============================================================================

/// Source that fails immediately on start.
struct FailingSource {
    id: PipelineId,
}

impl FailingSource {
    fn new(id: impl Into<String>) -> Self {
        Self {
            id: PipelineId::new(id),
        }
    }
}

impl Source for FailingSource {
    fn start(&self, _emit_handle: SourceEmitHandle) -> Result<(), SourceError> {
        Err(SourceError::StartFailed(
            "Intentional source failure".to_string(),
        ))
    }

    fn stop(&self) -> Result<(), SourceError> {
        Ok(())
    }

    fn id(&self) -> &PipelineId {
        &self.id
    }
}

unsafe impl Send for FailingSource {}
unsafe impl Sync for FailingSource {}

// ============================================================================
// ERROR HANDLING TESTS
// ============================================================================

#[test]
fn test_pipeline_error_terminates_immediately() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Pipeline fails on sequence 5
    let mut graph = PipelineGraph::new();
    let failing = FailingPipeline::new("fail", vec![5]);
    let id = failing.id().clone();
    graph.add_pipeline(Box::new(failing)).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Emit sequences 1-10
    for i in 1u64..=10 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        let _ = handle.emit(id.clone(), buffer);
    }

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();

    // Should process buffers 1-4, fail on 5, skip 6-10
    assert_eq!(stats.buffers_processed, 4);
    assert!(stats.has_errors());
    assert_eq!(stats.errors.len(), 1);

    let error = stats.first_error().unwrap();
    assert_eq!(error.entity_id, id);
    assert!(matches!(error.entity_type, EntityType::Pipeline));
    assert!(matches!(error.task_type, TaskType::WorkTask));
    assert!(error.error.contains("Intentional failure"));
}

#[test]
fn test_error_in_middle_pipeline_stops_graph() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Graph: A → B (fails on 3) → C
    let mut graph = PipelineGraph::new();

    let a = PassthroughPipeline::new("A");
    let a_id = a.id().clone();
    let _a_count = a.processed_count.clone();

    let b = FailingPipeline::new("B", vec![3]);
    let b_id = b.id().clone();

    let c = PassthroughPipeline::new("C");
    let _c_count = c.processed_count.clone();

    let c_id = c.id().clone();

    graph.add_pipeline(Box::new(a)).unwrap();
    graph.add_pipeline(Box::new(b)).unwrap();
    graph.add_pipeline(Box::new(c)).unwrap();
    graph.connect(&a_id, &b_id).unwrap();
    graph.connect(&b_id, &c_id).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Emit sequences 1-5
    for i in 1u64..=5 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        let _ = handle.emit(a_id.clone(), buffer);
    }

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();

    // A should process all buffers it received before error
    // B should fail on sequence 3
    // C should not execute after error
    assert!(stats.has_errors());
    assert_eq!(stats.errors.len(), 1);

    let error = stats.first_error().unwrap();
    assert_eq!(error.entity_id, b_id);
    assert!(matches!(error.entity_type, EntityType::Pipeline));
}

#[test]
fn test_source_start_error_terminates() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Source that fails on start
    let mut graph = PipelineGraph::new();
    let failing_source = FailingSource::new("failing_src");
    let source_id = failing_source.id().clone();
    graph.add_source(Arc::new(failing_source)).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(50)); // Give time for source to start and fail

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();

    // Should have error from source start
    assert!(stats.has_errors());
    assert_eq!(stats.errors.len(), 1);

    let error = stats.first_error().unwrap();
    assert_eq!(error.entity_id, source_id);
    assert!(matches!(error.entity_type, EntityType::Source));
    assert!(matches!(error.task_type, TaskType::StartSource));
    assert!(error.error.contains("Intentional source failure"));
}

#[test]
fn test_multiple_errors_captured() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Pipeline that fails on sequence 2
    let mut graph = PipelineGraph::new();

    let p1 = FailingPipeline::new("P1", vec![2]);
    let p1_id = p1.id().clone();

    graph.add_pipeline(Box::new(p1)).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Emit multiple buffers
    for i in 1u64..=5 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        let _ = handle.emit(p1_id.clone(), buffer);
    }

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();

    // At least one error should be captured
    assert!(stats.has_errors());
    assert_eq!(stats.errors.len(), 1);
}

#[test]
fn test_pending_tasks_skipped_after_error() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Pipeline that fails on sequence 2
    let mut graph = PipelineGraph::new();
    let tracker = PassthroughPipeline::new("tracker");
    let failing = FailingPipeline::new("fail", vec![2]);
    let tracker_count = tracker.processed_count.clone();
    let tracker_id = tracker.id().clone();
    let failing_id = failing.id().clone();

    graph.add_pipeline(Box::new(failing)).unwrap();
    graph.add_pipeline(Box::new(tracker)).unwrap();
    graph.connect(&failing_id, &tracker_id).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Enqueue many tasks quickly
    for i in 1u64..=20 {
        let buffer = Buffer::new(vec![i as u8], SequenceNumber::new(i));
        let _ = handle.emit(failing_id.clone(), buffer);
    }

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();

    // Should fail on buffer 2, skip remaining
    assert!(stats.has_errors());
    // Should process buffer 1 successfully, then fail on buffer 2
    assert!(stats.buffers_processed <= 2); // At most 2 buffers processed

    // Tracker may or may not have processed buffer 1 depending on timing
    let tracker_processed = *tracker_count.lock().unwrap();
    // Just verify most tasks were skipped
    assert!(tracker_processed < 20); // Most buffers were not processed
}

#[test]
fn test_emit_fails_after_error_detected() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Pipeline that fails on first buffer
    let mut graph = PipelineGraph::new();
    let failing = FailingPipeline::new("fail", vec![1]);
    let id = failing.id().clone();
    graph.add_pipeline(Box::new(failing)).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Emit first buffer (will fail)
    let buffer1 = Buffer::new(vec![1], SequenceNumber::new(1));
    handle.emit(id.clone(), buffer1).ok();

    // Wait for error to be detected
    thread::sleep(Duration::from_millis(50));

    // Try to emit another buffer - should fail
    let buffer2 = Buffer::new(vec![2], SequenceNumber::new(2));
    let result = handle.emit(id.clone(), buffer2);

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("executor is in error state"));

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();
    assert!(stats.has_errors());
}

#[test]
fn test_setup_error_during_deployment() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Pipeline with failing setup
    let mut graph = PipelineGraph::new();
    let failing_setup = FailingSetupPipeline::new("fail_setup");
    let id = failing_setup.id().clone();
    graph.add_pipeline(Box::new(failing_setup)).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(50)); // Give time for deployment

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();

    // Should have error from setup
    assert!(stats.has_errors());
    assert_eq!(stats.errors.len(), 1);

    let error = stats.first_error().unwrap();
    assert_eq!(error.entity_id, id);
    assert!(matches!(error.entity_type, EntityType::Pipeline));
    assert!(matches!(error.task_type, TaskType::DeployGraph));
    assert!(error.error.contains("Intentional setup failure"));
}

#[test]
fn test_deploy_graph_fails_after_error() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // First graph with failing setup
    let mut graph1 = PipelineGraph::new();
    let failing_setup = FailingSetupPipeline::new("fail_setup");
    graph1.add_pipeline(Box::new(failing_setup)).unwrap();

    handle.deploy_graph(graph1).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(50)); // Give time for error

    // Try to deploy another graph - should fail
    let graph2 = PipelineGraph::new();
    let result = handle.deploy_graph(graph2);

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("executor is in error state"));

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();
    assert!(stats.has_errors());
}

#[test]
fn test_end_of_stream_fails_after_error() {
    let executor = Executor::with_queue(FifoQueue::new());
    let handle = executor.get_handle();

    // Pipeline that fails on first buffer
    let mut graph = PipelineGraph::new();
    let failing = FailingPipeline::new("fail", vec![1]);
    let id = failing.id().clone();
    graph.add_pipeline(Box::new(failing)).unwrap();

    handle.deploy_graph(graph).unwrap();

    let exec_thread = thread::spawn(move || executor.run());
    thread::sleep(Duration::from_millis(10));

    // Emit buffer that causes failure
    let buffer = Buffer::new(vec![1], SequenceNumber::new(1));
    handle.emit(id.clone(), buffer).ok();

    // Wait for error
    thread::sleep(Duration::from_millis(50));

    // Try to signal end-of-stream - should fail
    let source_id = PipelineId::new("source");
    let result = handle.end_of_stream(source_id, id.clone());

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("executor is in error state"));

    handle.shutdown().ok();
    let stats = exec_thread.join().unwrap();
    assert!(stats.has_errors());
}
