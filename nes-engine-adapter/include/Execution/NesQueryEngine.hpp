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
#include <functional>
#include <memory>
#include <Execution/NesQueryPlan.hpp>

namespace NES
{

class AbstractBufferProvider;

/// NES-owned wrapper around the adaptive execution engine, using pimpl to hide
/// all adaptive_engine types from consumers.
class NesQueryEngine
{
public:
    using QueryId = uint64_t;
    using QueryTerminatedCallback = std::function<void(QueryId)>;

    static std::unique_ptr<NesQueryEngine> create(
        std::shared_ptr<AbstractBufferProvider> bufferProvider);

    void start();
    void shutdown();

    void setQueryTerminatedCallback(QueryTerminatedCallback callback);

    QueryId submitQuery(NesQueryPlan&& plan);
    bool stopQuery(QueryId id);

    ~NesQueryEngine();

    NesQueryEngine(const NesQueryEngine&) = delete;
    NesQueryEngine& operator=(const NesQueryEngine&) = delete;

private:
    NesQueryEngine();

    class Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace NES
