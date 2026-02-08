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
#include <Execution/NesPipelineStage.hpp>
#include <Runtime/Execution/OperatorHandler.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <nautilus/Engine.hpp>
#include <ExecutionContext.hpp>
#include <Pipeline.hpp>
#include <PipelineExecutionContext.hpp>

namespace NES
{
class DumpHelper;

/// A compiled executable pipeline stage uses nautilus-lib to compile a pipeline to a code snippet.
/// Implements NesPipelineStage for the execution engine.
class CompiledExecutablePipelineStage final : public NesPipelineStage
{
public:
    CompiledExecutablePipelineStage(
        std::shared_ptr<Pipeline> pipeline,
        std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>> operatorHandler,
        nautilus::engine::Options options,
        std::string stageId = "");

    /// Called once when the pipeline starts
    /// @param ctx Execution context for this invocation
    void start(NesStageContext& ctx) override;

    /// Called for each input buffer to process
    /// @param ctx Execution context for this invocation
    /// @param buffer The TupleBuffer to process
    void doExecute(NesStageContext& ctx, TupleBuffer& buffer) override;

    /// Called once when the pipeline stops
    /// @param ctx Execution context for this invocation
    void stop(NesStageContext& ctx) override;

    /// Get the unique identifier for this stage
    /// @return Stage identifier string
    [[nodiscard]] std::string getId() const override;

    /// Legacy PipelineExecutionContext-based interface (used by input formatter test infrastructure)
    void start(PipelineExecutionContext& pipelineExecutionContext);
    void execute(const TupleBuffer& inputTupleBuffer, PipelineExecutionContext& pipelineExecutionContext);
    void stop(PipelineExecutionContext& pipelineExecutionContext);

    friend std::ostream& operator<<(std::ostream& os, const CompiledExecutablePipelineStage& stage) { return stage.toString(os); }

private:
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
