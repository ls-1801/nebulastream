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
#include <memory>
#include <ostream>
#include <utility>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Util/Logger/Formatter.hpp>
#include <adaptive_engine/Engine.hpp>
#include <CompiledQueryPlan.hpp>

namespace NES
{

class SourceProvider;
class SourceHandle;
class ExecutablePipelineStage;

/// Internal structure for tracking instantiated pipelines with their successors.
/// Used during the transition period while we migrate from the old execution model.
struct ExecutablePipeline
{
    PipelineId id;
    std::unique_ptr<ExecutablePipelineStage> stage;
    std::vector<std::weak_ptr<ExecutablePipeline>> successors;

    static std::shared_ptr<ExecutablePipeline> create(
        PipelineId id,
        std::unique_ptr<ExecutablePipelineStage> stage,
        std::vector<std::weak_ptr<ExecutablePipeline>> successors)
    {
        auto pipeline = std::make_shared<ExecutablePipeline>();
        pipeline->id = id;
        pipeline->stage = std::move(stage);
        pipeline->successors = std::move(successors);
        return pipeline;
    }
};

/// The ExecutableQueryPlan represents a query with completely instantiated query processing components.
/// It holds both the legacy ExecutablePipeline structures and the compiled adaptive_engine stages.
///
/// TODO(US-031): Migrate to fully use adaptive_engine::QueryPlan for submission to the engine.
struct ExecutableQueryPlan
{
    using SourceWithSuccessor = std::pair<std::unique_ptr<SourceHandle>, std::vector<std::weak_ptr<ExecutablePipeline>>>;

    /// Instantiate a compiled query plan into an executable form.
    /// This creates concrete sources and sinks from descriptors and links the pipeline DAG.
    /// @param compiledQueryPlan The compiled query plan with stages and edges
    /// @param sourceProvider Provider for creating source instances
    /// @return ExecutableQueryPlan ready for execution
    static std::unique_ptr<ExecutableQueryPlan>
    instantiate(CompiledQueryPlan& compiledQueryPlan, const SourceProvider& sourceProvider);

    ExecutableQueryPlan(
        LocalQueryId localQueryId,
        std::vector<std::shared_ptr<ExecutablePipeline>> pipelines,
        std::vector<SourceWithSuccessor> instantiatedSources,
        std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages,
        std::vector<adaptive_engine::Edge> edges);

    /// Get the adaptive engine stages (owned by this plan).
    /// These implement adaptive_engine::PipelineStage and can be used with adaptive_engine::QueryPlan.
    [[nodiscard]] const std::vector<std::unique_ptr<adaptive_engine::PipelineStage>>& getAdaptiveStages() const
    {
        return ownedAdaptiveStages_;
    }

    /// Get the edge definitions for the stage DAG.
    [[nodiscard]] const std::vector<adaptive_engine::Edge>& getEdges() const { return edges_; }

    LocalQueryId localQueryId;
    std::vector<std::shared_ptr<ExecutablePipeline>> pipelines;
    std::vector<SourceWithSuccessor> sources;

    friend std::ostream& operator<<(std::ostream& os, const ExecutableQueryPlan& executableQueryPlan);

private:
    /// Ownership of adaptive_engine::PipelineStage instances (CompiledExecutablePipelineStage)
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages_;

    /// Edge definitions for the stage DAG
    std::vector<adaptive_engine::Edge> edges_;
};
}  // namespace NES

FMT_OSTREAM(NES::ExecutableQueryPlan);
