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
#include <optional>
#include <string>
#include <Execution/NesSourceAdapter.hpp>
#include <adaptive_engine/SourceHandle.hpp>
#include "AdaptiveStageContext.hpp"
#include "NesBufferProvider.hpp"

namespace NES
{

/// Internal adapter that wraps a NesSourceAdapter as an adaptive_engine::SourceHandle.
/// Bridges adaptive_engine calls to NES calls, wrapping/unwrapping TupleBuffer ↔ BufferHandle.
class SourceHandleAdapter final : public adaptive_engine::SourceHandle
{
public:
    explicit SourceHandleAdapter(std::unique_ptr<NesSourceAdapter> source) : source_(std::move(source)) { }

    std::optional<adaptive_engine::BufferHandle> next_buffer(adaptive_engine::ExecutionContext& ctx) override
    {
        AdaptiveStageContext nesCtx(ctx);
        auto result = source_->nextBuffer(nesCtx);
        if (!result.has_value())
        {
            return std::nullopt;
        }
        auto* wrapper = new NesBufferWrapper(std::move(*result));
        return adaptive_engine::BufferHandle{wrapper};
    }

    void open(adaptive_engine::ExecutionContext& ctx) override
    {
        AdaptiveStageContext nesCtx(ctx);
        source_->open(nesCtx);
    }

    void close(adaptive_engine::ExecutionContext& ctx) override
    {
        AdaptiveStageContext nesCtx(ctx);
        source_->close(nesCtx);
    }

    [[nodiscard]] std::string get_id() const override { return source_->getId(); }

    void request_stop() override { source_->requestStop(); }

private:
    std::unique_ptr<NesSourceAdapter> source_;
};

} /// namespace NES
