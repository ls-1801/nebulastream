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
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Sinks/SinkDescriptor.hpp>
#include <Sources/SourceDescriptor.hpp>
#include <Execution/NesQueryPlan.hpp>
#include <Execution/NesPipelineStage.hpp>

namespace NES
{

/// Compiled query plan that can be instantiated into an executable form.
///
/// This structure holds the compiled pipeline stages (which implement NesPipelineStage)
/// along with edge definitions and source/sink descriptors. The descriptors are abstract and get
/// instantiated into concrete sources and sinks during query instantiation.
struct CompiledQueryPlan
{
    /// Source descriptor with information about which stage(s) receive its buffers
    struct SourceInfo
    {
        OriginId originId;
        OperatorId operatorId;
        SourceDescriptor descriptor;
        std::vector<uint64_t> target_stage_indices;  ///< Indices into stages vector
    };

    /// Sink descriptor with its reserved stage index in the stages vector.
    /// The stages vector has a nullptr at stage_index; the actual sink is instantiated
    /// from the descriptor during query instantiation (ExecutableQueryPlan::instantiate).
    struct PendingSink
    {
        uint64_t stage_index;  ///< Index in stages vector (nullptr placeholder)
        PipelineId pipelineId;
        SinkDescriptor descriptor;
    };

    /// Create a CompiledQueryPlan
    static std::unique_ptr<CompiledQueryPlan> create(
        LocalQueryId localQueryId,
        std::vector<std::unique_ptr<NesPipelineStage>> stages,
        std::vector<NesEdge> edges,
        std::vector<SourceInfo> sources,
        std::vector<PendingSink> pending_sinks);

    LocalQueryId localQueryId;

    /// Pipeline stages implementing NesPipelineStage
    std::vector<std::unique_ptr<NesPipelineStage>> stages;

    /// Edges connecting stages (sourceStage, targetStage indices)
    std::vector<NesEdge> edges;

    /// Source descriptors with target stage mappings
    std::vector<SourceInfo> sources;

    /// Sink descriptors with reserved stage indices.
    /// The stages vector has nullptr at each pending_sink's stage_index.
    std::vector<PendingSink> pending_sinks;
};

}  // namespace NES
