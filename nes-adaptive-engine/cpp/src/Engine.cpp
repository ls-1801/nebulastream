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

/// Engine.cpp - C++ implementation calling Rust FFI functions
///
/// This file implements the Engine interface by delegating to Rust
/// FFI functions provided by the adaptive-engine crate.

#include "adaptive_engine/Engine.hpp"

#include <cstdint>
#include <cstring>
#include <memory>

namespace adaptive_engine
{

/// Forward declaration of opaque Rust EngineHandle
struct RustEngineHandle;

/// Rust FFI function declarations
/// These are implemented in src/ffi/engine.rs with #[no_mangle] extern "C"
extern "C" {
/// Create a new engine instance
RustEngineHandle* engine_create_ffi(uintptr_t context_ptr);

/// Create a new engine instance with statistics collection
RustEngineHandle* engine_create_with_stats_ffi(uintptr_t context_ptr, RustStatsQueueHandle** out_stats);

/// Start the engine's worker threads
void engine_start_ffi(const RustEngineHandle* engine);

/// Shutdown the engine
void engine_shutdown_ffi(const RustEngineHandle* engine);

/// Get global engine statistics
/// Returns stats via out parameters to avoid struct ABI issues
void engine_get_stats_raw(
    const RustEngineHandle* engine,
    uint64_t* buffers_processed,
    uint64_t* bytes_processed,
    uint64_t* tasks_executed,
    uint64_t* active_queries,
    double* avg_latency_ms);

/// Submit a query to the engine
uint64_t engine_submit_query_raw(
    const RustEngineHandle* engine,
    const uintptr_t* stage_ptrs,
    size_t num_stages,
    const uint64_t* edge_sources,
    const uint64_t* edge_targets,
    size_t num_edges,
    const uintptr_t* source_ptrs,
    size_t num_sources,
    const uint64_t* source_to_stage_sources,
    const uint64_t* source_to_stage_targets,
    size_t num_source_mappings,
    uintptr_t user_data);

/// Stop a running query
bool engine_stop_query_ffi(const RustEngineHandle* engine, uint64_t query_id);

/// Free the engine handle
void engine_destroy(RustEngineHandle* engine);

/// Poll next statistics event from the queue
bool engine_poll_event_ffi(
    const RustStatsQueueHandle* stats,
    uint64_t timeout_ms,
    uint32_t* out_event_type,
    uint64_t* out_worker_id,
    uint64_t* out_query_id,
    const uint8_t** out_pipeline_id_ptr,
    size_t* out_pipeline_id_len,
    const uint8_t** out_to_pipeline_id_ptr,
    size_t* out_to_pipeline_id_len,
    uint64_t* out_task_id);

/// Free the stats queue handle
void stats_queue_destroy(RustStatsQueueHandle* stats);
}

/// StatsQueue implementation
StatsQueue::~StatsQueue()
{
    if (handle_)
    {
        stats_queue_destroy(handle_);
        handle_ = nullptr;
    }
}

bool StatsQueue::poll(
    uint64_t timeout_ms,
    uint32_t& out_event_type,
    uint64_t& out_worker_id,
    uint64_t& out_query_id,
    std::string& out_pipeline_id,
    std::string& out_to_pipeline_id,
    uint64_t& out_task_id)
{
    if (!handle_)
        return false;

    const uint8_t* pipeline_id_ptr = nullptr;
    size_t pipeline_id_len = 0;
    const uint8_t* to_pipeline_id_ptr = nullptr;
    size_t to_pipeline_id_len = 0;

    bool got_event = engine_poll_event_ffi(
        handle_,
        timeout_ms,
        &out_event_type,
        &out_worker_id,
        &out_query_id,
        &pipeline_id_ptr,
        &pipeline_id_len,
        &to_pipeline_id_ptr,
        &to_pipeline_id_len,
        &out_task_id);

    if (got_event)
    {
        if (pipeline_id_ptr && pipeline_id_len > 0)
        {
            out_pipeline_id.assign(reinterpret_cast<const char*>(pipeline_id_ptr), pipeline_id_len);
        }
        else
        {
            out_pipeline_id.clear();
        }
        if (to_pipeline_id_ptr && to_pipeline_id_len > 0)
        {
            out_to_pipeline_id.assign(reinterpret_cast<const char*>(to_pipeline_id_ptr), to_pipeline_id_len);
        }
        else
        {
            out_to_pipeline_id.clear();
        }
    }
    return got_event;
}

/// Implementation of the Engine interface using Rust FFI
class EngineImpl : public Engine
{
public:
    explicit EngineImpl(RustEngineHandle* handle) : handle_(handle) { }

    ~EngineImpl() override
    {
        if (handle_)
        {
            engine_destroy(handle_);
            handle_ = nullptr;
        }
    }

    /// Non-copyable
    EngineImpl(const EngineImpl&) = delete;
    EngineImpl& operator=(const EngineImpl&) = delete;

    /// Movable
    EngineImpl(EngineImpl&& other) noexcept : handle_(other.handle_) { other.handle_ = nullptr; }

    EngineImpl& operator=(EngineImpl&& other) noexcept
    {
        if (this != &other)
        {
            if (handle_)
            {
                engine_destroy(handle_);
            }
            handle_ = other.handle_;
            other.handle_ = nullptr;
        }
        return *this;
    }

    void start() override
    {
        if (handle_)
        {
            engine_start_ffi(handle_);
        }
    }

    QueryId submit_query(const QueryPlan& plan, void* user_data) override
    {
        if (!handle_)
        {
            return 0;
        }

        /// Extract stage pointers
        std::vector<uintptr_t> stage_ptrs;
        stage_ptrs.reserve(plan.stages.size());
        for (auto* stage : plan.stages)
        {
            stage_ptrs.push_back(reinterpret_cast<uintptr_t>(stage));
        }

        /// Extract edge arrays
        std::vector<uint64_t> edge_sources;
        std::vector<uint64_t> edge_targets;
        edge_sources.reserve(plan.edges.size());
        edge_targets.reserve(plan.edges.size());
        for (const auto& edge : plan.edges)
        {
            edge_sources.push_back(edge.source_stage);
            edge_targets.push_back(edge.target_stage);
        }

        /// Extract source pointers
        std::vector<uintptr_t> source_ptrs;
        source_ptrs.reserve(plan.sources.size());
        for (auto* source : plan.sources)
        {
            source_ptrs.push_back(reinterpret_cast<uintptr_t>(source));
        }

        /// Extract source-to-stage mappings
        std::vector<uint64_t> source_to_stage_sources;
        std::vector<uint64_t> source_to_stage_targets;
        source_to_stage_sources.reserve(plan.source_to_stage.size());
        source_to_stage_targets.reserve(plan.source_to_stage.size());
        for (const auto& mapping : plan.source_to_stage)
        {
            source_to_stage_sources.push_back(mapping.first);
            source_to_stage_targets.push_back(mapping.second);
        }

        return engine_submit_query_raw(
            handle_,
            stage_ptrs.data(),
            stage_ptrs.size(),
            edge_sources.data(),
            edge_targets.data(),
            edge_sources.size(),
            source_ptrs.data(),
            source_ptrs.size(),
            source_to_stage_sources.data(),
            source_to_stage_targets.data(),
            source_to_stage_sources.size(),
            reinterpret_cast<uintptr_t>(user_data));
    }

    bool stop_query(QueryId id) override
    {
        if (!handle_)
        {
            return false;
        }
        return engine_stop_query_ffi(handle_, id);
    }

    void shutdown() override
    {
        if (handle_)
        {
            engine_shutdown_ffi(handle_);
        }
    }

    ExecutionStats get_query_stats(QueryId /*id*/) override
    {
        /// For now, return global stats (per-query stats not yet implemented)
        return get_stats();
    }

    ExecutionStats get_stats() override
    {
        ExecutionStats stats{};
        if (handle_)
        {
            engine_get_stats_raw(
                handle_,
                &stats.buffers_processed,
                &stats.bytes_processed,
                &stats.tasks_executed,
                &stats.active_queries,
                &stats.avg_latency_ms);
        }
        return stats;
    }

private:
    RustEngineHandle* handle_;
};

/// Static factory method implementation
std::unique_ptr<Engine> Engine::create(void* context)
{
    auto* handle = engine_create_ffi(reinterpret_cast<uintptr_t>(context));
    if (!handle)
    {
        return nullptr;
    }
    return std::make_unique<EngineImpl>(handle);
}

/// Static factory method with statistics collection
std::unique_ptr<Engine> Engine::create_with_stats(void* context, std::unique_ptr<StatsQueue>& out_stats_queue)
{
    RustStatsQueueHandle* stats_handle = nullptr;
    auto* handle = engine_create_with_stats_ffi(reinterpret_cast<uintptr_t>(context), &stats_handle);
    if (!handle)
    {
        return nullptr;
    }
    if (stats_handle)
    {
        out_stats_queue = std::make_unique<StatsQueue>(stats_handle);
    }
    return std::make_unique<EngineImpl>(handle);
}

} /// namespace adaptive_engine
