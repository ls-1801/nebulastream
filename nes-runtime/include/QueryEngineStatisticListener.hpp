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

#include <chrono>
#include <cstdint>
#include <type_traits>
#include <variant>
#include <Identifiers/Identifiers.hpp>

namespace NES
{

using ChronoClock = std::chrono::system_clock;

/// Base event structure with timestamp
struct BaseEvent
{
    BaseEvent() = default;
    ChronoClock::time_point timestamp = ChronoClock::now();
};

/// Event emitted when a query starts executing
struct QueryStart : BaseEvent
{
    QueryStart(WorkerThreadId threadId, LocalQueryId queryId) : threadId(threadId), queryId(queryId) { }

    QueryStart() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
};

/// Event emitted when a query fails
struct QueryFail : BaseEvent
{
    QueryFail(WorkerThreadId threadId, LocalQueryId queryId) : threadId(threadId), queryId(queryId) { }

    QueryFail() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
};

/// Event emitted when a query stop is requested
struct QueryStopRequest : BaseEvent
{
    QueryStopRequest(WorkerThreadId threadId, LocalQueryId queryId) : threadId(threadId), queryId(queryId) { }

    QueryStopRequest() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
};

/// Event emitted when a query completes stopping
struct QueryStop : BaseEvent
{
    QueryStop(WorkerThreadId threadId, LocalQueryId queryId) : threadId(threadId), queryId(queryId) { }

    QueryStop() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
};

/// Event emitted when a pipeline starts
struct PipelineStart : BaseEvent
{
    PipelineStart(WorkerThreadId threadId, LocalQueryId queryId, PipelineId pipelineId)
        : threadId(threadId), queryId(queryId), pipelineId(pipelineId)
    {
    }

    PipelineStart() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
    PipelineId pipelineId = INVALID_PIPELINE_ID;
};

/// Event emitted when a pipeline stops
struct PipelineStop : BaseEvent
{
    PipelineStop(WorkerThreadId threadId, LocalQueryId queryId, PipelineId pipelineId)
        : threadId(threadId), queryId(queryId), pipelineId(pipelineId)
    {
    }

    PipelineStop() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
    PipelineId pipelineId = INVALID_PIPELINE_ID;
};

/// Event emitted when a task starts executing
struct TaskExecutionStart : BaseEvent
{
    TaskExecutionStart(WorkerThreadId threadId, LocalQueryId queryId, PipelineId pipelineId, TaskId taskId, uint64_t numberOfTuples)
        : threadId(threadId), queryId(queryId), pipelineId(pipelineId), taskId(taskId), numberOfTuples(numberOfTuples)
    {
    }

    TaskExecutionStart() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
    PipelineId pipelineId = INVALID_PIPELINE_ID;
    TaskId taskId = INVALID_TASK_ID;
    uint64_t numberOfTuples = 0;
};

/// Event emitted when a task completes execution
struct TaskExecutionComplete : BaseEvent
{
    TaskExecutionComplete(WorkerThreadId threadId, LocalQueryId queryId, PipelineId pipelineId, TaskId taskId)
        : threadId(threadId), queryId(queryId), pipelineId(pipelineId), taskId(taskId)
    {
    }

    TaskExecutionComplete() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
    PipelineId pipelineId = INVALID_PIPELINE_ID;
    TaskId taskId = INVALID_TASK_ID;
};

/// Event emitted when a task emits data to another pipeline
struct TaskEmit : BaseEvent
{
    TaskEmit(
        WorkerThreadId threadId,
        LocalQueryId queryId,
        PipelineId fromPipeline,
        PipelineId toPipeline,
        TaskId taskId,
        uint64_t numberOfTuples)
        : threadId(threadId)
        , queryId(queryId)
        , fromPipeline(fromPipeline)
        , toPipeline(toPipeline)
        , taskId(taskId)
        , numberOfTuples(numberOfTuples)
    {
    }

    TaskEmit() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
    PipelineId fromPipeline = INVALID_PIPELINE_ID;
    PipelineId toPipeline = INVALID_PIPELINE_ID;
    TaskId taskId = INVALID_TASK_ID;
    uint64_t numberOfTuples = 0;
};

/// Event emitted when a task expires (pipeline was stopped)
struct TaskExpired : BaseEvent
{
    TaskExpired(WorkerThreadId threadId, LocalQueryId queryId, PipelineId pipelineId, TaskId taskId)
        : threadId(threadId), queryId(queryId), pipelineId(pipelineId), taskId(taskId)
    {
    }

    TaskExpired() = default;
    WorkerThreadId threadId = WorkerThreadId(0);
    LocalQueryId queryId = INVALID_LOCAL_QUERY_ID;
    PipelineId pipelineId = INVALID_PIPELINE_ID;
    TaskId taskId = INVALID_TASK_ID;
};

/// Variant type containing all possible query engine events
using Event = std::variant<
    QueryStart,
    QueryFail,
    QueryStopRequest,
    QueryStop,
    PipelineStart,
    PipelineStop,
    TaskExecutionStart,
    TaskExecutionComplete,
    TaskEmit,
    TaskExpired>;

static_assert(std::is_default_constructible_v<Event>, "Events should be default constructible");

/// Listener interface for query engine statistic events
struct QueryEngineStatisticListener
{
    virtual ~QueryEngineStatisticListener() = default;
    virtual void onEvent(Event event) = 0;
};

}  // namespace NES
