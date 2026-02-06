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

#include <memory>
#include <ostream>
#include <utility>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Sinks/SinkProvider.hpp>
#include <NesSourceHandle.hpp>
#include <Sources/SourceProvider.hpp>
#include <BackpressureChannel.hpp>
#include <CompiledQueryPlan.hpp>
#include <ErrorHandling.hpp>

namespace NES
{

std::ostream& operator<<(std::ostream& os, const ExecutableQueryPlan& instantiatedQueryPlan)
{
    os << "ExecutableQueryPlan(id=" << instantiatedQueryPlan.localQueryId
       << ", sources=" << instantiatedQueryPlan.sources.size()
       << ", stages=" << instantiatedQueryPlan.getAdaptiveStages().size() << ")";
    return os;
}

std::unique_ptr<ExecutableQueryPlan>
ExecutableQueryPlan::instantiate(CompiledQueryPlan& compiledQueryPlan, const SourceProvider& sourceProvider)
{
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

    // Create adaptive-engine source handles from descriptors.
    // These implement adaptive_engine::SourceHandle (pull model: open/next_buffer/close)
    // and are driven by the engine's internal source threads.
    std::vector<std::unique_ptr<NesSourceHandle>> instantiatedSources;
    for (const auto& sourceInfo : compiledQueryPlan.sources)
    {
        auto nesSourceHandle = sourceProvider.lowerAdaptive(sourceInfo.originId, sourceInfo.descriptor);
        instantiatedSources.emplace_back(std::move(nesSourceHandle));
    }

    // Build source-to-stage mappings from the compiled plan's target_stage_indices.
    std::vector<std::pair<uint64_t, uint64_t>> source_to_stage;
    for (size_t srcIdx = 0; srcIdx < compiledQueryPlan.sources.size(); ++srcIdx)
    {
        for (uint64_t stageIdx : compiledQueryPlan.sources[srcIdx].target_stage_indices)
        {
            source_to_stage.emplace_back(srcIdx, stageIdx);
        }
    }

    return std::make_unique<ExecutableQueryPlan>(
        compiledQueryPlan.localQueryId,
        std::move(instantiatedSources),
        std::move(ownedAdaptiveStages),
        std::move(edges),
        std::move(source_to_stage));
}

ExecutableQueryPlan::ExecutableQueryPlan(
    LocalQueryId localQueryId,
    std::vector<std::unique_ptr<NesSourceHandle>> instantiatedSources,
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages,
    std::vector<adaptive_engine::Edge> edges,
    std::vector<std::pair<uint64_t, uint64_t>> source_to_stage)
    : localQueryId(localQueryId)
    , sources(std::move(instantiatedSources))
    , ownedAdaptiveStages_(std::move(ownedAdaptiveStages))
    , edges_(std::move(edges))
    , source_to_stage_(std::move(source_to_stage))
{
}
}  // namespace NES
