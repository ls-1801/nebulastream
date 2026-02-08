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
#include <Listeners/QueryLog.hpp>
#include <Listeners/StatisticListener.hpp>
#include <Runtime/BufferManager.hpp>
#include <Runtime/Execution/QueryStatus.hpp>
#include <Util/Logger/Logger.hpp>
#include <ExecutableQueryPlan.hpp>
#include <QueryEngineStatisticListener.hpp>

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
    , workerThreadId_(workerThreadId)
{
    NES_INFO("Creating QueryEngine with {} worker threads", config_.numWorkerThreads.getValue());

    engine_ = NesQueryEngine::create(bufferManager, config_.numWorkerThreads.getValue());
    engine_->setQueryTerminatedCallback([this](NesQueryEngine::QueryId engineQueryId) { handleQueryTerminated(engineQueryId); });
    engine_->start();

    NES_INFO("QueryEngine started successfully");
}

QueryEngine::~QueryEngine()
{
    NES_INFO("Shutting down QueryEngine");

    if (engine_)
    {
        engine_->shutdown();
    }

    runningQueries_.wlock()->clear();

    NES_INFO("QueryEngine shutdown complete");
}

void QueryEngine::start(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> plan)
{
    NES_INFO("Starting query {}", queryId);

    /// Build a NesQueryPlan from the ExecutableQueryPlan
    NesQueryPlan queryPlan;

    /// Move stages from the plan
    queryPlan.stages = plan->takeStages();

    /// Copy edges
    queryPlan.edges = plan->getEdges();

    /// Move sources (NesSourceHandle IS-A NesSourceAdapter)
    queryPlan.sources.reserve(plan->sources.size());
    for (auto& source : plan->sources)
    {
        queryPlan.sources.push_back(std::move(source));
    }

    /// Copy source-to-stage mappings
    queryPlan.sourceToStage = plan->getSourceToStage();

    /// Submit the query to the engine
    NesQueryEngine::QueryId engineQueryId = engine_->submitQuery(std::move(queryPlan));

    /// Store the mapping and the plan
    runningQueries_.wlock()->emplace(queryId, RunningQuery{.engineQueryId = engineQueryId, .nesQueryId = queryId, .plan = std::move(plan)});

    /// Log the query start
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(queryId, QueryState::Running, std::chrono::system_clock::now());
    }

    /// Emit statistics event
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStart(workerThreadId_, queryId));
    }

    NES_INFO("Query {} started with engine query ID {}", queryId, engineQueryId);
}

void QueryEngine::stop(LocalQueryId queryId)
{
    NES_INFO("Stopping query {}", queryId);

    /// Emit stop request event
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStopRequest(workerThreadId_, queryId));
    }

    /// Find and remove the query from the running map
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

    /// Stop the query in the engine
    bool stopped = engine_->stopQuery(query->engineQueryId);

    if (stopped)
    {
        NES_INFO("Query {} stopped successfully", queryId);
    }
    else
    {
        NES_WARNING("Query {} may not have been fully stopped", queryId);
    }

    /// Log the query stop
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(queryId, QueryState::Stopped, std::chrono::system_clock::now());
    }

    /// Emit statistics event
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStop(workerThreadId_, queryId));
    }
}

void QueryEngine::handleQueryTerminated(NesQueryEngine::QueryId engineQueryId)
{
    /// Find the NES query ID corresponding to this engine query ID
    std::optional<LocalQueryId> nesQueryId;
    {
        auto locked = runningQueries_.wlock();
        for (auto it = locked->begin(); it != locked->end(); ++it)
        {
            if (it->second.engineQueryId == engineQueryId)
            {
                nesQueryId = it->first;
                locked->erase(it);
                break;
            }
        }
    }

    if (!nesQueryId)
    {
        NES_DEBUG("QueryTerminated for unknown engine query {} (may have been stopped already)", engineQueryId);
        return;
    }

    NES_INFO("Query {} terminated naturally (engine query {})", *nesQueryId, engineQueryId);

    /// Log the query stop in the QueryLog (this is what the systest polls)
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(*nesQueryId, QueryState::Stopped, std::chrono::system_clock::now());
    }

    /// Emit NES statistics events
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStop(workerThreadId_, *nesQueryId));
    }
}

} /// namespace NES
