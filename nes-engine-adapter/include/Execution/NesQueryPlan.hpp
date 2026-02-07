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
#include <utility>
#include <vector>

namespace NES
{

class NesPipelineStage;
class NesSourceAdapter;

/// Edge in the query DAG, connecting source stage to target stage by index.
struct NesEdge
{
    uint64_t sourceStage;
    uint64_t targetStage;
};

/// Query plan describing the DAG of stages and sources, using NES-owned types.
struct NesQueryPlan
{
    std::vector<std::unique_ptr<NesPipelineStage>> stages;
    std::vector<NesEdge> edges;
    std::vector<std::unique_ptr<NesSourceAdapter>> sources;
    std::vector<std::pair<uint64_t, uint64_t>> sourceToStage;
};

}  // namespace NES
