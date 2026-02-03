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

#include <AdaptiveEngineStage.hpp>
#include <AdaptiveEngineBindings.hpp>
#include <executor/lib.h>

#include <ErrorHandling.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <stdexcept>

namespace NES::AdaptiveEngine
{

// Custom deleters implementation
void AdaptiveEngineStage::ExecutorDeleter::operator()(::AdaptiveExecutor* ptr) const
{
    if (ptr)
    {
        rust::Box<::AdaptiveExecutor> boxed = rust::Box<::AdaptiveExecutor>::from_raw(ptr);
        adaptive_executor_free(std::move(boxed));
    }
}

void AdaptiveEngineStage::HandleDeleter::operator()(::AdaptiveExecutorHandle* ptr) const
{
    if (ptr)
    {
        rust::Box<::AdaptiveExecutorHandle> boxed = rust::Box<::AdaptiveExecutorHandle>::from_raw(ptr);
        adaptive_handle_free(std::move(boxed));
    }
}

AdaptiveEngineStage::AdaptiveEngineStage(std::string pipelineId) : pipelineId(std::move(pipelineId)), running(false) {}

AdaptiveEngineStage::~AdaptiveEngineStage()
{
    // Resources cleaned up by unique_ptr deleters
}

AdaptiveEngineStage::AdaptiveEngineStage(AdaptiveEngineStage&&) noexcept = default;
AdaptiveEngineStage& AdaptiveEngineStage::operator=(AdaptiveEngineStage&&) noexcept = default;

void AdaptiveEngineStage::start(PipelineExecutionContext& /*pipelineExecutionContext*/)
{
    PRECONDITION(!running, "AdaptiveEngineStage already started");

    // Create executor
    auto executorBox = adaptive_executor_new();
    auto* executorPtr = executorBox.into_raw();

    // Start the executor
    auto result = adaptive_executor_start(*executorPtr);
    if (result != ExecutorResult::Ok)
    {
        rust::Box<::AdaptiveExecutor> boxed = rust::Box<::AdaptiveExecutor>::from_raw(executorPtr);
        adaptive_executor_free(std::move(boxed));
        throw std::runtime_error("Failed to start adaptive engine executor");
    }

    // Get handle for submitting work
    auto handleBox = adaptive_executor_get_handle(*executorPtr);
    auto* handlePtr = handleBox.into_raw();

    executor.reset(executorPtr);
    handle.reset(handlePtr);
    running = true;
}

void AdaptiveEngineStage::execute(const TupleBuffer& inputTupleBuffer, PipelineExecutionContext& /*pipelineExecutionContext*/)
{
    PRECONDITION(running, "AdaptiveEngineStage not started");
    PRECONDITION(handle != nullptr, "Executor handle is null");

    // Create a mutable copy of the buffer for the builder
    // Note: This is necessary because NESTupleBufferBuilder needs a non-const reference
    auto mutableBuffer = inputTupleBuffer;

    // Get buffer data and metadata directly
    const auto* dataPtr = mutableBuffer.getAvailableMemoryArea<uint8_t>().data();
    auto bufferSize = mutableBuffer.getBufferSize();
    auto sequenceNumber = mutableBuffer.getSequenceNumber().getRawValue();
    auto originId = mutableBuffer.getOriginId().getRawValue();
    auto watermark = mutableBuffer.getWatermark().getRawValue();
    auto numberOfTuples = mutableBuffer.getNumberOfTuples();
    auto chunkNumber = static_cast<int64_t>(mutableBuffer.getChunkNumber().getRawValue());
    auto lastChunk = mutableBuffer.isLastChunk();

    // Create AdaptiveBuffer from raw data
    // Note: adaptive_buffer_create_from_ptr copies the data
    auto bufferBox = adaptive_buffer_create_from_raw(
        dataPtr,
        bufferSize,
        sequenceNumber,
        originId,
        watermark,
        numberOfTuples,
        chunkNumber,
        lastChunk);

    // Submit buffer to the pipeline
    auto result = adaptive_handle_emit(*handle, rust::Str(pipelineId.data(), pipelineId.size()), std::move(bufferBox));

    if (result != ExecutorResult::Ok)
    {
        throw std::runtime_error("Failed to emit buffer to adaptive engine");
    }
}

void AdaptiveEngineStage::stop(PipelineExecutionContext& /*pipelineExecutionContext*/)
{
    if (!running)
    {
        return;
    }

    // Release handle first
    handle.reset();

    // Shutdown executor
    if (executor)
    {
        auto result = adaptive_executor_shutdown(*executor);
        if (result != ExecutorResult::Ok && result != ExecutorResult::NotStarted)
        {
            // Log warning but don't throw - we're stopping anyway
        }
    }

    executor.reset();
    running = false;
}

std::ostream& AdaptiveEngineStage::toString(std::ostream& os) const
{
    return os << "AdaptiveEngineStage[pipelineId=" << pipelineId << ", running=" << running << "]";
}

} // namespace NES::AdaptiveEngine
