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

#include <Phases/LowerToCompiledQueryPlanPhase.hpp>

#include <algorithm>
#include <cstdint>
#include <memory>
#include <optional>
#include <ranges>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>
#include <Configuration/WorkerConfiguration.hpp>
#include <Identifiers/Identifiers.hpp>
#include <Pipelines/CompiledExecutablePipelineStage.hpp>
#include <Sources/SourceDescriptor.hpp>
#include <Util/DumpMode.hpp>
#include <Util/ExecutionMode.hpp>
#include <CompiledQueryPlan.hpp>
#include <ErrorHandling.hpp>
#include <Pipeline.hpp>
#include <PipelinedQueryPlan.hpp>
#include <SinkPhysicalOperator.hpp>
#include <SourcePhysicalOperator.hpp>
#include <options.hpp>

namespace NES
{

std::optional<uint64_t> LowerToCompiledQueryPlanPhase::processSuccessor(
    const std::optional<uint64_t>& predecessorStageIndex,
    const std::optional<OperatorId>& sourceOperatorId,
    const std::shared_ptr<Pipeline>& pipeline)
{
    PRECONDITION(pipeline->isSinkPipeline() || pipeline->isOperatorPipeline(), "expected a Sink or OperatorPipeline");

    if (pipeline->isSinkPipeline())
    {
        processSink(predecessorStageIndex, sourceOperatorId, pipeline);
        return std::nullopt;
    }
    return processOperatorPipeline(pipeline);
}

void LowerToCompiledQueryPlanPhase::processSource(const std::shared_ptr<Pipeline>& pipeline)
{
    PRECONDITION(pipeline->isSourcePipeline(), "expected a SourcePipeline {}", *pipeline);

    const auto sourceOperator = pipeline->getRootOperator().get<SourcePhysicalOperator>();

    CompiledQueryPlan::SourceInfo sourceInfo{
        .originId = sourceOperator.getOriginId(),
        .operatorId = sourceOperator.id,
        .descriptor = sourceOperator.getDescriptor(),
        .target_stage_indices = {}};

    for (const auto& successor : pipeline->getSuccessors())
    {
        if (auto stageIndex = processSuccessor(std::nullopt, sourceOperator.id, successor))
        {
            sourceInfo.target_stage_indices.push_back(*stageIndex);
        }
    }

    sources_.push_back(std::move(sourceInfo));
}

void LowerToCompiledQueryPlanPhase::processSink(
    const std::optional<uint64_t>& predecessorStageIndex,
    const std::optional<OperatorId>& sourceOperatorId,
    const std::shared_ptr<Pipeline>& pipeline)
{
    const auto sinkDescriptor = pipeline->getRootOperator().get<SinkPhysicalOperator>().getDescriptor();

    auto it = std::ranges::find(sinks_, pipeline->getPipelineId(), &CompiledQueryPlan::SinkInfo::pipelineId);
    if (it == sinks_.end())
    {
        sinks_.emplace_back(CompiledQueryPlan::SinkInfo{
            .pipelineId = PipelineId(pipeline->getPipelineId()),
            .descriptor = sinkDescriptor,
            .predecessor_stage_indices = {},
            .predecessor_sources = {}});
        it = sinks_.end() - 1;
    }

    if (predecessorStageIndex.has_value())
    {
        it->predecessor_stage_indices.push_back(*predecessorStageIndex);
    }
    if (sourceOperatorId.has_value())
    {
        it->predecessor_sources.push_back(*sourceOperatorId);
    }
}

std::unique_ptr<adaptive_engine::PipelineStage>
LowerToCompiledQueryPlanPhase::getStage(const std::shared_ptr<Pipeline>& pipeline)
{
    nautilus::engine::Options options;
    /// We disable multithreading in MLIR by default to not interfere with NebulaStream's thread model
    options.setOption("mlir.enableMultithreading", false);
    switch (pipelineQueryPlan->getExecutionMode())
    {
        case ExecutionMode::COMPILER: {
            options.setOption("engine.Compilation", true);
            break;
        }
        case ExecutionMode::INTERPRETER: {
            options.setOption("engine.Compilation", false);
            break;
        }
        default: {
            INVARIANT(false, "Invalid backend");
        }
    }
    /// See: https://github.com/nebulastream/nautilus/blob/main/docs/options.md
    switch (dumpQueryCompilationIntermediateRepresentations)
    {
        case DumpMode::NONE:
            options.setOption("dump.all", false);
            options.setOption("dump.console", false);
            options.setOption("dump.file", false);
            break;
        case DumpMode::CONSOLE:
            options.setOption("dump.all", true);
            options.setOption("dump.console", true);
            options.setOption("dump.file", false);
            break;
        case DumpMode::FILE:
            options.setOption("dump.all", true);
            options.setOption("dump.console", false);
            options.setOption("dump.file", true);
            break;
        case DumpMode::FILE_AND_CONSOLE:
            options.setOption("dump.all", true);
            options.setOption("dump.console", true);
            options.setOption("dump.file", true);
            break;
    }

    auto stageId = std::to_string(pipeline->getPipelineId().getRawValue());
    return std::make_unique<CompiledExecutablePipelineStage>(pipeline, pipeline->getOperatorHandlers(), options, stageId);
}

uint64_t LowerToCompiledQueryPlanPhase::processOperatorPipeline(const std::shared_ptr<Pipeline>& pipeline)
{
    /// Check if the particular pipeline already exists in the map
    if (const auto it = pipelineToStageIndex_.find(pipeline->getPipelineId()); it != pipelineToStageIndex_.end())
    {
        return it->second;
    }

    /// Create the stage and get its index
    auto stageIndex = static_cast<uint64_t>(stages_.size());
    stages_.push_back(getStage(pipeline));
    pipelineToStageIndex_.emplace(pipeline->getPipelineId(), stageIndex);

    /// Process successors and create edges
    for (const auto& successor : pipeline->getSuccessors())
    {
        if (auto successorStageIndex = processSuccessor(stageIndex, std::nullopt, successor))
        {
            edges_.push_back(adaptive_engine::Edge{.source_stage = stageIndex, .target_stage = *successorStageIndex});
        }
    }

    return stageIndex;
}

std::unique_ptr<CompiledQueryPlan>
LowerToCompiledQueryPlanPhase::apply(const std::shared_ptr<PipelinedQueryPlan>& pipelineQueryPlan)
{
    this->pipelineQueryPlan = pipelineQueryPlan;

    /// Clear state for this compilation
    stages_.clear();
    edges_.clear();
    sources_.clear();
    sinks_.clear();
    pipelineToStageIndex_.clear();

    /// Process all pipelines recursively starting from sources
    for (auto sourcePipelines = pipelineQueryPlan->getSourcePipelines(); const auto& pipeline : sourcePipelines)
    {
        processSource(pipeline);
    }

    return CompiledQueryPlan::create(
        pipelineQueryPlan->getQueryId(), std::move(stages_), std::move(edges_), std::move(sources_), std::move(sinks_));
}

}  // namespace NES
