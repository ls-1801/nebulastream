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

#include <memory>
#include <unordered_map>
#include <Identifiers/Identifiers.hpp>
#include <QueryEngineConfiguration.hpp>
#include <adaptive_engine/Engine.hpp>
#include <folly/Synchronized.h>

namespace NES
{

struct ExecutableQueryPlan;
class BufferManager;
struct QueryLog;
class NesBufferProvider;
struct StatisticListener;

/// The QueryEngine wraps the adaptive_engine::Engine and provides the NES-specific
/// interface for starting, stopping, and managing query execution.
///
/// This class bridges the gap between NES's query management (using LocalQueryId and
/// ExecutableQueryPlan) and the adaptive_engine's execution model (using QueryId and
/// QueryPlan).
class QueryEngine
{
public:
    /// Create a new QueryEngine with the given configuration and listeners.
    /// @param config Configuration options for the engine
    /// @param statisticsListener Listener for query engine events (may be nullptr)
    /// @param queryLog Log for query status changes
    /// @param bufferManager Buffer manager for memory allocation
    /// @param workerThreadId The worker thread ID for statistics events
    QueryEngine(
        const QueryEngineConfiguration& config,
        std::shared_ptr<StatisticListener> statisticsListener,
        std::shared_ptr<QueryLog> queryLog,
        std::shared_ptr<BufferManager> bufferManager,
        WorkerThreadId workerThreadId = WorkerThreadId(0));

    ~QueryEngine();

    QueryEngine(const QueryEngine&) = delete;
    QueryEngine& operator=(const QueryEngine&) = delete;

    /// Start executing a query.
    /// This method takes ownership of the ExecutableQueryPlan and submits it to the
    /// adaptive_engine::Engine for execution.
    /// @param queryId The NES local query ID
    /// @param plan The executable query plan to execute
    void start(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> plan);

    /// Stop a running query.
    /// This method signals the adaptive_engine to stop the query identified by queryId.
    /// The actual termination may happen asynchronously.
    /// @param queryId The NES local query ID to stop
    void stop(LocalQueryId queryId);

private:
    /// Internal representation of a running query
    struct RunningQuery
    {
        adaptive_engine::QueryId engineQueryId;
        std::unique_ptr<ExecutableQueryPlan> plan;
    };

    /// Configuration for the engine
    QueryEngineConfiguration config_;

    /// Statistics listener for events
    std::shared_ptr<StatisticListener> statisticsListener_;

    /// Query log for status changes
    std::shared_ptr<QueryLog> queryLog_;

    /// Buffer provider wrapping the NES buffer manager
    std::unique_ptr<NesBufferProvider> bufferProvider_;

    /// The underlying adaptive execution engine
    std::unique_ptr<adaptive_engine::Engine> engine_;

    /// Worker thread ID for statistics events
    WorkerThreadId workerThreadId_;

    /// Map from NES LocalQueryId to RunningQuery
    folly::Synchronized<std::unordered_map<LocalQueryId, RunningQuery>> runningQueries_;
};

}  // namespace NES
