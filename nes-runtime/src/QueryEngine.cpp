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

#include <QueryEngine.hpp>

#include <chrono>
#include <memory>
#include <utility>
#include <vector>
#include <BufferManagement/NesBufferProvider.hpp>
#include <ExecutableQueryPlan.hpp>
#include <Listeners/QueryLog.hpp>
#include <Listeners/StatisticListener.hpp>
#include <QueryEngineStatisticListener.hpp>
#include <Runtime/BufferManager.hpp>
#include <Runtime/Execution/QueryStatus.hpp>
#include <Util/Logger/Logger.hpp>
#include <adaptive_engine/Engine.hpp>

namespace NES
{

QueryEngine::QueryEngine(
    const QueryEngineConfiguration& config,
    std::shared_ptr<StatisticListener> statisticsListener,
    std::shared_ptr<QueryLog> queryLog,
    std::shared_ptr<BufferManager> bufferManager,
    WorkerId workerId)
    : config_(config)
    , statisticsListener_(std::move(statisticsListener))
    , queryLog_(std::move(queryLog))
    , bufferProvider_(std::make_unique<NesBufferProvider>(std::move(bufferManager)))
    , workerId_(workerId)
{
    NES_INFO("Creating QueryEngine with {} worker threads", config_.numWorkerThreads.getValue());

    // Create the adaptive engine with our buffer provider
    engine_ = adaptive_engine::Engine::create(bufferProvider_.get());

    // Start the engine's worker threads
    engine_->start();

    NES_INFO("QueryEngine started successfully");
}

QueryEngine::~QueryEngine()
{
    NES_INFO("Shutting down QueryEngine");

    // Shutdown the engine (this stops all queries and worker threads)
    if (engine_)
    {
        engine_->shutdown();
    }

    NES_INFO("QueryEngine shutdown complete");
}

void QueryEngine::start(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> plan)
{
    NES_INFO("Starting query {}", queryId);

    // Build the adaptive_engine::QueryPlan from the ExecutableQueryPlan
    adaptive_engine::QueryPlan queryPlan;

    // Get the stages from the plan
    const auto& adaptiveStages = plan->getAdaptiveStages();
    queryPlan.stages.reserve(adaptiveStages.size());
    for (const auto& stage : adaptiveStages)
    {
        queryPlan.stages.push_back(stage.get());
    }

    // Get the edges
    queryPlan.edges = plan->getEdges();

    // For now, sources are handled separately through the legacy path
    // TODO(US-033): Migrate sources to use NesSourceHandle and add to queryPlan.sources

    // Submit the query to the engine
    // user_data is nullptr for now - this can be used to pass OperatorHandlers in the future
    adaptive_engine::QueryId engineQueryId = engine_->submit_query(queryPlan, nullptr);

    // Store the mapping and the plan
    runningQueries_.wlock()->emplace(queryId, RunningQuery{engineQueryId, std::move(plan)});

    // Log the query start
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(queryId, QueryState::Running, std::chrono::system_clock::now());
    }

    // Emit statistics event
    if (statisticsListener_)
    {
        statisticsListener_->onEvent(QueryStart(WorkerThreadId(workerId_.getRawValue()), queryId));
    }

    NES_INFO("Query {} started with engine query ID {}", queryId, engineQueryId);
}

void QueryEngine::stop(LocalQueryId queryId)
{
    NES_INFO("Stopping query {}", queryId);

    // Emit stop request event
    if (statisticsListener_)
    {
        statisticsListener_->onEvent(QueryStopRequest(WorkerThreadId(workerId_.getRawValue()), queryId));
    }

    // Find and remove the query
    std::optional<RunningQuery> query;
    {
        auto locked = runningQueries_.wlock();
        auto it = locked->find(queryId);
        if (it != locked->end())
        {
            query = std::move(it->second);
            locked->erase(it);
        }
    }

    if (query)
    {
        // Stop the query in the engine
        bool stopped = engine_->stop_query(query->engineQueryId);

        if (stopped)
        {
            NES_INFO("Query {} stopped successfully", queryId);
        }
        else
        {
            NES_WARNING("Query {} may not have been fully stopped", queryId);
        }

        // Log the query stop
        if (queryLog_)
        {
            queryLog_->logQueryStatusChange(queryId, QueryState::Stopped, std::chrono::system_clock::now());
        }

        // Emit statistics event
        if (statisticsListener_)
        {
            statisticsListener_->onEvent(QueryStop(WorkerThreadId(workerId_.getRawValue()), queryId));
        }
    }
    else
    {
        NES_WARNING("Query {} not found when trying to stop", queryId);
    }
}

}  // namespace NES
