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

#include <cstddef>
#include <functional>
#include <memory>
#include <ostream>
#include <string>
#include <utility>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Sinks/SinkProvider.hpp>
#include <Sources/SourceHandle.hpp>
#include <Sources/SourceProvider.hpp>
#include <BackpressureChannel.hpp>
#include <CompiledQueryPlan.hpp>
#include <ErrorHandling.hpp>

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

    if (compiledQueryPlan.pending_sinks.size() != 1)
    {
        throw NotImplemented("Currently our execution model expects exactly one sink per query plan");
    }

    // Take ownership of the adaptive stages from the compiled plan.
    // The stages vector includes nullptr placeholders at sink indices.
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages = std::move(compiledQueryPlan.stages);

    // Take the edges from the compiled plan (includes edges to sink stages)
    std::vector<adaptive_engine::Edge> edges = std::move(compiledQueryPlan.edges);

    // Instantiate the sink from its descriptor and fill in the stages vector.
    // Sink inherits from NesPipelineStage (adaptive_engine::PipelineStage), so the
    // unique_ptr<Sink> upcasts to unique_ptr<PipelineStage> via public inheritance.
    auto& pendingSink = compiledQueryPlan.pending_sinks.front();
    auto sink = lower(std::move(backpressureController), pendingSink.descriptor);
    ownedAdaptiveStages[pendingSink.stage_index] = std::move(sink);

    // Create sources from descriptors.
    // Source target_stage_indices already include sink stage indices from the compiler.
    for (const auto& sourceInfo : compiledQueryPlan.sources)
    {
        std::vector<std::weak_ptr<ExecutablePipeline>> successorPipelines;

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
