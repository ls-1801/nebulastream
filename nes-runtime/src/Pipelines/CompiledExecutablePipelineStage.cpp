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
#include <Pipelines/CompiledExecutablePipelineStage.hpp>

#include <chrono>
#include <functional>
#include <memory>
#include <ostream>
#include <unordered_map>
#include <utility>
#include <BufferManagement/NesBufferProvider.hpp>
#include <Nautilus/Interface/RecordBuffer.hpp>
#include <Runtime/Execution/OperatorHandler.hpp>
#include <Runtime/TupleBuffer.hpp>
#include <cpptrace/from_current.hpp>
#include <fmt/format.h>
#include <nautilus/val_ptr.hpp>
#include <CompilationContext.hpp>
#include <Engine.hpp>
#include <ExecutionContext.hpp>
#include <PhysicalOperator.hpp>
#include <Pipeline.hpp>
#include <function.hpp>
#include <options.hpp>

namespace NES
{

namespace
{

/// Adapter class that bridges adaptive_engine::ExecutionContext to PipelineExecutionContext.
/// This allows the internal NES ExecutionContext struct to work with the new adaptive_engine interface.
class PipelineExecutionContextAdapter final : public PipelineExecutionContext
{
public:
    explicit PipelineExecutionContextAdapter(
        adaptive_engine::ExecutionContext& ctx,
        std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& handlers)
        : adaptiveCtx_(ctx), operatorHandlers_(handlers)
    {
    }

    bool emitBuffer(const TupleBuffer& buffer, ContinuationPolicy /*policy*/) override
    {
        // Get the buffer provider from the adaptive context
        auto* bufferProvider = adaptiveCtx_.get_buffer_provider();

        // Create a BufferHandle by wrapping the TupleBuffer
        // We need to cast away const since wrap() doesn't take const pointer
        auto* mutableBuffer = const_cast<TupleBuffer*>(&buffer);
        adaptive_engine::BufferMetadata metadata{};
        auto handle = bufferProvider->wrap(mutableBuffer, buffer.getBufferSize(), metadata);

        // Emit through the adaptive context
        adaptiveCtx_.emit_buffer(handle);
        return true;
    }

    void repeatTask(const TupleBuffer& /*buffer*/, std::chrono::milliseconds /*delay*/) override
    {
        adaptiveCtx_.repeat_task();
    }

    TupleBuffer allocateTupleBuffer() override
    {
        auto handle = adaptiveCtx_.allocate_buffer(0);  // Use default pool size
        if (handle.opaque == nullptr)
        {
            throw std::runtime_error("Failed to allocate tuple buffer");
        }

        auto* wrapper = static_cast<NesBufferWrapper*>(handle.opaque);
        TupleBuffer buffer = wrapper->buffer;

        // Note: We copy the buffer, which increments refcount.
        // The original handle's wrapper will be cleaned up separately.
        return buffer;
    }

    [[nodiscard]] WorkerThreadId getId() const override { return WorkerThreadId(adaptiveCtx_.get_worker_id()); }

    [[nodiscard]] uint64_t getNumberOfWorkerThreads() const override
    {
        // TODO: Get actual number from context if available
        return 1;
    }

    [[nodiscard]] std::shared_ptr<AbstractBufferProvider> getBufferManager() const override
    {
        // The buffer provider is accessed through the adaptive context
        // Return a shared_ptr that does NOT own the provider (it's owned by the context)
        auto* provider = adaptiveCtx_.get_buffer_provider();
        // Cast to NesBufferProvider to get the underlying AbstractBufferProvider
        auto* nesProvider = dynamic_cast<NesBufferProvider*>(provider);
        if (nesProvider == nullptr)
        {
            throw std::runtime_error("Buffer provider is not a NesBufferProvider");
        }
        // TODO: This is a workaround - we need proper ownership management
        // For now, return a null-deleter shared_ptr since the provider outlives this context
        return std::shared_ptr<AbstractBufferProvider>(
            const_cast<AbstractBufferProvider*>(reinterpret_cast<const AbstractBufferProvider*>(provider)),
            [](AbstractBufferProvider*) {}  // No-op deleter
        );
    }

    [[nodiscard]] PipelineId getPipelineId() const override { return PipelineId(adaptiveCtx_.get_pipeline_id()); }

    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& getOperatorHandlers() override
    {
        return operatorHandlers_;
    }

    void setOperatorHandlers(std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& handlers) override
    {
        operatorHandlers_ = handlers;
    }

private:
    adaptive_engine::ExecutionContext& adaptiveCtx_;
    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& operatorHandlers_;
};

}  // namespace

CompiledExecutablePipelineStage::CompiledExecutablePipelineStage(
    std::shared_ptr<Pipeline> pipeline,
    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>> operatorHandlers,
    nautilus::engine::Options options,
    std::string stageId)
    : engine(std::move(options))
    , compiledPipelineFunction(nullptr)
    , operatorHandlers(std::move(operatorHandlers))
    , pipeline(std::move(pipeline))
    , stageId_(std::move(stageId))
{
    if (stageId_.empty())
    {
        stageId_ = fmt::format("stage_{}", reinterpret_cast<uintptr_t>(this));
    }
}

void CompiledExecutablePipelineStage::execute(adaptive_engine::ExecutionContext& ctx, adaptive_engine::BufferHandle input)
{
    // Extract TupleBuffer from BufferHandle
    if (input.opaque == nullptr)
    {
        throw std::runtime_error("Cannot execute with null buffer handle");
    }

    auto* wrapper = static_cast<NesBufferWrapper*>(input.opaque);
    const TupleBuffer& inputTupleBuffer = wrapper->buffer;

    // Create adapter to bridge adaptive context to PipelineExecutionContext
    PipelineExecutionContextAdapter adapter(ctx, operatorHandlers);
    adapter.setOperatorHandlers(operatorHandlers);

    Arena arena(adapter.getBufferManager());
    compiledPipelineFunction(std::addressof(adapter), std::addressof(inputTupleBuffer), std::addressof(arena));
}

nautilus::engine::CallableFunction<void, PipelineExecutionContext*, const TupleBuffer*, const Arena*>
CompiledExecutablePipelineStage::compilePipeline() const
{
    CPPTRACE_TRY
    {
        /// We must capture the operatorPipeline by value to ensure it is not destroyed before the function is called
        /// Additionally, we can NOT use const or const references for the parameters of the lambda function
        /// NOLINTBEGIN(performance-unnecessary-value-param)
        const std::function<void(nautilus::val<PipelineExecutionContext*>, nautilus::val<const TupleBuffer*>, nautilus::val<const Arena*>)>
            compiledFunction = [this](
                                   nautilus::val<PipelineExecutionContext*> pipelineExecutionContext,
                                   nautilus::val<const TupleBuffer*> recordBufferRef,
                                   nautilus::val<const Arena*> arenaRef)
        {
            auto ctx = ExecutionContext(pipelineExecutionContext, arenaRef);
            RecordBuffer recordBuffer(recordBufferRef);

            pipeline->getRootOperator().open(ctx, recordBuffer);
            switch (ctx.getOpenReturnState())
            {
                case OpenReturnState::CONTINUE: {
                    pipeline->getRootOperator().close(ctx, recordBuffer);
                    break;
                }
                case OpenReturnState::REPEAT: {
                    nautilus::invoke(
                        +[](PipelineExecutionContext* pec, const TupleBuffer* buffer)
                        { pec->repeatTask(*buffer, std::chrono::milliseconds(0)); },
                        pipelineExecutionContext,
                        recordBufferRef);
                    break;
                }
            }
        };
        /// NOLINTEND(performance-unnecessary-value-param)
        return engine.registerFunction(compiledFunction);
    }
    CPPTRACE_CATCH(...)
    {
        throw wrapExternalException(fmt::format("Could not query compile pipeline: {}", *pipeline));
    }
    std::unreachable();
}

void CompiledExecutablePipelineStage::stop(adaptive_engine::ExecutionContext& ctx)
{
    PipelineExecutionContextAdapter adapter(ctx, operatorHandlers);
    adapter.setOperatorHandlers(operatorHandlers);

    Arena arena(adapter.getBufferManager());
    ExecutionContext nesCtx(std::addressof(adapter), std::addressof(arena));
    pipeline->getRootOperator().terminate(nesCtx);
}

std::ostream& CompiledExecutablePipelineStage::toString(std::ostream& os) const
{
    return os << "CompiledExecutablePipelineStage(" << stageId_ << ")";
}

void CompiledExecutablePipelineStage::start(adaptive_engine::ExecutionContext& ctx)
{
    PipelineExecutionContextAdapter adapter(ctx, operatorHandlers);
    adapter.setOperatorHandlers(operatorHandlers);

    Arena arena(adapter.getBufferManager());
    ExecutionContext nesCtx(std::addressof(adapter), std::addressof(arena));
    CompilationContext compilationCtx{engine};
    pipeline->getRootOperator().setup(nesCtx, compilationCtx);
    compiledPipelineFunction = this->compilePipeline();
}

std::string CompiledExecutablePipelineStage::get_id() const
{
    return stageId_;
}

}
