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

/// Adapter class that bridges NesStageContext to PipelineExecutionContext.
/// This allows the internal NES ExecutionContext struct to work with the NesPipelineStage interface.
class PipelineExecutionContextAdapter final : public PipelineExecutionContext
{
public:
    explicit PipelineExecutionContextAdapter(
        NesStageContext& ctx, std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& handlers)
        : nesCtx_(ctx), operatorHandlers_(handlers)
    {
    }

    bool emitBuffer(const TupleBuffer& buffer, ContinuationPolicy /*policy*/) override
    {
        nesCtx_.emitBuffer(buffer);
        return true;
    }

    void repeatTask(const TupleBuffer& /*buffer*/, std::chrono::milliseconds /*delay*/) override { nesCtx_.repeatTask(); }

    TupleBuffer allocateTupleBuffer() override { return nesCtx_.allocateBuffer(); }

    [[nodiscard]] WorkerThreadId getId() const override { return WorkerThreadId(nesCtx_.getWorkerId()); }

    [[nodiscard]] uint64_t getNumberOfWorkerThreads() const override { return 1; }

    [[nodiscard]] std::shared_ptr<AbstractBufferProvider> getBufferManager() const override { return nesCtx_.getBufferProvider(); }

    [[nodiscard]] PipelineId getPipelineId() const override { return PipelineId(nesCtx_.getPipelineId()); }

    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& getOperatorHandlers() override { return operatorHandlers_; }

    void setOperatorHandlers(std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& handlers) override
    {
        operatorHandlers_ = handlers;
    }

private:
    NesStageContext& nesCtx_;
    std::unordered_map<OperatorHandlerId, std::shared_ptr<OperatorHandler>>& operatorHandlers_;
};

} /// namespace

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

void CompiledExecutablePipelineStage::doExecute(NesStageContext& ctx, TupleBuffer& inputTupleBuffer)
{
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

void CompiledExecutablePipelineStage::stop(NesStageContext& ctx)
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

void CompiledExecutablePipelineStage::start(NesStageContext& ctx)
{
    PipelineExecutionContextAdapter adapter(ctx, operatorHandlers);
    adapter.setOperatorHandlers(operatorHandlers);
    Arena arena(adapter.getBufferManager());
    ExecutionContext nesCtx(std::addressof(adapter), std::addressof(arena));
    CompilationContext compilationCtx{engine};
    pipeline->getRootOperator().setup(nesCtx, compilationCtx);
    compiledPipelineFunction = this->compilePipeline();
}

std::string CompiledExecutablePipelineStage::getId() const
{
    return stageId_;
}

/// Legacy PipelineExecutionContext-based interface (used by input formatter test infrastructure)

void CompiledExecutablePipelineStage::start(PipelineExecutionContext& pipelineExecutionContext)
{
    pipelineExecutionContext.setOperatorHandlers(operatorHandlers);

    Arena arena(pipelineExecutionContext.getBufferManager());
    ExecutionContext nesCtx(std::addressof(pipelineExecutionContext), std::addressof(arena));
    CompilationContext compilationCtx{engine};
    pipeline->getRootOperator().setup(nesCtx, compilationCtx);
    compiledPipelineFunction = this->compilePipeline();
}

void CompiledExecutablePipelineStage::execute(const TupleBuffer& inputTupleBuffer, PipelineExecutionContext& pipelineExecutionContext)
{
    pipelineExecutionContext.setOperatorHandlers(operatorHandlers);

    Arena arena(pipelineExecutionContext.getBufferManager());
    compiledPipelineFunction(std::addressof(pipelineExecutionContext), std::addressof(inputTupleBuffer), std::addressof(arena));
}

void CompiledExecutablePipelineStage::stop(PipelineExecutionContext& pipelineExecutionContext)
{
    pipelineExecutionContext.setOperatorHandlers(operatorHandlers);

    Arena arena(pipelineExecutionContext.getBufferManager());
    ExecutionContext nesCtx(std::addressof(pipelineExecutionContext), std::addressof(arena));
    pipeline->getRootOperator().terminate(nesCtx);
}

}
