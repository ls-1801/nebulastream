#pragma once

#include <adaptive_engine/Engine.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <variant>
#include <vector>

namespace adaptive_engine::test {

/// Type aliases for statistics event fields.
/// Uses uint64_t directly for test simplicity (no NES type dependencies).
using WorkerId = uint64_t;
using QueryId = uint64_t;
using PipelineId = std::string;
using TaskId = uint64_t;

/// FFI event type tags matching Rust FfiStatisticsEventType.
enum class FfiEventType : uint32_t {
    None = 0,
    QueryStart = 1,
    QueryStop = 2,
    PipelineStart = 3,
    PipelineStop = 4,
    TaskExecutionStart = 5,
    TaskExecutionComplete = 6,
    TaskEmit = 7,
    QueryRunning = 8,
    QueryTerminated = 9,
};

// Forward declarations of event types
struct QueryStartEvent;
struct QueryStopEvent;
struct PipelineStartEvent;
struct PipelineStopEvent;
struct TaskExecutionStartEvent;
struct TaskExecutionCompleteEvent;
struct TaskEmitEvent;
struct QueryRunningEvent;
struct QueryTerminatedEvent;

/// Variant type containing all possible statistics events.
using StatisticsEvent = std::variant<
    QueryStartEvent,
    QueryStopEvent,
    PipelineStartEvent,
    PipelineStopEvent,
    TaskExecutionStartEvent,
    TaskExecutionCompleteEvent,
    TaskEmitEvent,
    QueryRunningEvent,
    QueryTerminatedEvent>;

/// Event emitted when a query starts executing (graph deployed).
struct QueryStartEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};

    bool operator==(const QueryStartEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id;
    }
};

/// Event emitted when a query stops (all pipelines stopped).
struct QueryStopEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};

    bool operator==(const QueryStopEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id;
    }
};

/// Event emitted when a pipeline starts (setup succeeds).
struct PipelineStartEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};
    PipelineId pipeline_id;

    bool operator==(const PipelineStartEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id &&
               pipeline_id == other.pipeline_id;
    }
};

/// Event emitted when a pipeline stops (teardown called).
struct PipelineStopEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};
    PipelineId pipeline_id;

    bool operator==(const PipelineStopEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id &&
               pipeline_id == other.pipeline_id;
    }
};

/// Event emitted when a task starts executing.
struct TaskExecutionStartEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};
    PipelineId pipeline_id;
    TaskId task_id{0};

    bool operator==(const TaskExecutionStartEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id &&
               pipeline_id == other.pipeline_id && task_id == other.task_id;
    }
};

/// Event emitted when a task completes execution.
struct TaskExecutionCompleteEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};
    PipelineId pipeline_id;
    TaskId task_id{0};

    bool operator==(const TaskExecutionCompleteEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id &&
               pipeline_id == other.pipeline_id && task_id == other.task_id;
    }
};

/// Event emitted when a task emits data to downstream pipelines.
struct TaskEmitEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};
    PipelineId from_pipeline_id;
    PipelineId to_pipeline_id;
    TaskId task_id{0};

    bool operator==(const TaskEmitEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id &&
               from_pipeline_id == other.from_pipeline_id &&
               to_pipeline_id == other.to_pipeline_id && task_id == other.task_id;
    }
};

/// Event emitted when a query becomes fully operational.
struct QueryRunningEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};

    bool operator==(const QueryRunningEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id;
    }
};

/// Event emitted when a query has fully terminated.
struct QueryTerminatedEvent {
    WorkerId worker_id{0};
    QueryId query_id{0};

    bool operator==(const QueryTerminatedEvent& other) const {
        return worker_id == other.worker_id && query_id == other.query_id;
    }
};

/// Statistics listener for collecting execution events in tests.
///
/// This class provides:
/// - Thread-safe collection of statistics events
/// - Assertion helpers for verifying expected event counts
/// - Wait methods for synchronizing with async execution
/// - Background polling thread for consuming events from StatsQueue
///
/// Usage:
/// ```cpp
/// auto listener = std::make_shared<StatisticsListener>();
///
/// // ... run queries ...
///
/// // Verify events
/// ASSERT_TRUE(listener->expect_query_start(2));   // Expect 2 queries started
/// ASSERT_TRUE(listener->expect_pipeline_stop(4)); // Expect 4 pipelines stopped
///
/// // Get all events for detailed inspection
/// auto events = listener->take_events();
/// ```
class StatisticsListener {
public:
    static constexpr std::chrono::milliseconds DEFAULT_TIMEOUT{10000};

    StatisticsListener() = default;

    ~StatisticsListener() {
        stop_polling();
    }

    StatisticsListener(const StatisticsListener&) = delete;
    StatisticsListener& operator=(const StatisticsListener&) = delete;
    StatisticsListener(StatisticsListener&&) = delete;
    StatisticsListener& operator=(StatisticsListener&&) = delete;

    /// Start a background polling thread that reads events from the StatsQueue
    /// and records them into this listener.
    void start_polling(adaptive_engine::StatsQueue* stats_queue) {
        stop_polling_.store(false);
        poll_thread_ = std::thread([this, stats_queue]() {
            while (!stop_polling_.load()) {
                uint32_t event_type = 0;
                uint64_t worker_id = 0;
                uint64_t query_id = 0;
                std::string pipeline_id;
                std::string to_pipeline_id;
                uint64_t task_id = 0;

                // Poll with 50ms timeout to allow checking stop flag
                bool got = stats_queue->poll(50, event_type, worker_id, query_id,
                                             pipeline_id, to_pipeline_id, task_id);
                if (!got) continue;

                auto type = static_cast<FfiEventType>(event_type);
                switch (type) {
                    case FfiEventType::QueryStart:
                        record(QueryStartEvent{worker_id, query_id});
                        break;
                    case FfiEventType::QueryStop:
                        record(QueryStopEvent{worker_id, query_id});
                        break;
                    case FfiEventType::PipelineStart:
                        record(PipelineStartEvent{worker_id, query_id, pipeline_id});
                        break;
                    case FfiEventType::PipelineStop:
                        record(PipelineStopEvent{worker_id, query_id, pipeline_id});
                        break;
                    case FfiEventType::TaskExecutionStart:
                        record(TaskExecutionStartEvent{worker_id, query_id, pipeline_id, task_id});
                        break;
                    case FfiEventType::TaskExecutionComplete:
                        record(TaskExecutionCompleteEvent{worker_id, query_id, pipeline_id, task_id});
                        break;
                    case FfiEventType::TaskEmit:
                        record(TaskEmitEvent{worker_id, query_id, pipeline_id, to_pipeline_id, task_id});
                        break;
                    case FfiEventType::QueryRunning:
                        record(QueryRunningEvent{worker_id, query_id});
                        break;
                    case FfiEventType::QueryTerminated:
                        record(QueryTerminatedEvent{worker_id, query_id});
                        break;
                    default:
                        break;
                }
            }
        });
    }

    /// Stop the background polling thread.
    void stop_polling() {
        stop_polling_.store(true);
        if (poll_thread_.joinable()) {
            poll_thread_.join();
        }
    }

    // Event recording (called by the engine/test harness)

    /// Record a statistics event.
    /// Thread-safe: can be called from any thread.
    void record(StatisticsEvent event) {
        std::lock_guard<std::mutex> lock(mutex_);
        events_.push_back(std::move(event));
        event_cv_.notify_all();
    }

    /// Record a QueryStart event.
    void record_query_start(WorkerId worker_id, QueryId query_id) {
        record(QueryStartEvent{worker_id, query_id});
    }

    /// Record a QueryStop event.
    void record_query_stop(WorkerId worker_id, QueryId query_id) {
        record(QueryStopEvent{worker_id, query_id});
    }

    /// Record a PipelineStart event.
    void record_pipeline_start(WorkerId worker_id, QueryId query_id,
                               const PipelineId& pipeline_id) {
        record(PipelineStartEvent{worker_id, query_id, pipeline_id});
    }

    /// Record a PipelineStop event.
    void record_pipeline_stop(WorkerId worker_id, QueryId query_id,
                              const PipelineId& pipeline_id) {
        record(PipelineStopEvent{worker_id, query_id, pipeline_id});
    }

    /// Record a TaskExecutionStart event.
    void record_task_execution_start(WorkerId worker_id, QueryId query_id,
                                     const PipelineId& pipeline_id, TaskId task_id) {
        record(TaskExecutionStartEvent{worker_id, query_id, pipeline_id, task_id});
    }

    /// Record a TaskExecutionComplete event.
    void record_task_execution_complete(WorkerId worker_id, QueryId query_id,
                                        const PipelineId& pipeline_id, TaskId task_id) {
        record(TaskExecutionCompleteEvent{worker_id, query_id, pipeline_id, task_id});
    }

    /// Record a TaskEmit event.
    void record_task_emit(WorkerId worker_id, QueryId query_id,
                          const PipelineId& from_pipeline_id,
                          const PipelineId& to_pipeline_id, TaskId task_id) {
        record(TaskEmitEvent{worker_id, query_id, from_pipeline_id, to_pipeline_id, task_id});
    }

    /// Record a QueryRunning event.
    void record_query_running(WorkerId worker_id, QueryId query_id) {
        record(QueryRunningEvent{worker_id, query_id});
    }

    /// Record a QueryTerminated event.
    void record_query_terminated(WorkerId worker_id, QueryId query_id) {
        record(QueryTerminatedEvent{worker_id, query_id});
    }

    // Assertion helpers

    /// Expect exactly n QueryStart events within timeout.
    [[nodiscard]] bool expect_query_start(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<QueryStartEvent>(n, timeout);
    }

    /// Expect exactly n QueryStop events within timeout.
    [[nodiscard]] bool expect_query_stop(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<QueryStopEvent>(n, timeout);
    }

    /// Expect exactly n PipelineStart events within timeout.
    [[nodiscard]] bool expect_pipeline_start(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<PipelineStartEvent>(n, timeout);
    }

    /// Expect exactly n PipelineStop events within timeout.
    [[nodiscard]] bool expect_pipeline_stop(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<PipelineStopEvent>(n, timeout);
    }

    /// Expect exactly n TaskExecutionStart events within timeout.
    [[nodiscard]] bool expect_task_execution_start(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<TaskExecutionStartEvent>(n, timeout);
    }

    /// Expect exactly n TaskExecutionComplete events within timeout.
    [[nodiscard]] bool expect_task_execution_complete(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<TaskExecutionCompleteEvent>(n, timeout);
    }

    /// Expect exactly n TaskEmit events within timeout.
    [[nodiscard]] bool expect_task_emit(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<TaskEmitEvent>(n, timeout);
    }

    /// Expect exactly n QueryRunning events within timeout.
    [[nodiscard]] bool expect_query_running(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<QueryRunningEvent>(n, timeout);
    }

    /// Expect exactly n QueryTerminated events within timeout.
    [[nodiscard]] bool expect_query_terminated(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_count<QueryTerminatedEvent>(n, timeout);
    }

    /// Wait for at least n events of type within timeout (no exact match).
    template <typename EventType>
    [[nodiscard]] bool wait_for_at_least(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        std::unique_lock<std::mutex> lock(mutex_);
        return event_cv_.wait_for(lock, timeout, [this, n] {
            return count_type_unlocked<EventType>() >= n;
        });
    }

    /// Expect count in range [min_n, max_n] within timeout.
    template <typename EventType>
    [[nodiscard]] bool expect_count_in_range(
        size_t min_n, size_t max_n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        std::unique_lock<std::mutex> lock(mutex_);
        // Wait until we have at least min_n
        bool ok = event_cv_.wait_for(lock, timeout, [this, min_n] {
            return count_type_unlocked<EventType>() >= min_n;
        });
        if (!ok) return false;
        // Give a small window for more events to arrive
        event_cv_.wait_for(lock, std::chrono::milliseconds(100), [this, max_n] {
            return count_type_unlocked<EventType>() > max_n;
        });
        auto c = count_type_unlocked<EventType>();
        return c >= min_n && c <= max_n;
    }

    // Wait helpers for query lifecycle (similar to NES waitForQepRunning/waitForQepTermination)

    /// Wait for a QueryRunning event for the specified query.
    [[nodiscard]] bool wait_for_query_running(
        adaptive_engine::QueryId query_id,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        std::unique_lock<std::mutex> lock(mutex_);
        return event_cv_.wait_for(lock, timeout, [this, query_id] {
            for (const auto& event : events_) {
                if (std::holds_alternative<QueryRunningEvent>(event)) {
                    if (std::get<QueryRunningEvent>(event).query_id == query_id) {
                        return true;
                    }
                }
            }
            return false;
        });
    }

    /// Wait for a QueryTerminated event for the specified query.
    [[nodiscard]] bool wait_for_query_terminated(
        adaptive_engine::QueryId query_id,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        std::unique_lock<std::mutex> lock(mutex_);
        return event_cv_.wait_for(lock, timeout, [this, query_id] {
            for (const auto& event : events_) {
                if (std::holds_alternative<QueryTerminatedEvent>(event)) {
                    if (std::get<QueryTerminatedEvent>(event).query_id == query_id) {
                        return true;
                    }
                }
            }
            return false;
        });
    }

    // Event access

    /// Take all recorded events, clearing the internal list.
    std::vector<StatisticsEvent> take_events() {
        std::lock_guard<std::mutex> lock(mutex_);
        std::vector<StatisticsEvent> result = std::move(events_);
        events_.clear();
        return result;
    }

    /// Get a copy of all recorded events (non-destructive).
    [[nodiscard]] std::vector<StatisticsEvent> get_events() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return events_;
    }

    /// Get the total number of events recorded.
    [[nodiscard]] size_t event_count() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return events_.size();
    }

    /// Count events of a specific type.
    template <typename EventType>
    [[nodiscard]] size_t count() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return count_type_unlocked<EventType>();
    }

    /// Get all events of a specific type.
    template <typename EventType>
    [[nodiscard]] std::vector<EventType> get_events_of_type() const {
        std::lock_guard<std::mutex> lock(mutex_);
        std::vector<EventType> result;
        for (const auto& event : events_) {
            if (std::holds_alternative<EventType>(event)) {
                result.push_back(std::get<EventType>(event));
            }
        }
        return result;
    }

    // Synchronization

    /// Wait until at least n total events have been recorded.
    [[nodiscard]] bool wait_for_events(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        std::unique_lock<std::mutex> lock(mutex_);
        return event_cv_.wait_for(lock, timeout, [this, n] {
            return events_.size() >= n;
        });
    }

    /// Clear all recorded events.
    void clear() {
        std::lock_guard<std::mutex> lock(mutex_);
        events_.clear();
    }

private:
    template <typename EventType>
    [[nodiscard]] size_t count_type_unlocked() const {
        size_t result = 0;
        for (const auto& event : events_) {
            if (std::holds_alternative<EventType>(event)) {
                ++result;
            }
        }
        return result;
    }

    template <typename EventType>
    [[nodiscard]] bool wait_for_count(
        size_t n,
        std::chrono::milliseconds timeout) const {
        std::unique_lock<std::mutex> lock(mutex_);
        return event_cv_.wait_for(lock, timeout, [this, n] {
            return count_type_unlocked<EventType>() >= n;
        }) && count_type_unlocked<EventType>() == n;
    }

    mutable std::mutex mutex_;
    mutable std::condition_variable event_cv_;
    std::vector<StatisticsEvent> events_;

    // Polling thread state
    std::atomic<bool> stop_polling_{false};
    std::thread poll_thread_;
};

}  // namespace adaptive_engine::test
