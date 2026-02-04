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

#include <ExecutableQueryPlan.hpp>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <iterator>
#include <memory>
#include <ostream>
#include <unordered_map>
#include <utility>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Runtime/AbstractBufferProvider.hpp>
#include <Sinks/SinkProvider.hpp>
#include <Sources/SourceHandle.hpp>
#include <Sources/SourceProvider.hpp>
#include <BackpressureChannel.hpp>
#include <CompiledQueryPlan.hpp>
#include <ErrorHandling.hpp>
#include <ExecutablePipelineStage.hpp>

namespace NES
{

std::ostream& operator<<(std::ostream& os, const ExecutableQueryPlan& instantiatedQueryPlan)
{
    std::function<void(const std::weak_ptr<ExecutablePipeline>&, size_t)> printNode
        = [&os, &printNode](const std::weak_ptr<ExecutablePipeline>& weakPipeline, size_t indent)
    {
        auto pipeline = weakPipeline.lock();
        if (pipeline && pipeline->stage)
        {
            os << std::string(indent * 4, ' ') << *pipeline->stage << "(" << pipeline->id << ")" << '\n';
            for (const auto& successor : pipeline->successors)
            {
                printNode(successor, indent + 1);
            }
        }
    };

    for (const auto& [source, successors] : instantiatedQueryPlan.sources)
    {
        os << *source << '\n';
        for (const auto& successor : successors)
        {
            printNode(successor, 1);
        }
    }
    return os;
}

std::unique_ptr<ExecutableQueryPlan>
ExecutableQueryPlan::instantiate(CompiledQueryPlan& compiledQueryPlan, const SourceProvider& sourceProvider)
{
    std::vector<ExecutableQueryPlan::SourceWithSuccessor> instantiatedSources;
    std::vector<std::shared_ptr<ExecutablePipeline>> pipelines;

    auto [backpressureController, backpressureListener] = createBackpressureChannel();

    if (compiledQueryPlan.sinks.size() != 1)
    {
        throw NotImplemented("Currently our execution model expects exactly one sink per query plan");
    }

    // Take ownership of the adaptive stages from the compiled plan
    // These implement adaptive_engine::PipelineStage (CompiledExecutablePipelineStage)
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages = std::move(compiledQueryPlan.stages);

    // Take the edges from the compiled plan
    std::vector<adaptive_engine::Edge> edges = std::move(compiledQueryPlan.edges);

    // Create sink using the legacy interface
    auto& sinkInfo = compiledQueryPlan.sinks.front();
    auto sink = ExecutablePipeline::create(
        sinkInfo.pipelineId, lower(std::move(backpressureController), sinkInfo.descriptor), {});
    pipelines.push_back(sink);

    // Build a map from stage index to ExecutablePipeline for linking
    // Note: In the new model, stages are already built. For legacy compatibility, we wrap the adaptive
    // stages in ExecutablePipeline but they don't really "own" the stage (it's owned by ownedAdaptiveStages).
    // The actual execution will use the adaptive stages directly in US-031.

    // Track which sources feed directly to sink (source -> sink case without intermediate stages)
    std::unordered_map<OperatorId, bool> sourceFeedsSink;
    for (const auto& srcId : sinkInfo.predecessor_sources)
    {
        sourceFeedsSink[srcId] = true;
    }

    // Create sources from descriptors
    for (const auto& sourceInfo : compiledQueryPlan.sources)
    {
        std::vector<std::weak_ptr<ExecutablePipeline>> successorPipelines;

        // If source feeds directly to sink, add sink as successor
        if (sourceFeedsSink.contains(sourceInfo.operatorId))
        {
            successorPipelines.push_back(sink);
        }

        // Create the source handle
        auto sourceHandle = sourceProvider.lower(sourceInfo.originId, backpressureListener, sourceInfo.descriptor);

        instantiatedSources.emplace_back(std::move(sourceHandle), std::move(successorPipelines));
    }

    return std::make_unique<ExecutableQueryPlan>(
        compiledQueryPlan.localQueryId,
        std::move(pipelines),
        std::move(instantiatedSources),
        std::move(ownedAdaptiveStages),
        std::move(edges));
}

ExecutableQueryPlan::ExecutableQueryPlan(
    LocalQueryId localQueryId,
    std::vector<std::shared_ptr<ExecutablePipeline>> pipelines,
    std::vector<SourceWithSuccessor> instantiatedSources,
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages,
    std::vector<adaptive_engine::Edge> edges)
    : localQueryId(localQueryId)
    , pipelines(std::move(pipelines))
    , sources(std::move(instantiatedSources))
    , ownedAdaptiveStages_(std::move(ownedAdaptiveStages))
    , edges_(std::move(edges))
{
}
}  // namespace NES
