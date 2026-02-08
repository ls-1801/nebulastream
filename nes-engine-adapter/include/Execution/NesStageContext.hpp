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

#include <cstdint>
#include <memory>
#include <Runtime/TupleBuffer.hpp>

namespace NES
{

class AbstractBufferProvider;

/// Abstract execution context passed to pipeline stages during execution.
/// This is the NES-owned replacement for adaptive_engine::ExecutionContext in public APIs.
class NesStageContext
{
public:
    virtual ~NesStageContext() = default;
    virtual void emitBuffer(const TupleBuffer& buffer) = 0;
    virtual void repeatTask() = 0;
    [[nodiscard]] virtual uint32_t getWorkerId() const = 0;
    [[nodiscard]] virtual uint64_t getWorkerCount() const = 0;
    [[nodiscard]] virtual uint64_t getPipelineId() const = 0;
    virtual TupleBuffer allocateBuffer() = 0;
    [[nodiscard]] virtual std::shared_ptr<AbstractBufferProvider> getBufferProvider() const = 0;
};

} /// namespace NES
