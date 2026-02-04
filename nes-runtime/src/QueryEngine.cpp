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

#include <atomic>
#include <chrono>
#include <functional>
#include <memory>
#include <thread>
#include <utility>
#include <variant>
#include <vector>
#include <BufferManagement/NesBufferProvider.hpp>
#include <ExecutableQueryPlan.hpp>
#include <ExecutablePipelineStage.hpp>
#include <Listeners/QueryLog.hpp>
#include <Listeners/StatisticListener.hpp>
#include <PipelineExecutionContext.hpp>
#include <QueryEngineStatisticListener.hpp>
#include <Runtime/BufferManager.hpp>
#include <Runtime/Execution/QueryStatus.hpp>
#include <Sources/SourceHandle.hpp>
#include <Sources/SourceReturnType.hpp>
#include <Util/Logger/Logger.hpp>
#include <Util/Overloaded.hpp>
#include <adaptive_engine/Engine.hpp>

namespace NES
{

namespace
{
/// Simple PipelineExecutionContext that routes buffers to successor pipelines.
/// This is a minimal implementation for the legacy execution path.
class LegacyPipelineExecutionContext : public PipelineExecutionContext
{
public:
    LegacyPipelineExecutionContext(
        PipelineId pipelineId,
        WorkerThreadId workerId,
        std::shared_ptr<BufferManager> bufferManager,
        std::vector<std::weak_ptr<ExecutablePipeline>> successors)
        : pipelineId_(pipelineId)
        , workerId_(workerId)
        , bufferManager_(std::move(bufferManager))
        , successors_(std::move(successors))
    {
    }

    bool emitBuffer(const TupleBuffer& buffer, ContinuationPolicy /*policy*/) override
    {
        // Route buffer to all successor pipelines
        for (const auto& weakSuccessor : successors_)
        {
            if (auto successor = weakSuccessor.lock())
            {
                // Create a context for the successor pipeline
                LegacyPipelineExecutionContext successorContext(
                    successor->id, workerId_, bufferManager_, successor->successors);

                // Execute the successor with the buffer
                successor->stage->execute(buffer, successorContext);
            }
        }
        return true;
    }

    void repeatTask(const TupleBuffer& /*buffer*/, std::chrono::milliseconds /*delay*/) override
    {
        // Not implemented for legacy path - just a no-op
        NES_WARNING("repeatTask not implemented in legacy execution path");
    }

    TupleBuffer allocateTupleBuffer() override
    {
        auto buffer = bufferManager_->getBufferBlocking();
        return buffer;
    }

    [[nodiscard]] WorkerThreadId getId() const override { return workerId_; }

    [[nodiscard]] uint64_t getNumberOfWorkerThreads() const override { return 1; }

    [[nodiscard]] std::shared_ptr<AbstractBufferProvider> getBufferManager() const override { return bufferManager_; }

    [[nodiscard]] PipelineId getPipelineId() const override { return pipelineId_; }

    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& getOperatorHandlers() override
    {
        return operatorHandlers_;
    }

    void setOperatorHandlers(std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& handlers) override
    {
        operatorHandlers_ = handlers;
    }

private:
    PipelineId pipelineId_;
    WorkerThreadId workerId_;
    std::shared_ptr<BufferManager> bufferManager_;
    std::vector<std::weak_ptr<ExecutablePipeline>> successors_;
    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>> operatorHandlers_;
};
}  // namespace

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

    // Stop all running queries
    auto queries = runningQueries_.wlock();
    for (auto& [queryId, query] : *queries)
    {
        if (query.isLegacy && query.plan)
        {
            // Stop sources for legacy queries
            for (auto& [source, successors] : query.plan->sources)
            {
                source->stop();
            }

            // Stop pipelines in reverse order (sinks first)
            for (auto it = query.plan->pipelines.rbegin(); it != query.plan->pipelines.rend(); ++it)
            {
                if (*it && (*it)->stage)
                {
                    LegacyPipelineExecutionContext ctx((*it)->id, workerThreadId_, bufferManager_, (*it)->successors);
                    (*it)->stage->stop(ctx);
                }
            }
        }
    }
    queries->clear();

    // Shutdown the adaptive engine
    if (engine_)
    {
        engine_->shutdown();
    }

    // Wait for any cleanup threads to complete
    {
        std::lock_guard<std::mutex> lock(cleanupThreadsMutex_);
        for (auto& thread : cleanupThreads_)
        {
            if (thread.joinable())
            {
                thread.join();
            }
        }
        cleanupThreads_.clear();
    }

    NES_INFO("QueryEngine shutdown complete");
}

void QueryEngine::start(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> plan)
{
    NES_INFO("Starting query {}", queryId);

    // Check if this is a legacy query (no adaptive stages)
    if (plan->getAdaptiveStages().empty())
    {
        startLegacy(queryId, std::move(plan));
    }
    else
    {
        startAdaptive(queryId, std::move(plan));
    }
}

void QueryEngine::startLegacy(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> plan)
{
    NES_INFO("Starting query {} using legacy execution path", queryId);

    // Emit statistics event - query start
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStart(workerThreadId_, queryId));
    }

    // Start all pipeline stages
    for (const auto& pipeline : plan->pipelines)
    {
        if (pipeline && pipeline->stage)
        {
            LegacyPipelineExecutionContext ctx(pipeline->id, workerThreadId_, bufferManager_, pipeline->successors);
            pipeline->stage->start(ctx);

            // Emit pipeline start event
            if (statisticsListener_)
            {
                static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())
                    ->onEvent(PipelineStart(workerThreadId_, queryId, pipeline->id));
            }
        }
    }

    const size_t numSources = plan->sources.size();

    // Store query before starting sources (sources might complete immediately)
    {
        auto queries = runningQueries_.wlock();
        queries->emplace(
            queryId, RunningQuery{.engineQueryId = 0, .plan = std::move(plan), .isLegacy = true, .sourcesFinished = 0, .totalSources = numSources});
    }

    // Log that the query has started (pipelines initialized, before sources produce data)
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(queryId, QueryState::Started, std::chrono::system_clock::now());
    }

    // Start each source with an emit function that routes buffers to successor pipelines
    auto queries = runningQueries_.wlock();
    auto& query = queries->at(queryId);

    // Task ID counter for statistics (simple incrementing counter per query)
    static std::atomic<uint64_t> taskIdCounter{0};

    for (auto& [source, successors] : query.plan->sources)
    {
        const OriginId sourceId = source->getSourceId();

        // Create emit function that processes buffers through the legacy pipeline
        SourceReturnType::EmitFunction emitFn = [this, queryId, sourceId, successors = successors, workerId = workerThreadId_](
                                                    OriginId /*originId*/,
                                                    SourceReturnType::SourceReturnType result,
                                                    const std::stop_token& /*stopToken*/) -> SourceReturnType::EmitResult
        {
            return std::visit(
                Overloaded{
                    [&](const SourceReturnType::Data& data) -> SourceReturnType::EmitResult
                    {
                        const TaskId taskId = TaskId(taskIdCounter.fetch_add(1));
                        const uint64_t numberOfTuples = data.buffer.getNumberOfTuples();
                        // Use pipeline ID 0 for source-level task execution
                        const PipelineId sourcePipelineId = PipelineId(0);

                        // Emit task execution start event
                        if (statisticsListener_)
                        {
                            static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())
                                ->onEvent(TaskExecutionStart(workerId, queryId, sourcePipelineId, taskId, numberOfTuples));
                        }

                        // Route buffer to all successor pipelines
                        for (const auto& weakSuccessor : successors)
                        {
                            if (auto successor = weakSuccessor.lock())
                            {
                                LegacyPipelineExecutionContext ctx(
                                    successor->id, workerId, bufferManager_, successor->successors);

                                // Emit task emit event before execution
                                if (statisticsListener_)
                                {
                                    static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())
                                        ->onEvent(TaskEmit(workerId, queryId, sourcePipelineId, successor->id, taskId, numberOfTuples));
                                }

                                successor->stage->execute(data.buffer, ctx);
                            }
                        }

                        // Emit task execution complete event
                        if (statisticsListener_)
                        {
                            static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())
                                ->onEvent(TaskExecutionComplete(workerId, queryId, sourcePipelineId, taskId));
                        }

                        return SourceReturnType::EmitResult::SUCCESS;
                    },
                    [&](const SourceReturnType::EoS&) -> SourceReturnType::EmitResult
                    {
                        NES_DEBUG("Source {} reached end of stream for query {}", sourceId, queryId);
                        handleSourceTermination(queryId, sourceId, QueryTerminationType::Graceful);
                        return SourceReturnType::EmitResult::SUCCESS;
                    },
                    [&](const SourceReturnType::Error& error) -> SourceReturnType::EmitResult
                    {
                        NES_ERROR("Source {} failed for query {}: {}", sourceId, queryId, error.ex.what());
                        handleSourceTermination(queryId, sourceId, QueryTerminationType::Failure);
                        return SourceReturnType::EmitResult::STOP_REQUESTED;
                    }},
                result);
        };

        // Start the source
        if (!source->start(std::move(emitFn)))
        {
            NES_ERROR("Failed to start source {} for query {}", sourceId, queryId);
        }
    }

    // Log that the query is now running (sources are active)
    if (queryLog_)
    {
        queryLog_->logQueryStatusChange(queryId, QueryState::Running, std::chrono::system_clock::now());
    }

    NES_INFO("Query {} started with {} sources using legacy execution path", queryId, numSources);
}

void QueryEngine::startAdaptive(LocalQueryId queryId, std::unique_ptr<ExecutableQueryPlan> plan)
{
    NES_INFO("Starting query {} using adaptive execution path", queryId);

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

    // Submit the query to the engine
    adaptive_engine::QueryId engineQueryId = engine_->submit_query(queryPlan, nullptr);

    // Store the mapping and the plan
    runningQueries_.wlock()->emplace(
        queryId, RunningQuery{.engineQueryId = engineQueryId, .plan = std::move(plan), .isLegacy = false, .sourcesFinished = 0, .totalSources = 0});

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

void QueryEngine::handleSourceTermination(LocalQueryId queryId, OriginId sourceId, QueryTerminationType type)
{
    NES_DEBUG("Handling source {} termination ({}) for query {}", sourceId, static_cast<int>(type), queryId);

    // Log the source termination
    if (queryLog_)
    {
        queryLog_->logSourceTermination(queryId, sourceId, type, std::chrono::system_clock::now());
    }

    bool allSourcesFinished = false;
    std::unique_ptr<ExecutableQueryPlan> planToCleanup;

    {
        auto queries = runningQueries_.wlock();
        auto it = queries->find(queryId);
        if (it == queries->end())
        {
            NES_WARNING("Query {} not found when handling source termination", queryId);
            return;
        }

        it->second.sourcesFinished++;
        allSourcesFinished = (it->second.sourcesFinished >= it->second.totalSources);

        if (allSourcesFinished)
        {
            planToCleanup = std::move(it->second.plan);
            queries->erase(it);
        }
    }

    if (allSourcesFinished && planToCleanup)
    {
        NES_INFO("All sources finished for query {}, stopping pipelines", queryId);

        // Run cleanup in a separate thread to avoid deadlock.
        // This is called from within the source thread's emit callback, and stopping
        // pipelines/sources could try to join the source thread, causing a deadlock.
        std::thread cleanupThread(
            [this, queryId, plan = std::move(planToCleanup)]() mutable
            {
                // Stop pipelines in reverse order (sinks first)
                for (auto it = plan->pipelines.rbegin(); it != plan->pipelines.rend(); ++it)
                {
                    if (*it && (*it)->stage)
                    {
                        LegacyPipelineExecutionContext ctx((*it)->id, workerThreadId_, bufferManager_, (*it)->successors);
                        (*it)->stage->stop(ctx);

                        // Emit pipeline stop event
                        if (statisticsListener_)
                        {
                            static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())
                                ->onEvent(PipelineStop(workerThreadId_, queryId, (*it)->id));
                        }
                    }
                }

                // Emit query stop event
                if (statisticsListener_)
                {
                    static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStop(workerThreadId_, queryId));
                }

                // Log the query stop
                if (queryLog_)
                {
                    queryLog_->logQueryStatusChange(queryId, QueryState::Stopped, std::chrono::system_clock::now());
                }

                // Plan is destroyed when lambda exits, cleaning up all resources
            });

        // Store the thread for later joining in destructor
        {
            std::lock_guard<std::mutex> lock(cleanupThreadsMutex_);
            cleanupThreads_.push_back(std::move(cleanupThread));
        }
    }
}

void QueryEngine::stop(LocalQueryId queryId)
{
    NES_INFO("Stopping query {}", queryId);

    // Emit stop request event
    if (statisticsListener_)
    {
        static_cast<QueryEngineStatisticListener*>(statisticsListener_.get())->onEvent(QueryStopRequest(workerThreadId_, queryId));
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

    if (!query)
    {
        NES_WARNING("Query {} not found when trying to stop", queryId);
        return;
    }

    if (query->isLegacy)
    {
        stopLegacy(queryId);
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

void QueryEngine::stopLegacy(LocalQueryId queryId)
{
    NES_INFO("Stopping legacy query {}", queryId);

    // Query was already removed from map in stop()
    // The sources should have been stopped already or will stop on their own

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
