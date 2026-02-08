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

#pragma once

#include "Buffer.hpp"
#include "ExecutionContext.hpp"
#include "PipelineStage.hpp"
#include "SourceHandle.hpp"

#include <cstdint>
#include <memory>
#include <vector>

namespace adaptive_engine
{

/// Unique identifier for a submitted query
using QueryId = uint64_t;

/// Edge in the query DAG, connecting source stage to target stage
struct Edge
{
    uint64_t source_stage; ///< Index of the source stage
    uint64_t target_stage; ///< Index of the target stage
};

/// Query plan describing the DAG of stages and sources
struct QueryPlan
{
    std::vector<PipelineStage*> stages; ///< Pipeline stages
    std::vector<Edge> edges; ///< Edges connecting stages
    std::vector<SourceHandle*> sources; ///< Data sources
    std::vector<std::pair<uint64_t, uint64_t>> source_to_stage; ///< (source_idx, stage_idx) mappings
};

/// Execution statistics for queries
struct ExecutionStats
{
    uint64_t buffers_processed; ///< Total buffers processed
    uint64_t bytes_processed; ///< Total bytes processed
    uint64_t tasks_executed; ///< Total tasks executed
    uint64_t active_queries; ///< Currently running queries
    double avg_latency_ms; ///< Average processing latency in milliseconds
};

/// Abstract interface for the adaptive execution engine
///
/// The engine manages query execution across a pool of workers.
/// Queries are submitted as DAGs of stages and sources.
/// Opaque handle to the Rust statistics queue (forward declaration)
struct RustStatsQueueHandle;

/// Handle for polling statistics events from the Rust engine.
///
/// Owns the Rust-side mpsc::Receiver<StatisticsEvent> and provides
/// a polling interface for C++ test code to consume events.
class StatsQueue
{
public:
    explicit StatsQueue(RustStatsQueueHandle* handle) : handle_(handle) { }

    ~StatsQueue();

    StatsQueue(const StatsQueue&) = delete;
    StatsQueue& operator=(const StatsQueue&) = delete;

    StatsQueue(StatsQueue&& other) noexcept : handle_(other.handle_) { other.handle_ = nullptr; }

    StatsQueue& operator=(StatsQueue&&) = delete;

    /// Poll the next event from the queue.
    /// @param timeout_ms 0 for non-blocking, >0 for timed wait
    /// @param out_event_type Event type (see FfiStatisticsEventType enum values)
    /// @param out_worker_id Worker ID
    /// @param out_query_id Query ID
    /// @param out_pipeline_id Pipeline ID string (copied)
    /// @param out_to_pipeline_id To-pipeline ID string for TaskEmit (copied)
    /// @param out_task_id Task ID
    /// @return true if an event was received, false if timeout/empty
    bool poll(
        uint64_t timeout_ms,
        uint32_t& out_event_type,
        uint64_t& out_worker_id,
        uint64_t& out_query_id,
        std::string& out_pipeline_id,
        std::string& out_to_pipeline_id,
        uint64_t& out_task_id);

    RustStatsQueueHandle* handle() const { return handle_; }

private:
    RustStatsQueueHandle* handle_;
};

class Engine
{
public:
    virtual ~Engine() = default;

    /// Create an engine instance with the given opaque context
    /// @param context Opaque pointer controlled by the caller (e.g., NesBufferProvider*)
    /// @return Unique pointer to the created engine
    static std::unique_ptr<Engine> create(void* context);

    /// Create an engine instance with statistics collection enabled
    /// @param context Opaque pointer controlled by the caller (e.g., NesBufferProvider*)
    /// @param out_stats_queue Receives the stats queue for polling events
    /// @return Unique pointer to the created engine
    static std::unique_ptr<Engine> create_with_stats(void* context, std::unique_ptr<StatsQueue>& out_stats_queue);

    /// Create an engine instance with worker threads and statistics collection
    /// @param context Opaque pointer controlled by the caller (e.g., NesBufferProvider*)
    /// @param num_workers Number of worker threads
    /// @param out_stats_queue Receives the stats queue for polling events
    /// @return Unique pointer to the created engine
    static std::unique_ptr<Engine>
    create_with_workers_and_stats(void* context, size_t num_workers, std::unique_ptr<StatsQueue>& out_stats_queue);

    /// Start the engine's worker threads
    virtual void start() = 0;

    /// Submit a query for execution
    /// @param plan The query plan describing stages and edges
    /// @param user_data Opaque pointer passed to stages during execution
    /// @return Query identifier for tracking and control
    virtual QueryId submit_query(const QueryPlan& plan, void* user_data) = 0;

    /// Stop a running query
    /// @param id The query to stop
    /// @return True if the query was successfully stopped
    virtual bool stop_query(QueryId id) = 0;

    /// Shutdown the engine, stopping all queries and workers
    virtual void shutdown() = 0;

    /// Get statistics for a specific query
    /// @param id The query to get stats for
    /// @return Execution statistics for the query
    virtual ExecutionStats get_query_stats(QueryId id) = 0;

    /// Get global engine statistics
    /// @return Aggregated execution statistics
    virtual ExecutionStats get_stats() = 0;
};

} /// namespace adaptive_engine
