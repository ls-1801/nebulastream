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

#include <cstdint>
#include <memory>
#include <unordered_map>
#include <vector>

#include <Execution/NesPipelineStage.hpp>
#include <Execution/NesQueryPlan.hpp>
#include <Identifiers/Identifiers.hpp>
#include <Util/DumpMode.hpp>
#include <CompiledQueryPlan.hpp>
#include <PipelinedQueryPlan.hpp>

namespace NES
{
class LowerToCompiledQueryPlanPhase
{
public:
    explicit LowerToCompiledQueryPlanPhase(DumpMode dumpQueryCompilationIntermediateRepresentations)
        : dumpQueryCompilationIntermediateRepresentations(dumpQueryCompilationIntermediateRepresentations)
    {
    }

    std::unique_ptr<CompiledQueryPlan> apply(const std::shared_ptr<PipelinedQueryPlan>& pipelineQueryPlan);

private:
    /// Process successors recursively, creating stages and tracking edges.
    /// Always returns the stage index of the created/found stage.
    uint64_t processSuccessor(const std::shared_ptr<Pipeline>& pipeline);

    /// Process a source pipeline
    void processSource(const std::shared_ptr<Pipeline>& pipeline);

    /// Process a sink pipeline, reserving a stage index for later instantiation
    uint64_t processSink(const std::shared_ptr<Pipeline>& pipeline);

    /// Process an operator pipeline, creating a stage
    uint64_t processOperatorPipeline(const std::shared_ptr<Pipeline>& pipeline);

    /// Create a pipeline stage from a Pipeline
    std::unique_ptr<NesPipelineStage> getStage(const std::shared_ptr<Pipeline>& pipeline);

    /// Lowering context - populated during apply()
    std::vector<std::unique_ptr<NesPipelineStage>> stages_;
    std::vector<NesEdge> edges_;
    std::vector<CompiledQueryPlan::SourceInfo> sources_;
    std::vector<CompiledQueryPlan::PendingSink> pending_sinks_;

    /// Map from PipelineId to stage index for deduplication
    std::unordered_map<PipelineId, uint64_t> pipelineToStageIndex_;

    std::shared_ptr<PipelinedQueryPlan> pipelineQueryPlan;

    /// Config parameter
    DumpMode dumpQueryCompilationIntermediateRepresentations;
};
} /// namespace NES
