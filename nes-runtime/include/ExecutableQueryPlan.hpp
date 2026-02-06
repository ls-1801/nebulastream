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
#include <Sources/SourceHandle.hpp>

namespace NES
{

class SourceProvider;

/// The ExecutableQueryPlan represents a query with completely instantiated query processing components.
/// It holds the adaptive_engine stages and source handles ready for submission to the engine.
struct ExecutableQueryPlan
{
    /// Instantiate a compiled query plan into an executable form.
    /// This creates concrete sources and sinks from descriptors and links the pipeline DAG.
    /// @param compiledQueryPlan The compiled query plan with stages and edges
    /// @param sourceProvider Provider for creating source instances
    /// @return ExecutableQueryPlan ready for execution
    static std::unique_ptr<ExecutableQueryPlan>
    instantiate(CompiledQueryPlan& compiledQueryPlan, const SourceProvider& sourceProvider);

    ExecutableQueryPlan(
        LocalQueryId localQueryId,
        std::vector<std::unique_ptr<SourceHandle>> instantiatedSources,
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
    std::vector<std::unique_ptr<SourceHandle>> sources;

    friend std::ostream& operator<<(std::ostream& os, const ExecutableQueryPlan& executableQueryPlan);

private:
    /// Ownership of adaptive_engine::PipelineStage instances
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> ownedAdaptiveStages_;

    /// Edge definitions for the stage DAG
    std::vector<adaptive_engine::Edge> edges_;
};
}  // namespace NES

FMT_OSTREAM(NES::ExecutableQueryPlan);
