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

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <iterator>
#include <memory>
#include <ranges>
#include <thread>
#include <utility>
#include <vector>
#include <Identifiers/Identifiers.hpp>
#include <Runtime/Execution/QueryStatus.hpp>
#include <Runtime/QueryTerminationType.hpp>
#include <Util/Logger/LogLevel.hpp>
#include <Util/Logger/Logger.hpp>
#include <Util/Logger/impl/NesLogger.hpp>
#include <Util/Ranges.hpp>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <BaseUnitTest.hpp>
#include <ExecutableQueryPlan.hpp>
#include <QueryEngineStatisticListener.hpp>
#include <QueryEngineTestingInfrastructure.hpp>
#include <TestSource.hpp>

namespace NES::Testing
{
class QueryEngineTest : public Testing::BaseUnitTest
{
public:
    static void SetUpTestSuite()
    {
        Logger::setupLogging("QueryEngineTest.log", LogLevel::LOG_DEBUG);
        NES_DEBUG("Setup QueryEngineTest test class.");
    }

    void SetUp() override { BaseUnitTest::SetUp(); }
};

TEST_F(QueryEngineTest, simpleTest)
{
    TestingHarness test;
    test.start();
    test.stop();
}

/// The Query consists of just a source without any successor pipelines
TEST_F(QueryEngineTest, singleQueryWithShutdown)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source = builder.addSource();
    auto pipeline = builder.addSink({source});
    auto [queryId, query] = test.addNewQuery(std::move(builder));
    auto ctrl = test.sourceControls[source];

    /// Statistics. Note: No Pipeline Terminate and no QueryStop because engine shutdown does not gracefully terminate any query.
    test.stats.expect(
        ExpectStats::QueryStart(1),
        ExpectStats::PipelineStart(1),
        ExpectStats::TaskExecutionStart(4),
        ExpectStats::TaskExecutionComplete(4));

    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running});

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl->waitUntilOpened());

        ctrl->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ASSERT_TRUE(test.sinkControls[pipeline]->waitForNumberOfReceivedBuffersOrMore(4));

        /// The tests asserts that a query reaches the running state, to prevent flakey tests. Even if the query already produced 4 buffers
        /// shutting down the engine races the shutdown of the query and the is running report.
        ASSERT_TRUE(test.waitForQepRunning(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
    }
    test.stop();

    ASSERT_TRUE(ctrl->waitUntilDestroyed());
    EXPECT_TRUE(ctrl->wasOpened());
    EXPECT_TRUE(ctrl->wasClosed());
}

/// The Query is stopped via shutdown of the system
TEST_F(QueryEngineTest, singleQueryWithSystemShutdown)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source = builder.addSource();
    auto sink = builder.addSink({builder.addPipeline({source})});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    auto ctrl = test.sourceControls[source];
    auto sinkCtrl = test.sinkControls[sink];

    /// Statistics. Note: No Pipeline Terminate and no QueryStop because engine shutdown does not gracefully terminate any query.
    test.stats.expect(
        ExpectStats::QueryStart(1),
        ExpectStats::PipelineStart(2),
        ExpectStats::TaskExecutionStart(8),
        ExpectStats::TaskExecutionComplete(8),
        ExpectStats::TaskEmit(4));

    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running});

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl->waitUntilOpened());
        ASSERT_TRUE(test.waitForQepRunning(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
        EXPECT_FALSE(ctrl->wasClosed());

        ctrl->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);

        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(4));
    }

    auto buffers = sinkCtrl->takeBuffers();
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));
    test.stop();

    ASSERT_TRUE(ctrl->waitUntilDestroyed());
    EXPECT_TRUE(ctrl->wasOpened());
    EXPECT_TRUE(ctrl->wasClosed());
}

/// Source stop: The Query was stopped by the source
TEST_F(QueryEngineTest, singleQueryWithExternalStop)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source = builder.addSource();
    auto sink = builder.addSink({builder.addPipeline({source})});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    auto ctrl = test.sourceControls[source];
    auto sinkCtrl = test.sinkControls[sink];

    /// Statistics. Note: Pipelines are terminated because the query is gracefully stopped. The QueryTermination event is only emitted when
    /// query termination is requested via a system event, not via a source event.
    test.stats.expect(
        ExpectStats::QueryStart(1),
        ExpectStats::QueryStop(1),
        ExpectStats::QueryStopRequest(0),
        ExpectStats::PipelineStart(2),
        ExpectStats::PipelineStop(2),
        ExpectStats::TaskExecutionStart(8),
        ExpectStats::TaskExecutionComplete(8),
        ExpectStats::TaskEmit(4));

    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running, QueryState::Stopped});
    test.expectSourceTermination(queryId, source, QueryTerminationType::Graceful);

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl->waitUntilOpened());

        ctrl->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectEoS();

        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(4));
    }
    ASSERT_TRUE(sinkCtrl->waitForStop());
    ASSERT_TRUE(ctrl->waitUntilDestroyed());
    EXPECT_TRUE(ctrl->wasOpened());
    EXPECT_TRUE(ctrl->wasClosed());
    ASSERT_TRUE(test.waitForQepTermination(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
    test.stop();

    auto buffers = sinkCtrl->takeBuffers();
    EXPECT_EQ(buffers.size(), 4);
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));
}

/// System Stop: Meaning the Query was stopped internally from the query manager via the stop query
TEST_F(QueryEngineTest, singleQueryWithSystemStop)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source = builder.addSource();
    auto sink = builder.addSink({builder.addPipeline({source})});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    auto ctrl = test.sourceControls[source];
    auto sinkCtrl = test.sinkControls[sink];
    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running, QueryState::Stopped});

    /// Statistics.
    ///     Note: Pipelines are terminated because the query is gracefully stopped.
    ///           We expect a range of executed tasks between 8-10 because the query stop races with the 2nd-5th emit.
    test.stats.expect(
        ExpectStats::QueryStart(1),
        ExpectStats::QueryStop(1),
        ExpectStats::QueryStopRequest(1),
        ExpectStats::PipelineStart(2),
        ExpectStats::PipelineStop(2),
        ExpectStats::TaskExecutionStart(2, 10),
        ExpectStats::TaskExecutionComplete(2, 10),
        ExpectStats::TaskEmit(1, 5));

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl->waitUntilOpened());
        EXPECT_FALSE(ctrl->wasClosed());

        ctrl->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);

        /// Race between Source Data and System Stop
        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(1));
        test.stopQuery(queryId);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ASSERT_TRUE(test.waitForQepTermination(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
    }
    test.stop();

    ASSERT_TRUE(sinkCtrl->waitForStop());
    ASSERT_TRUE(ctrl->waitUntilDestroyed());
    EXPECT_TRUE(ctrl->wasOpened());
    EXPECT_TRUE(ctrl->wasClosed());

    auto buffers = sinkCtrl->takeBuffers();
    EXPECT_GE(buffers.size(), 1) << "Expected at least one buffer";
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));
}

TEST_F(QueryEngineTest, singleQueryWithSourceFailure)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source = builder.addSource();
    auto sink = builder.addSink({builder.addPipeline({source})});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    auto ctrl = test.sourceControls[source];
    auto sinkCtrl = test.sinkControls[sink];
    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running, QueryState::Failed});
    test.expectSourceTermination(queryId, source, QueryTerminationType::Failure);

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl->waitUntilOpened());
        EXPECT_FALSE(ctrl->waitUntilClosed());

        ctrl->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);

        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(1));
        ctrl->injectError("Source Failed");
        ASSERT_TRUE(test.waitForQepTermination(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
    }
    test.stop();

    ASSERT_TRUE(ctrl->waitUntilDestroyed());
    EXPECT_TRUE(ctrl->wasOpened());
    EXPECT_TRUE(ctrl->wasClosed());

    auto buffers = sinkCtrl->takeBuffers();
    EXPECT_GE(buffers.size(), 1) << "Expected at least one buffer";
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));
}

/// Shutdown of the Query Engine will `HardStop` all query plans.
TEST_F(QueryEngineTest, singleQueryWithTwoSourcesShutdown)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source1 = builder.addSource();
    auto source2 = builder.addSource();
    auto sink = builder.addSink({builder.addPipeline({source1, source2})});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    auto ctrl1 = test.sourceControls[source1];
    auto ctrl2 = test.sourceControls[source2];
    auto sinkCtrl = test.sinkControls[sink];
    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running});

    /// Statistics.
    ///     Note: Pipelines are not terminated, due to system shutdown

    test.stats.expect(
        ExpectStats::QueryStart(1),
        ExpectStats::QueryStop(0),
        ExpectStats::PipelineStart(2),
        ExpectStats::PipelineStop(0),
        ExpectStats::TaskExecutionStart(8),
        ExpectStats::TaskExecutionComplete(8),
        ExpectStats::TaskEmit(4));

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl1->waitUntilOpened());
        EXPECT_FALSE(ctrl1->wasClosed());

        ASSERT_TRUE(ctrl2->waitUntilOpened());
        EXPECT_FALSE(ctrl2->wasClosed());

        ctrl1->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl1->injectData(identifiableData(2), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl2->injectData(identifiableData(3), NUMBER_OF_TUPLES_PER_BUFFER);
        ctrl2->injectData(identifiableData(4), NUMBER_OF_TUPLES_PER_BUFFER);
        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(4));
    }


    auto buffers = sinkCtrl->takeBuffers();
    test.stop();

    ASSERT_TRUE(ctrl1->waitUntilDestroyed());
    ASSERT_TRUE(ctrl2->waitUntilDestroyed());
}

TEST_F(QueryEngineTest, singleQueryWithTwoSourcesWaitingForTwoStops)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source1 = builder.addSource();
    auto source2 = builder.addSource();
    auto sink = builder.addSink({builder.addPipeline({source1, source2})});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    auto ctrl1 = test.sourceControls[source1];
    auto ctrl2 = test.sourceControls[source2];
    auto sinkCtrl = test.sinkControls[sink];
    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running, QueryState::Stopped});
    test.expectSourceTermination(queryId, source1, QueryTerminationType::Graceful);
    test.expectSourceTermination(queryId, source2, QueryTerminationType::Graceful);

    test.start();
    {
        test.startQuery(queryId, std::move(query));

        ASSERT_TRUE(ctrl1->waitUntilOpened());
        EXPECT_FALSE(ctrl1->wasClosed());

        ASSERT_TRUE(ctrl2->waitUntilOpened());
        EXPECT_FALSE(ctrl2->wasClosed());

        ASSERT_TRUE(ctrl1->injectData(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER + 0));
        ASSERT_TRUE(ctrl1->injectData(identifiableData(2), NUMBER_OF_TUPLES_PER_BUFFER + 1));
        ASSERT_TRUE(ctrl2->injectData(identifiableData(3), NUMBER_OF_TUPLES_PER_BUFFER + 2));
        ASSERT_TRUE(ctrl2->injectData(identifiableData(4), NUMBER_OF_TUPLES_PER_BUFFER + 3));
        ASSERT_TRUE(ctrl1->injectEoS());

        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(4));

        ASSERT_TRUE(ctrl2->injectData(identifiableData(5), NUMBER_OF_TUPLES_PER_BUFFER + 4));
        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(5));
        ASSERT_TRUE(ctrl2->injectData(identifiableData(6), NUMBER_OF_TUPLES_PER_BUFFER + 5));
        ASSERT_TRUE(sinkCtrl->waitForNumberOfReceivedBuffersOrMore(6));
        ASSERT_TRUE(ctrl2->injectEoS());

        ASSERT_TRUE(test.waitForQepTermination(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
    }
    test.stop();

    auto buffers = sinkCtrl->takeBuffers();
    ASSERT_TRUE(ctrl1->waitUntilDestroyed());
    ASSERT_TRUE(ctrl2->waitUntilDestroyed());
}

TEST_F(QueryEngineTest, singleQueryWithTwoSourceExternalStop)
{
    constexpr size_t numberOfSources = 2;

    TestingHarness test(LARGE_NUMBER_OF_THREADS, numberOfSources * NUMBER_OF_BUFFERS_PER_SOURCE);
    auto builder = test.buildNewQuery();
    auto source1 = builder.addSource();
    auto source2 = builder.addSource();
    auto pipeline = builder.addPipeline({source1, source2});
    auto sink = builder.addSink({pipeline});
    auto [queryId, query] = test.addNewQuery(std::move(builder));

    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running, QueryState::Stopped});

    test.start();
    {
        test.startQuery(queryId, std::move(query));
        ASSERT_TRUE(test.sourceControls[source1]->waitUntilOpened());
        ASSERT_TRUE(test.sourceControls[source2]->waitUntilOpened());

        test.sourceControls[source1]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        test.sourceControls[source1]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        test.sourceControls[source2]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        ASSERT_TRUE(test.sinkControls[sink]->waitForNumberOfReceivedBuffersOrMore(3));
        test.stopQuery(queryId);
        ASSERT_TRUE(test.waitForQepTermination(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
        ASSERT_TRUE(test.sourceControls[source1]->waitUntilDestroyed());
        ASSERT_TRUE(test.sourceControls[source2]->waitUntilDestroyed());
    }
    test.stop();
}

template <typename T>
auto IsInInclusiveRange(T lo, T hi)
{
    return ::testing::AllOf(::testing::Ge((lo)), ::testing::Le((hi)));
}

TEST_F(QueryEngineTest, singleSourceWithMultipleSuccessors)
{
    TestingHarness test;
    auto builder = test.buildNewQuery();
    auto source = builder.addSource();
    auto pipeline1 = builder.addPipeline({source});
    auto pipeline2 = builder.addPipeline({source});
    auto pipeline3 = builder.addPipeline({source});
    auto sink = builder.addSink({pipeline1, pipeline2, pipeline3});

    auto [queryId, query] = test.addNewQuery(std::move(builder));
    test.expectQueryStatusEvents(queryId, {QueryState::Started, QueryState::Running, QueryState::Stopped});
    test.expectSourceTermination(queryId, source, QueryTerminationType::Graceful);

    test.start();
    {
        test.startQuery(queryId, std::move(query));
        ASSERT_TRUE(test.sinkControls[sink]->waitForStart());
        ASSERT_TRUE(test.waitForQepRunning(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));

        test.sourceControls[source]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        test.sourceControls[source]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        test.sourceControls[source]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        test.sourceControls[source]->injectData(std::vector(DEFAULT_BUFFER_SIZE, std::byte(0)), NUMBER_OF_TUPLES_PER_BUFFER);
        test.sourceControls[source]->injectEoS();

        ASSERT_TRUE(test.sinkControls[sink]->waitForNumberOfReceivedBuffersOrMore(4 * 3));
        ASSERT_TRUE(test.waitForQepTermination(queryId, DEFAULT_LONG_AWAIT_TIMEOUT));
        ASSERT_TRUE(test.sourceControls[source]->waitUntilDestroyed());
        EXPECT_TRUE(test.pipelineControls[pipeline1]->wasStopped());
        EXPECT_TRUE(test.pipelineControls[pipeline2]->wasStopped());
        EXPECT_TRUE(test.pipelineControls[pipeline3]->wasStopped());
    }
    test.stop();
}

}
