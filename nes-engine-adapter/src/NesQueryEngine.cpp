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

#include <Execution/NesQueryEngine.hpp>

#include <atomic>
#include <memory>
#include <thread>
#include <utility>
#include <adaptive_engine/Engine.hpp>
#include "NesBufferProvider.hpp"
#include "PipelineStageAdapter.hpp"
#include "SourceHandleAdapter.hpp"

namespace NES
{

namespace
{
/// Event type values from FfiStatisticsEventType (Rust enum).
constexpr uint32_t FFI_EVENT_QUERY_TERMINATED = 9;
} /// namespace

class NesQueryEngine::Impl
{
public:
    explicit Impl(std::shared_ptr<AbstractBufferProvider> bufferProvider, size_t numWorkerThreads)
        : bufferProvider_(std::make_unique<NesBufferProvider>(std::move(bufferProvider)))
    {
        engine_ = adaptive_engine::Engine::create_with_workers_and_stats(
            static_cast<void*>(bufferProvider_.get()), numWorkerThreads, statsQueue_);
    }

    ~Impl()
    {
        stopPolling_.store(true);
        if (pollingThread_.joinable())
        {
            pollingThread_.join();
        }
        if (engine_)
        {
            engine_->shutdown();
        }
    }

    void start()
    {
        engine_->start();

        if (statsQueue_)
        {
            stopPolling_.store(false);
            pollingThread_ = std::thread(
                [this]()
                {
                    while (!stopPolling_.load())
                    {
                        uint32_t eventType = 0;
                        uint64_t workerId = 0;
                        uint64_t queryId = 0;
                        std::string pipelineId;
                        std::string toPipelineId;
                        uint64_t taskId = 0;

                        bool got = statsQueue_->poll(50, eventType, workerId, queryId, pipelineId, toPipelineId, taskId);
                        if (!got)
                        {
                            continue;
                        }

                        if (eventType == FFI_EVENT_QUERY_TERMINATED && callback_)
                        {
                            callback_(queryId);
                        }
                    }
                });
        }
    }

    void shutdown()
    {
        stopPolling_.store(true);
        if (pollingThread_.joinable())
        {
            pollingThread_.join();
        }
        if (engine_)
        {
            engine_->shutdown();
        }
    }

    void setQueryTerminatedCallback(QueryTerminatedCallback callback) { callback_ = std::move(callback); }

    QueryId submitQuery(NesQueryPlan&& plan)
    {
        adaptive_engine::QueryPlan enginePlan;

        /// Wrap each NesPipelineStage in a PipelineStageAdapter
        std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedStages;
        ownedStages.reserve(plan.stages.size());
        for (auto& stage : plan.stages)
        {
            ownedStages.push_back(std::make_unique<PipelineStageAdapter>(std::move(stage)));
        }

        enginePlan.stages.reserve(ownedStages.size());
        for (const auto& stage : ownedStages)
        {
            enginePlan.stages.push_back(stage.get());
        }

        /// Convert NesEdge -> adaptive_engine::Edge
        enginePlan.edges.reserve(plan.edges.size());
        for (const auto& edge : plan.edges)
        {
            enginePlan.edges.push_back(adaptive_engine::Edge{.source_stage = edge.sourceStage, .target_stage = edge.targetStage});
        }

        /// Wrap each NesSourceAdapter in a SourceHandleAdapter
        std::vector<std::unique_ptr<adaptive_engine::SourceHandle>> ownedSources;
        ownedSources.reserve(plan.sources.size());
        for (auto& source : plan.sources)
        {
            ownedSources.push_back(std::make_unique<SourceHandleAdapter>(std::move(source)));
        }

        enginePlan.sources.reserve(ownedSources.size());
        for (const auto& source : ownedSources)
        {
            enginePlan.sources.push_back(source.get());
        }

        /// Copy source-to-stage mappings
        enginePlan.source_to_stage = plan.sourceToStage;

        /// Submit to the engine
        auto queryId = engine_->submit_query(enginePlan, nullptr);

        /// Release ownership to the Rust engine (it destroys via stage_destroy/source_destroy)
        for (auto& stage : ownedStages)
        {
            stage.release();
        }
        for (auto& source : ownedSources)
        {
            source.release();
        }

        return queryId;
    }

    bool stopQuery(QueryId id) { return engine_->stop_query(id); }

private:
    std::unique_ptr<NesBufferProvider> bufferProvider_;
    std::unique_ptr<adaptive_engine::Engine> engine_;
    std::unique_ptr<adaptive_engine::StatsQueue> statsQueue_;
    QueryTerminatedCallback callback_;
    std::thread pollingThread_;
    std::atomic<bool> stopPolling_{false};
};

NesQueryEngine::NesQueryEngine() = default;

NesQueryEngine::~NesQueryEngine() = default;

std::unique_ptr<NesQueryEngine> NesQueryEngine::create(std::shared_ptr<AbstractBufferProvider> bufferProvider, size_t numWorkerThreads)
{
    auto engine = std::unique_ptr<NesQueryEngine>(new NesQueryEngine());
    engine->impl_ = std::make_unique<Impl>(std::move(bufferProvider), numWorkerThreads);
    return engine;
}

void NesQueryEngine::start()
{
    impl_->start();
}

void NesQueryEngine::shutdown()
{
    impl_->shutdown();
}

void NesQueryEngine::setQueryTerminatedCallback(QueryTerminatedCallback callback)
{
    impl_->setQueryTerminatedCallback(std::move(callback));
}

NesQueryEngine::QueryId NesQueryEngine::submitQuery(NesQueryPlan&& plan)
{
    return impl_->submitQuery(std::move(plan));
}

bool NesQueryEngine::stopQuery(QueryId id)
{
    return impl_->stopQuery(id);
}

} /// namespace NES
