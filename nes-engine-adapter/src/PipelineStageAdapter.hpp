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
#include <stdexcept>
#include <string>
#include <Execution/NesPipelineStage.hpp>
#include <adaptive_engine/PipelineStage.hpp>
#include "AdaptiveStageContext.hpp"
#include "NesBufferProvider.hpp"

namespace NES
{

/// Internal adapter that wraps a NesPipelineStage as an adaptive_engine::PipelineStage.
/// Bridges adaptive_engine calls (with BufferHandle) to NES calls (with TupleBuffer).
class PipelineStageAdapter final : public adaptive_engine::PipelineStage
{
public:
    explicit PipelineStageAdapter(std::unique_ptr<NesPipelineStage> stage) : stage_(std::move(stage)) { }

    void start(adaptive_engine::ExecutionContext& ctx) override
    {
        AdaptiveStageContext nesCtx(ctx);
        stage_->start(nesCtx);
    }

    void execute(adaptive_engine::ExecutionContext& ctx, adaptive_engine::BufferHandle input) override
    {
        if (input.opaque == nullptr)
        {
            throw std::runtime_error("Cannot execute with null buffer handle");
        }

        auto* wrapper = static_cast<NesBufferWrapper*>(input.opaque);
        TupleBuffer buffer = wrapper->buffer; /// copies TupleBuffer (increments ref count)

        AdaptiveStageContext nesCtx(ctx);
        stage_->doExecute(nesCtx, buffer);
    }

    void stop(adaptive_engine::ExecutionContext& ctx) override
    {
        AdaptiveStageContext nesCtx(ctx);
        stage_->stop(nesCtx);
    }

    [[nodiscard]] std::string get_id() const override { return stage_->getId(); }

private:
    std::unique_ptr<NesPipelineStage> stage_;
};

} /// namespace NES
