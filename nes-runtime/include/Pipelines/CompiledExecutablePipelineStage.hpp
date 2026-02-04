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

#include <iostream>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>
#include <Runtime/Execution/OperatorHandler.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <nautilus/Engine.hpp>
#include <adaptive_engine/PipelineStage.hpp>
#include <ExecutionContext.hpp>
#include <Pipeline.hpp>
#include <PipelineExecutionContext.hpp>

namespace NES
{
class DumpHelper;

/// A compiled executable pipeline stage uses nautilus-lib to compile a pipeline to a code snippet.
/// Implements the adaptive_engine::PipelineStage interface for integration with the Rust execution engine.
class CompiledExecutablePipelineStage final : public adaptive_engine::PipelineStage
{
public:
    CompiledExecutablePipelineStage(
        std::shared_ptr<Pipeline> pipeline,
        std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>> operatorHandler,
        nautilus::engine::Options options,
        std::string stageId = "");

    /// Called once when the pipeline starts
    /// @param ctx Execution context for this invocation
    void start(adaptive_engine::ExecutionContext& ctx) override;

    /// Called for each input buffer to process
    /// @param ctx Execution context for this invocation
    /// @param input The input buffer to process
    void execute(adaptive_engine::ExecutionContext& ctx, adaptive_engine::BufferHandle input) override;

    /// Called once when the pipeline stops
    /// @param ctx Execution context for this invocation
    void stop(adaptive_engine::ExecutionContext& ctx) override;

    /// Get the unique identifier for this stage
    /// @return Stage identifier string
    [[nodiscard]] std::string get_id() const override;

    friend std::ostream& operator<<(std::ostream& os, const CompiledExecutablePipelineStage& stage)
    {
        return stage.toString(os);
    }

protected:
    std::ostream& toString(std::ostream& os) const;

private:
    [[nodiscard]] nautilus::engine::CallableFunction<void, PipelineExecutionContext*, const TupleBuffer*, const Arena*>
    compilePipeline() const;
    nautilus::engine::NautilusEngine engine;
    nautilus::engine::CallableFunction<void, PipelineExecutionContext*, const TupleBuffer*, const Arena*> compiledPipelineFunction;
    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>> operatorHandlers;
    std::shared_ptr<Pipeline> pipeline;
    std::string stageId_;
};

}
