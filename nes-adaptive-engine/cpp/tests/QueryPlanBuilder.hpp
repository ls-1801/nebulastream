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

#include <adaptive_engine/Engine.hpp>
#include "TestBuffer.hpp"
#include "TestPipeline.hpp"
#include "TestSink.hpp"
#include "TestSource.hpp"

#include <cstdint>
#include <memory>
#include <stdexcept>
#include <string>
#include <unordered_map>
#include <variant>
#include <vector>

namespace adaptive_engine::test
{

/// Strong type wrapper for unique identifiers in QueryPlanBuilder
template <typename Tag>
struct StrongId
{
    uint64_t value{0};

    StrongId() = default;

    explicit StrongId(uint64_t v) : value(v) { }

    bool operator==(const StrongId& other) const { return value == other.value; }

    bool operator!=(const StrongId& other) const { return value != other.value; }

    /// For use in unordered_map
    struct Hash
    {
        std::size_t operator()(const StrongId& id) const { return std::hash<uint64_t>{}(id.value); }
    };

    /// Pre-increment operator for ID generation
    StrongId& operator++()
    {
        ++value;
        return *this;
    }

    StrongId operator++(int)
    {
        StrongId tmp = *this;
        ++value;
        return tmp;
    }
};

/// Tag types for distinct strong IDs
struct SourceIdTag
{
};

struct PipelineIdTag
{
};

struct SinkIdTag
{
};

/// Unique identifier for sources in QueryPlanBuilder (internal builder ID)
using BuilderSourceId = StrongId<SourceIdTag>;

/// Unique identifier for pipelines in QueryPlanBuilder (internal builder ID)
using BuilderPipelineId = StrongId<PipelineIdTag>;

/// Unique identifier for sinks in QueryPlanBuilder (internal builder ID)
using BuilderSinkId = StrongId<SinkIdTag>;

/// Controller variant for type-safe storage of different controller types
using ControllerVariant
    = std::variant<std::shared_ptr<TestSourceController>, std::shared_ptr<TestPipelineController>, std::shared_ptr<TestSinkController>>;

/// Result of building a query plan.
/// Contains the QueryPlan and maps of IDs to controllers for test assertions.
struct QueryPlanBuildResult
{
    /// Release ownership of stages and sources without deleting them.
    /// Ownership transfers to the Rust engine when submit_query() is called.
    /// The engine destroys objects via source_destroy/stage_destroy on shutdown.
    ~QueryPlanBuildResult()
    {
        for (auto& s : owned_stages)
        {
            s.release();
        }
        for (auto& s : owned_sources)
        {
            s.release();
        }
    }

    /// Move-only (non-copyable due to unique_ptrs)
    QueryPlanBuildResult() = default;
    QueryPlanBuildResult(QueryPlanBuildResult&&) = default;
    QueryPlanBuildResult& operator=(QueryPlanBuildResult&&) = default;
    QueryPlanBuildResult(const QueryPlanBuildResult&) = delete;
    QueryPlanBuildResult& operator=(const QueryPlanBuildResult&) = delete;

    /// The built query plan (stages, edges, sources, source_to_stage mappings)
    QueryPlan plan;

    /// Owned stages (released to engine on destruction, NOT deleted)
    std::vector<std::unique_ptr<PipelineStage>> owned_stages;

    /// Owned sources (released to engine on destruction, NOT deleted)
    std::vector<std::unique_ptr<SourceHandle>> owned_sources;

    /// Map of BuilderSourceId to TestSourceController
    std::unordered_map<BuilderSourceId, std::shared_ptr<TestSourceController>, BuilderSourceId::Hash> source_controllers;

    /// Map of BuilderPipelineId to TestPipelineController
    std::unordered_map<BuilderPipelineId, std::shared_ptr<TestPipelineController>, BuilderPipelineId::Hash> pipeline_controllers;

    /// Map of BuilderSinkId to TestSinkController
    std::unordered_map<BuilderSinkId, std::shared_ptr<TestSinkController>, BuilderSinkId::Hash> sink_controllers;

    /// Get a source controller by ID
    /// @param id The source ID
    /// @return Shared pointer to the TestSourceController
    /// @throws std::out_of_range if ID not found
    std::shared_ptr<TestSourceController> get_source_controller(BuilderSourceId id) const { return source_controllers.at(id); }

    /// Get a pipeline controller by ID
    /// @param id The pipeline ID
    /// @return Shared pointer to the TestPipelineController
    /// @throws std::out_of_range if ID not found
    std::shared_ptr<TestPipelineController> get_pipeline_controller(BuilderPipelineId id) const { return pipeline_controllers.at(id); }

    /// Get a sink controller by ID
    /// @param id The sink ID
    /// @return Shared pointer to the TestSinkController
    /// @throws std::out_of_range if ID not found
    std::shared_ptr<TestSinkController> get_sink_controller(BuilderSinkId id) const { return sink_controllers.at(id); }
};

/// Fluent builder for constructing test query plans.
///
/// Usage:
/// ```cpp
/// TestBufferProvider provider;
/// QueryPlanBuilder builder(&provider);
///
/// /// Build a simple source -> pipeline -> sink topology
/// auto source_id = builder.add_source();
/// auto pipeline_id = builder.add_pipeline({source_id});
/// auto sink_id = builder.add_sink({pipeline_id});
///
/// auto result = builder.build();
///
/// /// Access controllers for test assertions
/// auto source_ctrl = result.get_source_controller(source_id);
/// auto sink_ctrl = result.get_sink_controller(sink_id);
///
/// /// Submit the query
/// engine->submit_query(result.plan, nullptr);
///
/// /// Inject data and verify
/// source_ctrl->inject_data({1, 2, 3, 4}, 1);
/// source_ctrl->inject_eos();
/// ASSERT_TRUE(sink_ctrl->wait_for_buffers(1));
/// ```
class QueryPlanBuilder
{
public:
    /// Create a builder with the given buffer provider.
    /// @param provider Buffer provider for creating test sources and sinks
    explicit QueryPlanBuilder(test::TestBufferProvider* provider) : provider_(provider) { }

    ~QueryPlanBuilder() = default;

    QueryPlanBuilder(const QueryPlanBuilder&) = delete;
    QueryPlanBuilder& operator=(const QueryPlanBuilder&) = delete;
    QueryPlanBuilder(QueryPlanBuilder&&) = default;
    QueryPlanBuilder& operator=(QueryPlanBuilder&&) = default;

    /// Add a source to the query plan.
    /// @return BuilderSourceId that can be used as input to add_pipeline()
    BuilderSourceId add_source()
    {
        BuilderSourceId id = next_source_id_++;
        auto controller = std::make_shared<TestSourceController>();
        source_controllers_[id] = controller;
        source_order_.push_back(id);
        return id;
    }

    /// Add a source with a custom controller.
    /// @param controller Pre-configured controller for the source
    /// @return BuilderSourceId that can be used as input to add_pipeline()
    BuilderSourceId add_source(std::shared_ptr<TestSourceController> controller)
    {
        BuilderSourceId id = next_source_id_++;
        source_controllers_[id] = std::move(controller);
        source_order_.push_back(id);
        return id;
    }

    /// Add a pipeline stage to the query plan.
    /// @param inputs IDs of sources or pipelines that feed into this pipeline
    /// @return BuilderPipelineId that can be used as input to other pipelines or sinks
    BuilderPipelineId add_pipeline(std::vector<std::variant<BuilderSourceId, BuilderPipelineId>> inputs)
    {
        BuilderPipelineId id = next_pipeline_id_++;
        auto controller = std::make_shared<TestPipelineController>();
        pipeline_controllers_[id] = controller;
        pipeline_inputs_[id] = std::move(inputs);
        pipeline_order_.push_back(id);
        return id;
    }

    /// Add a pipeline with a custom controller.
    /// @param inputs IDs of sources or pipelines that feed into this pipeline
    /// @param controller Pre-configured controller for the pipeline
    /// @return BuilderPipelineId that can be used as input to other pipelines or sinks
    BuilderPipelineId
    add_pipeline(std::vector<std::variant<BuilderSourceId, BuilderPipelineId>> inputs, std::shared_ptr<TestPipelineController> controller)
    {
        BuilderPipelineId id = next_pipeline_id_++;
        pipeline_controllers_[id] = std::move(controller);
        pipeline_inputs_[id] = std::move(inputs);
        pipeline_order_.push_back(id);
        return id;
    }

    /// Add a sink (terminal stage) to the query plan.
    /// @param inputs IDs of sources or pipelines that feed into this sink
    /// @return BuilderSinkId for accessing the sink controller
    BuilderSinkId add_sink(std::vector<std::variant<BuilderSourceId, BuilderPipelineId>> inputs)
    {
        BuilderSinkId id = next_sink_id_++;
        auto controller = std::make_shared<TestSinkController>();
        sink_controllers_[id] = controller;
        sink_inputs_[id] = std::move(inputs);
        sink_order_.push_back(id);
        return id;
    }

    /// Add a sink with a custom controller.
    /// @param inputs IDs of sources or pipelines that feed into this sink
    /// @param controller Pre-configured controller for the sink
    /// @return BuilderSinkId for accessing the sink controller
    BuilderSinkId
    add_sink(std::vector<std::variant<BuilderSourceId, BuilderPipelineId>> inputs, std::shared_ptr<TestSinkController> controller)
    {
        BuilderSinkId id = next_sink_id_++;
        sink_controllers_[id] = std::move(controller);
        sink_inputs_[id] = std::move(inputs);
        sink_order_.push_back(id);
        return id;
    }

    /// Build the query plan.
    /// @return QueryPlanBuildResult containing the plan and controller maps
    /// @throws std::runtime_error if the plan is invalid (e.g., dangling references)
    QueryPlanBuildResult build()
    {
        QueryPlanBuildResult result;

        /// Track stage indices for building edges
        /// Source index in sources vector -> stage index that receives from it
        std::unordered_map<BuilderSourceId, std::vector<uint64_t>, BuilderSourceId::Hash> source_to_stage_indices;
        /// Pipeline ID -> stage index
        std::unordered_map<BuilderPipelineId, uint64_t, BuilderPipelineId::Hash> pipeline_to_stage_index;
        /// Sink ID -> stage index
        std::unordered_map<BuilderSinkId, uint64_t, BuilderSinkId::Hash> sink_to_stage_index;

        /// Create sources (in order)
        uint64_t source_index = 0;
        for (BuilderSourceId source_id : source_order_)
        {
            auto& controller = source_controllers_.at(source_id);
            auto source = std::make_unique<TestSource>("source-" + std::to_string(source_id.value), controller, provider_);
            result.plan.sources.push_back(source.get());
            result.owned_sources.push_back(std::move(source));
            result.source_controllers[source_id] = controller;

            /// Track source index for source_to_stage mappings
            source_index++;
        }

        /// Create pipelines (in order)
        for (BuilderPipelineId pipeline_id : pipeline_order_)
        {
            auto& controller = pipeline_controllers_.at(pipeline_id);
            auto pipeline = std::make_unique<TestPipeline>("pipeline-" + std::to_string(pipeline_id.value), controller);

            uint64_t stage_index = static_cast<uint64_t>(result.plan.stages.size());
            pipeline_to_stage_index[pipeline_id] = stage_index;

            result.plan.stages.push_back(pipeline.get());
            result.owned_stages.push_back(std::move(pipeline));
            result.pipeline_controllers[pipeline_id] = controller;

            /// Track inputs for edge/source_to_stage creation
            for (const auto& input : pipeline_inputs_.at(pipeline_id))
            {
                if (std::holds_alternative<BuilderSourceId>(input))
                {
                    BuilderSourceId sid = std::get<BuilderSourceId>(input);
                    source_to_stage_indices[sid].push_back(stage_index);
                }
            }
        }

        /// Create sinks (in order)
        for (BuilderSinkId sink_id : sink_order_)
        {
            auto& controller = sink_controllers_.at(sink_id);
            auto sink = std::make_unique<TestSink>("sink-" + std::to_string(sink_id.value), controller, provider_);

            uint64_t stage_index = static_cast<uint64_t>(result.plan.stages.size());
            sink_to_stage_index[sink_id] = stage_index;

            result.plan.stages.push_back(sink.get());
            result.owned_stages.push_back(std::move(sink));
            result.sink_controllers[sink_id] = controller;

            /// Track inputs for edge/source_to_stage creation
            for (const auto& input : sink_inputs_.at(sink_id))
            {
                if (std::holds_alternative<BuilderSourceId>(input))
                {
                    BuilderSourceId sid = std::get<BuilderSourceId>(input);
                    source_to_stage_indices[sid].push_back(stage_index);
                }
            }
        }

        /// Build source_to_stage mappings
        uint64_t src_idx = 0;
        for (BuilderSourceId source_id : source_order_)
        {
            auto it = source_to_stage_indices.find(source_id);
            if (it != source_to_stage_indices.end())
            {
                for (uint64_t stage_idx : it->second)
                {
                    result.plan.source_to_stage.emplace_back(src_idx, stage_idx);
                }
            }
            src_idx++;
        }

        /// Build edges (pipeline to pipeline, pipeline to sink)
        for (BuilderPipelineId pipeline_id : pipeline_order_)
        {
            uint64_t target_stage = pipeline_to_stage_index.at(pipeline_id);
            for (const auto& input : pipeline_inputs_.at(pipeline_id))
            {
                if (std::holds_alternative<BuilderPipelineId>(input))
                {
                    BuilderPipelineId source_pipeline = std::get<BuilderPipelineId>(input);
                    uint64_t source_stage = pipeline_to_stage_index.at(source_pipeline);
                    result.plan.edges.push_back({source_stage, target_stage});
                }
                /// BuilderSourceId inputs are handled via source_to_stage, not edges
            }
        }

        for (BuilderSinkId sink_id : sink_order_)
        {
            uint64_t target_stage = sink_to_stage_index.at(sink_id);
            for (const auto& input : sink_inputs_.at(sink_id))
            {
                if (std::holds_alternative<BuilderPipelineId>(input))
                {
                    BuilderPipelineId source_pipeline = std::get<BuilderPipelineId>(input);
                    uint64_t source_stage = pipeline_to_stage_index.at(source_pipeline);
                    result.plan.edges.push_back({source_stage, target_stage});
                }
                /// BuilderSourceId inputs are handled via source_to_stage, not edges
            }
        }

        /// Release ownership from unique_ptrs - the Rust engine takes ownership
        /// of C++ objects via CppSourceHandle/CppPipelineStage Drop implementations
        /// which call source_destroy/stage_destroy to free the objects.
        for (auto& s : result.owned_sources)
            s.release();
        result.owned_sources.clear();
        for (auto& s : result.owned_stages)
            s.release();
        result.owned_stages.clear();

        return result;
    }

private:
    test::TestBufferProvider* provider_;

    /// ID generators (start from 1 to make 0 invalid)
    BuilderSourceId next_source_id_{BuilderSourceId{1}};
    BuilderPipelineId next_pipeline_id_{BuilderPipelineId{1}};
    BuilderSinkId next_sink_id_{BuilderSinkId{1}};

    /// Controllers
    std::unordered_map<BuilderSourceId, std::shared_ptr<TestSourceController>, BuilderSourceId::Hash> source_controllers_;
    std::unordered_map<BuilderPipelineId, std::shared_ptr<TestPipelineController>, BuilderPipelineId::Hash> pipeline_controllers_;
    std::unordered_map<BuilderSinkId, std::shared_ptr<TestSinkController>, BuilderSinkId::Hash> sink_controllers_;

    /// Input tracking for edges and source_to_stage
    std::unordered_map<BuilderPipelineId, std::vector<std::variant<BuilderSourceId, BuilderPipelineId>>, BuilderPipelineId::Hash>
        pipeline_inputs_;
    std::unordered_map<BuilderSinkId, std::vector<std::variant<BuilderSourceId, BuilderPipelineId>>, BuilderSinkId::Hash> sink_inputs_;

    /// Preserve insertion order
    std::vector<BuilderSourceId> source_order_;
    std::vector<BuilderPipelineId> pipeline_order_;
    std::vector<BuilderSinkId> sink_order_;
};

} /// namespace adaptive_engine::test
