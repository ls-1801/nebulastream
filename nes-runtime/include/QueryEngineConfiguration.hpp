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

#include <string>
#include <vector>
#include <Configurations/BaseConfiguration.hpp>
#include <Configurations/ScalarOption.hpp>
#include <Configurations/Validation/NumberValidation.hpp>

namespace NES
{

/// Configuration options for the query engine.
class QueryEngineConfiguration final : public BaseConfiguration
{
public:
    QueryEngineConfiguration() = default;

    QueryEngineConfiguration(const std::string& name, const std::string& description) : BaseConfiguration(name, description) { }

    /// Number of worker threads for query execution.
    /// Default is 4 threads. Pipeline stages must be thread-safe,
    /// as multiple workers may concurrently execute the same pipeline stage on different buffers.
    UIntOption numWorkerThreads
        = {"num_worker_threads", "4", "Number of worker threads for query execution", {std::make_shared<NumberValidation>()}};

private:
    std::vector<BaseOption*> getOptions() override { return {&numWorkerThreads}; }
};

} /// namespace NES
