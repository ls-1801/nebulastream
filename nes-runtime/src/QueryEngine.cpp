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
    WorkerThreadId workerThreadId)
    : config_(config)
    , statisticsListener_(std::move(statisticsListener))
    , queryLog_(std::move(queryLog))
    , bufferManager_(bufferManager)
    , bufferProvider_(std::make_unique<NesBufferProvider>(bufferManager))
    , workerThreadId_(workerThreadId)
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

    // Shutdown the adaptive engine (stops all queries and worker threads).
    // This destroys stages/sources owned by the engine via source_destroy/stage_destroy.
    if (engine_)
    {
        engine_->shutdown();
    }

    // Clear running queries (plans no longer own stages/sources after releaseOwnership)
    runningQueries_.wlock()->clear();

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

    // Get the sources (NesSourceHandle implements adaptive_engine::SourceHandle)
    queryPlan.sources.reserve(plan->sources.size());
    for (const auto& source : plan->sources)
    {
        queryPlan.sources.push_back(source.get());
    }

    // Get the source-to-stage mappings
    queryPlan.source_to_stage = plan->getSourceToStage();

    // Submit the query to the engine
    adaptive_engine::QueryId engineQueryId = engine_->submit_query(queryPlan, nullptr);

    // Release ownership of stages and sources - the Rust engine now owns them
    // and will destroy them via source_destroy/stage_destroy on shutdown/stop.
    plan->releaseOwnership();

    // Store the mapping and the plan
    runningQueries_.wlock()->emplace(
        queryId, RunningQuery{.engineQueryId = engineQueryId, .plan = std::move(plan)});

    // Log the query start
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(queryId, QueryState::Running, std::chrono::system_clock::now());
    }

    // Emit statistics event
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStart(workerThreadId_, queryId));
    }

    NES_INFO("Query {} started with engine query ID {}", queryId, engineQueryId);
}

void QueryEngine::stop(LocalQueryId queryId)
{
    NES_INFO("Stopping query {}", queryId);

    // Emit stop request event
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStopRequest(workerThreadId_, queryId));
    }

    // Find and remove the query from the running map
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

    if (!query)
    {
        NES_WARNING("Query {} not found when trying to stop", queryId);
        return;
    }

    // Stop the query in the adaptive engine
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
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStop(workerThreadId_, queryId));
    }
}

}  // namespace NES
