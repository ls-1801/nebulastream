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

#include <Execution/NesExecutionContext.hpp>
#include <ErrorHandling.hpp>

namespace NES
{

NesExecutionContext::NesExecutionContext(
    NesBufferProvider* bufferProvider,
    uint32_t workerId,
    uint64_t pipelineId,
    void* userData,
    EmitBufferCallback emitCallback,
    RepeatTaskCallback repeatCallback)
    : bufferProvider_(bufferProvider)
    , workerId_(workerId)
    , pipelineId_(pipelineId)
    , userData_(userData)
    , emitCallback_(std::move(emitCallback))
    , repeatCallback_(std::move(repeatCallback))
{
    PRECONDITION(bufferProvider_ != nullptr, "NesExecutionContext requires a valid buffer provider");
}

void NesExecutionContext::emit_buffer(adaptive_engine::BufferHandle handle)
{
    if (emitCallback_)
    {
        emitCallback_(handle);
    }
    // If no callback is set, the buffer is silently dropped.
    // This may happen during testing or when there are no successors.
}

void NesExecutionContext::repeat_task()
{
    if (repeatCallback_)
    {
        repeatCallback_();
    }
    // If no callback is set, the repeat request is silently ignored.
}

uint32_t NesExecutionContext::get_worker_id() const
{
    return workerId_;
}

uint64_t NesExecutionContext::get_pipeline_id() const
{
    return pipelineId_;
}

void* NesExecutionContext::get_user_data()
{
    return userData_;
}

}  // namespace NES
