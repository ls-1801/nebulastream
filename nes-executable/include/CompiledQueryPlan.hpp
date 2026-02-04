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
#include <adaptive_engine/Engine.hpp>

namespace NES
{

/// Compiled query plan that can be instantiated into an executable form.
///
/// This structure holds the compiled pipeline stages (which implement adaptive_engine::PipelineStage)
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

    /// Sink descriptor with information about which stage(s) feed into it
    struct SinkInfo
    {
        PipelineId pipelineId;
        SinkDescriptor descriptor;
        std::vector<uint64_t> predecessor_stage_indices;  ///< Indices into stages vector (empty if from source)
        std::vector<OperatorId> predecessor_sources;       ///< Source operator IDs that feed directly to sink
    };

    /// Create a CompiledQueryPlan
    static std::unique_ptr<CompiledQueryPlan> create(
        LocalQueryId localQueryId,
        std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> stages,
        std::vector<adaptive_engine::Edge> edges,
        std::vector<SourceInfo> sources,
        std::vector<SinkInfo> sinks);

    LocalQueryId localQueryId;

    /// Pipeline stages implementing adaptive_engine::PipelineStage
    std::vector<std::unique_ptr<adaptive_engine::PipelineStage>> stages;

    /// Edges connecting stages (source_stage, target_stage indices)
    std::vector<adaptive_engine::Edge> edges;

    /// Source descriptors with target stage mappings
    std::vector<SourceInfo> sources;

    /// Sink descriptors with predecessor mappings
    std::vector<SinkInfo> sinks;
};

}  // namespace NES
