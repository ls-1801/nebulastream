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
#include <Execution/NesStageContext.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include "NesBufferProvider.hpp"

namespace NES
{

/// Internal adapter that wraps adaptive_engine::ExecutionContext as NesStageContext.
/// This is the only place (besides other internal adapters) that touches adaptive_engine types.
class AdaptiveStageContext final : public NesStageContext
{
public:
    explicit AdaptiveStageContext(adaptive_engine::ExecutionContext& ctx)
        : ctx_(ctx), nesProvider_(static_cast<NesBufferProvider*>(ctx.get_user_data()))
    {
    }

    void emitBuffer(const TupleBuffer& buffer) override
    {
        auto* wrapper = new NesBufferWrapper(buffer);
        ctx_.emit_buffer(adaptive_engine::BufferHandle{wrapper});
    }

    void repeatTask() override { ctx_.repeat_task(adaptive_engine::BufferHandle{nullptr}); }

    [[nodiscard]] uint32_t getWorkerId() const override { return ctx_.get_worker_id(); }

    [[nodiscard]] uint64_t getPipelineId() const override { return ctx_.get_pipeline_id(); }

    TupleBuffer allocateBuffer() override
    {
        auto handle = nesProvider_->allocate(0);
        if (handle.opaque == nullptr)
        {
            throw std::runtime_error("Failed to allocate tuple buffer");
        }
        auto* wrapper = static_cast<NesBufferWrapper*>(handle.opaque);
        TupleBuffer buffer = wrapper->buffer;
        wrapper->do_release();
        return buffer;
    }

    [[nodiscard]] std::shared_ptr<AbstractBufferProvider> getBufferProvider() const override
    {
        return nesProvider_->getUnderlyingProvider();
    }

private:
    adaptive_engine::ExecutionContext& ctx_;
    NesBufferProvider* nesProvider_;
};

} /// namespace NES
