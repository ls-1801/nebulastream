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
#include <string>
#include <ExecutablePipelineStage.hpp>

// Forward declarations for Rust opaque types (global namespace from CXX bridge)
struct AdaptiveExecutor;
struct AdaptiveExecutorHandle;
struct AdaptiveBuffer;

namespace NES::AdaptiveEngine
{

/// AdaptiveEngineStage is an ExecutablePipelineStage implementation that
/// integrates the Rust adaptive-engine with NebulaStream's pipeline execution.
///
/// This stage manages an adaptive-engine executor and forwards incoming
/// TupleBuffers to the Rust execution engine for processing.
///
/// Example usage:
/// ```cpp
/// auto stage = std::make_shared<AdaptiveEngineStage>("my-pipeline");
/// stage->start(ctx);
/// stage->execute(buffer, ctx);
/// stage->stop(ctx);
/// ```
class AdaptiveEngineStage : public ExecutablePipelineStage
{
public:
    /// Create an adaptive engine stage for the specified pipeline
    /// @param pipelineId Identifier for this pipeline in the adaptive engine
    explicit AdaptiveEngineStage(std::string pipelineId);

    /// Destructor ensures proper cleanup of Rust resources
    ~AdaptiveEngineStage() override;

    // Non-copyable
    AdaptiveEngineStage(const AdaptiveEngineStage&) = delete;
    AdaptiveEngineStage& operator=(const AdaptiveEngineStage&) = delete;

    // Movable
    AdaptiveEngineStage(AdaptiveEngineStage&&) noexcept;
    AdaptiveEngineStage& operator=(AdaptiveEngineStage&&) noexcept;

    /// Start the adaptive engine executor
    /// @param pipelineExecutionContext The NES execution context
    /// @throws std::runtime_error if executor fails to start
    void start(PipelineExecutionContext& pipelineExecutionContext) override;

    /// Execute processing on the input buffer
    /// @param inputTupleBuffer The input data buffer
    /// @param pipelineExecutionContext The NES execution context
    void execute(const TupleBuffer& inputTupleBuffer, PipelineExecutionContext& pipelineExecutionContext) override;

    /// Stop the adaptive engine executor
    /// @param pipelineExecutionContext The NES execution context
    void stop(PipelineExecutionContext& pipelineExecutionContext) override;

    /// Get the pipeline ID
    [[nodiscard]] const std::string& getPipelineId() const { return pipelineId; }

    /// Check if the stage is running
    [[nodiscard]] bool isRunning() const { return running; }

protected:
    std::ostream& toString(std::ostream& os) const override;

private:
    std::string pipelineId;
    bool running = false;

    // Rust opaque handles (using unique_ptr with custom deleters)
    struct ExecutorDeleter
    {
        void operator()(::AdaptiveExecutor* ptr) const;
    };
    struct HandleDeleter
    {
        void operator()(::AdaptiveExecutorHandle* ptr) const;
    };

    std::unique_ptr<::AdaptiveExecutor, ExecutorDeleter> executor;
    std::unique_ptr<::AdaptiveExecutorHandle, HandleDeleter> handle;
};

} // namespace NES::AdaptiveEngine
