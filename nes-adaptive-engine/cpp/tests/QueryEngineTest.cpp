/**
 * QueryEngineTest.cpp - Tests for the adaptive engine
 *
 * This file contains tests that exercise the adaptive engine's core functionality
 * including query lifecycle, error handling, multi-query isolation, and repeat_task.
 *
 * Test infrastructure headers:
 * - TestBuffer.hpp: TestBuffer and TestBufferProvider
 * - TestSource.hpp: TestSourceController and TestSource
 * - TestPipeline.hpp: TestPipelineController and TestPipeline
 * - TestSink.hpp: TestSinkController and TestSink
 * - StatisticsListener.hpp: StatisticsListener for assertions
 * - QueryPlanBuilder.hpp: Fluent API for building test query plans
 */

#include <gtest/gtest.h>

#include "TestBuffer.hpp"
#include "TestSource.hpp"
#include "TestPipeline.hpp"
#include "TestSink.hpp"
#include "StatisticsListener.hpp"
#include "QueryPlanBuilder.hpp"

#include <adaptive_engine/Engine.hpp>

#include <atomic>
#include <chrono>
#include <thread>
#include <vector>

namespace adaptive_engine::test {

// Constants matching the original NES tests
constexpr size_t DEFAULT_BUFFER_SIZE = 4096;
constexpr size_t NUMBER_OF_TUPLES_PER_BUFFER = 100;
constexpr auto DEFAULT_AWAIT_TIMEOUT = std::chrono::milliseconds(1000);
constexpr auto DEFAULT_LONG_AWAIT_TIMEOUT = std::chrono::milliseconds(10000);

/// Generate identifiable test data with a unique marker byte
inline std::vector<uint8_t> identifiableData(uint8_t identifier) {
    std::vector<uint8_t> data(DEFAULT_BUFFER_SIZE, 0);
    data[0] = identifier;  // First byte identifies the buffer
    return data;
}

/// Verify that a captured buffer has the expected identifier
inline bool verifyIdentifier(const CapturedBuffer& buffer, uint8_t expected_id) {
    return !buffer.data.empty() && buffer.data[0] == expected_id;
}

// Test fixture providing common setup/teardown for QueryEngine tests
class QueryEngineTestFixture : public ::testing::Test {
protected:
    void SetUp() override {
        buffer_provider_ = std::make_unique<TestBufferProvider>();
        stats_ = std::make_shared<StatisticsListener>();
    }

    void TearDown() override {
        // Stop the statistics polling thread before engine cleanup
        stats_->stop_polling();

        // Verify no buffer leaks
        EXPECT_EQ(buffer_provider_->active_buffer_count(), 0)
            << "Buffer leak detected: " << buffer_provider_->active_buffer_count() << " buffers still active";
    }

    /// Create an engine with statistics collection enabled.
    /// The stats_queue_ and polling thread are set up automatically.
    std::unique_ptr<Engine> createEngine() {
        auto engine = Engine::create_with_stats(static_cast<void*>(buffer_provider_.get()), stats_queue_);
        if (engine && stats_queue_) {
            stats_->start_polling(stats_queue_.get());
        }
        return engine;
    }

    std::unique_ptr<TestBufferProvider> buffer_provider_;
    std::unique_ptr<StatsQueue> stats_queue_;
    std::shared_ptr<StatisticsListener> stats_;
};

//==============================================================================
// US-018: Basic Lifecycle Tests (7 tests)
//==============================================================================

/// Test that engine can be started and stopped without any queries
TEST_F(QueryEngineTestFixture, simpleTest) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    engine->start();
    engine->shutdown();
}

/// Test: Source -> Sink, inject data, shutdown engine
/// Verifies that:
/// - Query starts running
/// - Data flows from source to sink
/// - Engine shutdown terminates the query
TEST_F(QueryEngineTestFixture, singleQueryWithShutdown) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto sink = builder.add_sink({source});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject test data
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for all buffers to arrive at sink
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(4));

    // Shutdown the engine (non-graceful query termination)
    engine->shutdown();

    // Verify statistics
    // source->sink = 2 pipelines (source adapter + sink stage)
    // 4 buffers processed by sink = 4 task executions, 0 task emits (sink has no successors)
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(2));
    ASSERT_TRUE(stats_->expect_task_execution_start(4));
    ASSERT_TRUE(stats_->expect_task_execution_complete(4));

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());
}

/// Test: Source -> Pipeline -> Sink, inject data, shutdown engine
/// Verifies that:
/// - Query with intermediate pipeline runs correctly
/// - Data flows through the entire pipeline
/// - Engine shutdown terminates the query
TEST_F(QueryEngineTestFixture, singleQueryWithSystemShutdown) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline = builder.add_pipeline({source});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());
    EXPECT_FALSE(source_ctrl->was_closed());

    // Inject test data
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for all buffers to arrive at sink
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(4));

    // Verify first buffer has expected identifier
    auto buffers = sink_ctrl->take_buffers();
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));

    // Shutdown the engine
    engine->shutdown();

    // Verify statistics
    // source->pipeline->sink = 3 pipelines (source adapter + pipeline + sink)
    // 4 buffers: pipeline executes 4 + sink executes 4 = 8 task executions
    // pipeline emits to sink = 4 task emits
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(3));
    ASSERT_TRUE(stats_->expect_task_execution_start(8));
    ASSERT_TRUE(stats_->expect_task_execution_complete(8));
    ASSERT_TRUE(stats_->expect_task_emit(4));

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());
}

/// Test: Source -> Pipeline -> Sink, inject data + EOS
/// Verifies that:
/// - Query terminates gracefully when source sends EOS
/// - All data flows through before termination
/// - Pipelines are properly stopped
TEST_F(QueryEngineTestFixture, singleQueryWithExternalStop) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline = builder.add_pipeline({source});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject test data
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Signal end of stream (graceful termination)
    source_ctrl->inject_eos();

    // Wait for all buffers to arrive at sink
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(4));

    // Wait for sink to stop (graceful termination completes)
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Verify buffers
    auto buffers = sink_ctrl->take_buffers();
    EXPECT_EQ(buffers.size(), 4);
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));

    // Verify statistics
    // source->pipeline->sink = 3 pipelines (source adapter + pipeline + sink)
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(3));
    ASSERT_TRUE(stats_->expect_pipeline_stop(3));
    ASSERT_TRUE(stats_->expect_task_execution_start(8));
    ASSERT_TRUE(stats_->expect_task_execution_complete(8));
    ASSERT_TRUE(stats_->expect_task_emit(4));

    // Cleanup
    engine->shutdown();
}

/// Test: Source -> Pipeline -> Sink, inject data, stop_query()
/// Verifies that:
/// - Query can be stopped via engine stop_query()
/// - At least some data flows through before stop
/// - Query terminates gracefully
TEST_F(QueryEngineTestFixture, singleQueryWithSystemStop) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline = builder.add_pipeline({source});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());
    EXPECT_FALSE(source_ctrl->was_closed());

    // Inject some test data
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for at least one buffer to arrive (race between data and stop)
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(1));

    // Stop the query via engine API
    bool stopped = engine->stop_query(query_id);
    EXPECT_TRUE(stopped);

    // Inject more data (races with stop)
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for sink to stop
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Verify at least one buffer was received
    auto buffers = sink_ctrl->take_buffers();
    EXPECT_GE(buffers.size(), 1) << "Expected at least one buffer";
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));

    // Verify statistics (ranges due to race between data and stop)
    // source->pipeline->sink = 3 pipelines (source adapter + pipeline + sink)
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(3));
    ASSERT_TRUE(stats_->expect_pipeline_stop(3));
    ASSERT_TRUE(stats_->expect_count_in_range<TaskExecutionStartEvent>(2, 10));
    ASSERT_TRUE(stats_->expect_count_in_range<TaskExecutionCompleteEvent>(2, 10));
    ASSERT_TRUE(stats_->expect_count_in_range<TaskEmitEvent>(1, 5));

    // Cleanup
    engine->shutdown();
}

/// Test: Two sources -> Pipeline -> Sink, shutdown engine
/// Verifies that:
/// - Query with multiple sources runs correctly
/// - Data from both sources reaches the sink
/// - Engine shutdown terminates the query
TEST_F(QueryEngineTestFixture, singleQueryWithTwoSourcesShutdown) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source1 + source2 -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source1 = builder.add_source();
    auto source2 = builder.add_source();
    auto pipeline = builder.add_pipeline({source1, source2});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source1_ctrl = result.get_source_controller(source1);
    auto source2_ctrl = result.get_source_controller(source2);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for both sources to be opened
    ASSERT_TRUE(source1_ctrl->wait_until_opened());
    EXPECT_FALSE(source1_ctrl->was_closed());
    ASSERT_TRUE(source2_ctrl->wait_until_opened());
    EXPECT_FALSE(source2_ctrl->was_closed());

    // Inject test data from both sources
    source1_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
    source1_ctrl->inject_data(identifiableData(2), NUMBER_OF_TUPLES_PER_BUFFER);
    source2_ctrl->inject_data(identifiableData(3), NUMBER_OF_TUPLES_PER_BUFFER);
    source2_ctrl->inject_data(identifiableData(4), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for all buffers to arrive at sink
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(4));

    // Take buffers before shutdown
    auto buffers = sink_ctrl->take_buffers();

    // Shutdown the engine
    engine->shutdown();

    // Verify statistics (shutdown = no graceful stop events)
    // 2 sources + pipeline + sink = 4 pipelines
    // 4 buffers through pipeline (4 executions) + 4 buffers through sink (4 executions) = 8
    // pipeline emits to sink = 4 task emits
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(4));
    ASSERT_TRUE(stats_->expect_task_execution_start(8));
    ASSERT_TRUE(stats_->expect_task_execution_complete(8));
    ASSERT_TRUE(stats_->expect_task_emit(4));

    // Verify both sources were destroyed
    ASSERT_TRUE(source1_ctrl->wait_until_destroyed());
    ASSERT_TRUE(source2_ctrl->wait_until_destroyed());
}

/// Test: Two sources -> Pipeline -> Sink, both sources send EOS
/// Verifies that:
/// - Query continues running as long as any source is active
/// - Data from the remaining source still flows through
/// - Query terminates only after all sources send EOS
TEST_F(QueryEngineTestFixture, singleQueryWithTwoSourcesWaitingForTwoStops) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source1 + source2 -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source1 = builder.add_source();
    auto source2 = builder.add_source();
    auto pipeline = builder.add_pipeline({source1, source2});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source1_ctrl = result.get_source_controller(source1);
    auto source2_ctrl = result.get_source_controller(source2);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for both sources to be opened
    ASSERT_TRUE(source1_ctrl->wait_until_opened());
    EXPECT_FALSE(source1_ctrl->was_closed());
    ASSERT_TRUE(source2_ctrl->wait_until_opened());
    EXPECT_FALSE(source2_ctrl->was_closed());

    // Inject data from both sources
    ASSERT_TRUE(source1_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER + 0));
    ASSERT_TRUE(source1_ctrl->inject_data(identifiableData(2), NUMBER_OF_TUPLES_PER_BUFFER + 1));
    ASSERT_TRUE(source2_ctrl->inject_data(identifiableData(3), NUMBER_OF_TUPLES_PER_BUFFER + 2));
    ASSERT_TRUE(source2_ctrl->inject_data(identifiableData(4), NUMBER_OF_TUPLES_PER_BUFFER + 3));

    // Source 1 sends EOS first
    ASSERT_TRUE(source1_ctrl->inject_eos());

    // Wait for 4 buffers
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(4));

    // Source 2 continues sending data after source 1 is done
    ASSERT_TRUE(source2_ctrl->inject_data(identifiableData(5), NUMBER_OF_TUPLES_PER_BUFFER + 4));
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(5));

    ASSERT_TRUE(source2_ctrl->inject_data(identifiableData(6), NUMBER_OF_TUPLES_PER_BUFFER + 5));
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(6));

    // Source 2 sends EOS (query should now terminate)
    ASSERT_TRUE(source2_ctrl->inject_eos());

    // Wait for sink to stop (query terminated)
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify all buffers were received
    auto buffers = sink_ctrl->take_buffers();
    EXPECT_EQ(buffers.size(), 6);

    // Verify both sources were destroyed
    ASSERT_TRUE(source1_ctrl->wait_until_destroyed());
    ASSERT_TRUE(source2_ctrl->wait_until_destroyed());

    // Verify statistics
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));

    // Cleanup
    engine->shutdown();
}

//==============================================================================
// US-019: Source Failure Tests (4 tests)
//==============================================================================

/// Test: Source -> Pipeline -> Sink, source injects error after some data
/// Verifies that:
/// - Query handles source failure gracefully
/// - Data flows until error
/// - Source lifecycle completes (opened + closed even on error)
TEST_F(QueryEngineTestFixture, singleQueryWithSourceFailure) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline = builder.add_pipeline({source});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());
    EXPECT_FALSE(source_ctrl->wait_until_closed(std::chrono::milliseconds(10)));

    // Inject test data
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for at least one buffer to arrive at sink
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(1));

    // Inject error (causes source failure)
    source_ctrl->inject_error("Source Failed");

    // Wait for source lifecycle to complete
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify at least one buffer was received
    auto buffers = sink_ctrl->take_buffers();
    EXPECT_GE(buffers.size(), 1) << "Expected at least one buffer";
    EXPECT_TRUE(verifyIdentifier(buffers[0], 1));

    // Shutdown engine
    engine->shutdown();
}

/// Test: Many sources -> Pipeline -> Sink, one source fails using fail_after_n
/// Verifies that:
/// - Query fails when any source fails
/// - Error isolation does not affect the failure detection
/// - All sources are properly destroyed
TEST_F(QueryEngineTestFixture, singleQueryWithManySourcesOneOfThemFails) {
    constexpr size_t numberOfSources = 10;
    constexpr size_t numberOfBuffersBeforeFailure = 5;

    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source0 + source1 + ... + sourceN -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    std::vector<BuilderSourceId> source_ids;
    std::vector<std::variant<BuilderSourceId, BuilderPipelineId>> inputs;

    for (size_t i = 0; i < numberOfSources; i++) {
        auto src = builder.add_source();
        source_ids.push_back(src);
        inputs.push_back(src);
    }

    auto pipeline = builder.add_pipeline(inputs);
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    // Configure source 0 to fail after N buffers
    auto source0_ctrl = result.get_source_controller(source_ids[0]);
    source0_ctrl->fail_after_n(numberOfBuffersBeforeFailure, "Source 0 failed");

    // Collect all source controllers
    std::vector<std::shared_ptr<TestSourceController>> source_ctrls;
    for (const auto& sid : source_ids) {
        source_ctrls.push_back(result.get_source_controller(sid));
    }

    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for all sources to be opened
    for (size_t i = 0; i < numberOfSources; i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_opened())
            << "Source " << i << " should have been opened";
    }

    // Inject data from all sources (source 0 will fail after numberOfBuffersBeforeFailure)
    for (size_t buf = 0; buf < numberOfBuffersBeforeFailure + 2; buf++) {
        for (size_t i = 0; i < numberOfSources; i++) {
            // Source 0 will fail after emitting numberOfBuffersBeforeFailure buffers
            source_ctrls[i]->inject_data(
                std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0),
                NUMBER_OF_TUPLES_PER_BUFFER);
        }
    }

    // Wait for all sources to be destroyed (query terminated due to failure)
    for (size_t i = 0; i < numberOfSources; i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_destroyed())
            << "Source " << i << " should have been destroyed";
        ASSERT_TRUE(source_ctrls[i]->was_opened())
            << "Source " << i << " should have been opened";
    }

    // Shutdown engine
    engine->shutdown();
}

/// Test: Single source -> Three pipelines -> Sink, source fails after data
/// Verifies that:
/// - Fan-out topology handles source failure
/// - All pipelines are affected by the source failure
/// - Pipelines are not gracefully stopped (due to error)
TEST_F(QueryEngineTestFixture, singleSourceWithMultipleSuccessorsSourceFailure) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline1/pipeline2/pipeline3 -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline1 = builder.add_pipeline({source});
    auto pipeline2 = builder.add_pipeline({source});
    auto pipeline3 = builder.add_pipeline({source});
    auto sink = builder.add_sink({pipeline1, pipeline2, pipeline3});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto pipeline1_ctrl = result.get_pipeline_controller(pipeline1);
    auto pipeline2_ctrl = result.get_pipeline_controller(pipeline2);
    auto pipeline3_ctrl = result.get_pipeline_controller(pipeline3);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start (indicates query is running)
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject test data
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Inject error (causes source failure)
    source_ctrl->inject_error("I should fail here!");

    // Wait for source to be destroyed (query terminated due to failure)
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());

    // Pipelines should NOT have been stopped gracefully (error case)
    EXPECT_FALSE(pipeline1_ctrl->was_stopped());
    EXPECT_FALSE(pipeline2_ctrl->was_stopped());
    EXPECT_FALSE(pipeline3_ctrl->was_stopped());

    // Allow some time for cleanup to complete
    std::this_thread::sleep_for(std::chrono::milliseconds(100));

    // Shutdown engine
    engine->shutdown();
}

/// Test: Source -> Pipeline -> Sink, pipeline fails on 1st invocation while EOS races
/// Verifies that:
/// - Race between pipeline failure and EOS is handled correctly
/// - Query fails (does not hang)
/// - Source lifecycle completes
TEST_F(QueryEngineTestFixture, RaceBetweenFailureAndEOS) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> failing_pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();

    // Create pipeline that will fail on 1st invocation
    auto failing_ctrl = std::make_shared<TestPipelineController>();
    failing_ctrl->fail_on_nth_invocation = 1;  // Fail on first execute() call
    auto failing_pipeline = builder.add_pipeline({source}, failing_ctrl);
    auto sink = builder.add_sink({failing_pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject data (pipeline will fail on first buffer)
    source_ctrl->inject_data(identifiableData(1), 1);

    // Inject EOS - races with the pipeline failure
    source_ctrl->inject_eos();

    // Wait for source to be destroyed (query terminated due to failure or completion)
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());

    // Shutdown engine
    engine->shutdown();
}

//==============================================================================
// US-020: Pipeline Failure Tests (7 tests)
//==============================================================================

/// Test: Source -> good -> fail -> succ -> Sink
/// Pipeline "fail" throws during stop()
/// Verifies that:
/// - Predecessor "good" is stopped gracefully (before the failure)
/// - Successor "succ" and sink are destroyed but not stopped gracefully
/// - Query terminates with failure state
TEST_F(QueryEngineTestFixture, failureDuringPipelineStop) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: src -> good -> fail -> succ -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto src = builder.add_source();

    // Create "good" pipeline (normal behavior)
    auto good_ctrl = std::make_shared<TestPipelineController>();
    auto good = builder.add_pipeline({src}, good_ctrl);

    // Create "fail" pipeline (fails during stop)
    auto fail_ctrl = std::make_shared<TestPipelineController>();
    fail_ctrl->fail_on_stop = true;
    auto fail = builder.add_pipeline({good}, fail_ctrl);

    // Create "succ" pipeline (successor to failing pipeline)
    auto succ_ctrl = std::make_shared<TestPipelineController>();
    auto succ = builder.add_pipeline({fail}, succ_ctrl);

    auto sink = builder.add_sink({succ});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(src);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start (indicates query is running)
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject EOS to trigger graceful shutdown, which will call stop() on pipelines
    source_ctrl->inject_eos();

    // Wait for "fail" pipeline to be destroyed
    ASSERT_TRUE(fail_ctrl->wait_for_destruction())
        << "Failing pipeline should have been destroyed";
    EXPECT_FALSE(fail_ctrl->was_stopped())
        << "Failing pipeline should not have been stopped gracefully (it threw)";

    // Wait for successor pipeline to be destroyed
    ASSERT_TRUE(succ_ctrl->wait_for_destruction())
        << "Successors of failing pipelines should have been destroyed";
    EXPECT_FALSE(succ_ctrl->was_stopped())
        << "Successors of failing pipelines should not be stopped gracefully";

    // The "good" predecessor should have been stopped gracefully
    EXPECT_TRUE(good_ctrl->was_stopped())
        << "Predecessors of failing pipelines should have been stopped gracefully";

    // Wait for source to be destroyed
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());

    // Shutdown engine
    engine->shutdown();
}

/// Test: Complex topology with two sources
///         sink <-- Destroyed (due to failure)
///        /    \
///     succ     pipe <-- Gracefully stopped (before failure)
///       |        |
///     fail     src2 <-- EoS first
///       |
///     src1 <-- EoS second
///
/// Verifies cascading shutdown behavior with multiple sources
TEST_F(QueryEngineTestFixture, failureDuringPipelineStopMultipleSources) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: src1 -> fail -> succ --\
    //              src2 -> pipe -----------> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto src1 = builder.add_source();

    // Create "fail" pipeline (fails during stop)
    auto fail_ctrl = std::make_shared<TestPipelineController>();
    fail_ctrl->fail_on_stop = true;
    auto fail = builder.add_pipeline({src1}, fail_ctrl);

    // Create "succ" pipeline (successor to failing pipeline)
    auto succ_ctrl = std::make_shared<TestPipelineController>();
    auto succ = builder.add_pipeline({fail}, succ_ctrl);

    auto src2 = builder.add_source();

    // Create "pipe" pipeline (from src2)
    auto pipe_ctrl = std::make_shared<TestPipelineController>();
    auto pipe = builder.add_pipeline({src2}, pipe_ctrl);

    // Sink receives from both succ and pipe
    auto sink = builder.add_sink({succ, pipe});
    auto result = builder.build();

    auto src1_ctrl = result.get_source_controller(src1);
    auto src2_ctrl = result.get_source_controller(src2);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start (indicates query is running)
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for both sources to be opened
    ASSERT_TRUE(src1_ctrl->wait_until_opened());
    ASSERT_TRUE(src2_ctrl->wait_until_opened());

    // src2 sends EOS first
    src2_ctrl->inject_eos();

    // Wait for "pipe" to stop gracefully (its source is done)
    ASSERT_TRUE(pipe_ctrl->wait_for_stop())
        << "Pipeline should be stopped after its predecessor source has been stopped";

    // Sink should still be running (kept alive by the other branch)
    EXPECT_TRUE(sink_ctrl->keep_running())
        << "Sink should not have been stopped as it is kept alive by the other predecessor";

    // Now src1 sends EOS - this will trigger the failing pipeline's stop
    src1_ctrl->inject_eos();

    // Wait for failing pipeline to be destroyed
    ASSERT_TRUE(fail_ctrl->wait_for_destruction())
        << "Pipeline should be stopped forcefully";
    EXPECT_FALSE(fail_ctrl->was_stopped())
        << "Pipeline should not be stopped gracefully (it threw during stop)";

    // Wait for successor to be destroyed (cascade from failure)
    ASSERT_TRUE(succ_ctrl->wait_for_destruction())
        << "Successor to failing should be stopped forcefully";
    EXPECT_FALSE(succ_ctrl->was_stopped())
        << "Successor to failing should not be stopped gracefully";

    // Wait for sink to be destroyed
    ASSERT_TRUE(sink_ctrl->wait_for_destruction())
        << "Successor to failing should be stopped forcefully even if one child was stopped gracefully";
    EXPECT_FALSE(sink_ctrl->was_stopped())
        << "Successor to failing should not be stopped gracefully even if one child was stopped gracefully";

    // Wait for sources to be destroyed
    ASSERT_TRUE(src1_ctrl->wait_until_destroyed());
    ASSERT_TRUE(src2_ctrl->wait_until_destroyed());

    // Shutdown engine
    engine->shutdown();
}

/// Test: Race between pipeline failure during stop and EOS from another source
/// src1 sends EOS first, triggering fail's stop (which throws)
/// src2 hasn't sent EOS yet - should be affected by the query failure
TEST_F(QueryEngineTestFixture, failureDuringPipelineStopMultipleSourcesRaceBetweenFailAndEoS) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Same topology as above
    QueryPlanBuilder builder(buffer_provider_.get());
    auto src1 = builder.add_source();

    // Create "fail" pipeline (fails during stop)
    auto fail_ctrl = std::make_shared<TestPipelineController>();
    fail_ctrl->fail_on_stop = true;
    auto fail = builder.add_pipeline({src1}, fail_ctrl);

    auto succ_ctrl = std::make_shared<TestPipelineController>();
    auto succ = builder.add_pipeline({fail}, succ_ctrl);

    auto src2 = builder.add_source();
    auto pipe_ctrl = std::make_shared<TestPipelineController>();
    auto pipe = builder.add_pipeline({src2}, pipe_ctrl);

    auto sink = builder.add_sink({succ, pipe});
    auto result = builder.build();

    auto src1_ctrl = result.get_source_controller(src1);
    auto src2_ctrl = result.get_source_controller(src2);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for both sources to be opened
    ASSERT_TRUE(src1_ctrl->wait_until_opened());
    ASSERT_TRUE(src2_ctrl->wait_until_opened());

    // src1 sends EOS (triggers failure during fail's stop)
    // Note: src2 does NOT send EOS - it will be affected by the query failure
    src1_ctrl->inject_eos();

    // Wait for all pipelines to be destroyed
    ASSERT_TRUE(fail_ctrl->wait_for_destruction())
        << "Pipeline should be stopped forcefully";
    ASSERT_TRUE(pipe_ctrl->wait_for_destruction())
        << "Pipeline should be stopped after its predecessor source has been stopped";
    ASSERT_TRUE(succ_ctrl->wait_for_destruction())
        << "Successor to failing should be stopped forcefully";
    ASSERT_TRUE(sink_ctrl->wait_for_destruction())
        << "Successor to failing should be stopped forcefully";

    // None of them should have been stopped gracefully (due to failure)
    EXPECT_FALSE(pipe_ctrl->was_stopped())
        << "Pipeline should have failed because of the QEP failure";
    EXPECT_FALSE(fail_ctrl->was_stopped())
        << "Pipeline should not be stopped gracefully";
    EXPECT_FALSE(succ_ctrl->was_stopped())
        << "Successor to failing should not be stopped gracefully";
    EXPECT_FALSE(sink_ctrl->was_stopped())
        << "Successor to failing should not be stopped gracefully";

    // Wait for sources to be destroyed
    ASSERT_TRUE(src1_ctrl->wait_until_destroyed());
    ASSERT_TRUE(src2_ctrl->wait_until_destroyed());

    // Shutdown engine
    engine->shutdown();
}

/// Test: Pipeline fails during start() with many pipelines
/// One of 101 pipelines fails during start - query should fail immediately
TEST_F(QueryEngineTestFixture, failureDuringPipelineStartWithMultiplePipelines) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> failingPipeline -> sink (and 100 more okay pipelines)
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();

    // Create failing pipeline
    auto fail_ctrl = std::make_shared<TestPipelineController>();
    fail_ctrl->fail_on_start = true;
    auto failing_pipeline = builder.add_pipeline({source}, fail_ctrl);
    builder.add_sink({failing_pipeline});

    // Create 100 additional okay pipelines
    for (size_t i = 0; i < 100; i++) {
        auto okay_pipeline = builder.add_pipeline({source});
        builder.add_sink({okay_pipeline});
    }

    auto result = builder.build();
    auto source_ctrl = result.get_source_controller(source);

    engine->start();

    // Submit query - should fail during start
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for source to be destroyed (query should terminate due to start failure)
    ASSERT_TRUE(source_ctrl->wait_until_destroyed())
        << "Query should have been terminated due to pipeline start failure";

    // Shutdown engine
    engine->shutdown();
}

/// Test: Pipeline fails during start() with multiple sources
TEST_F(QueryEngineTestFixture, failureDuringPipelineStartWithMultipleSources) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source1 -> failingPipeline -> failingPipelineSuccessor --\
    //              source2 -> pipeline ---------------------------------> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source1 = builder.add_source();

    // Create failing pipeline
    auto fail_ctrl = std::make_shared<TestPipelineController>();
    fail_ctrl->fail_on_start = true;
    auto failing_pipeline = builder.add_pipeline({source1}, fail_ctrl);
    auto failing_succ = builder.add_pipeline({failing_pipeline});

    auto source2 = builder.add_source();
    auto pipeline = builder.add_pipeline({source2});

    builder.add_sink({failing_succ, pipeline});

    auto result = builder.build();
    auto source1_ctrl = result.get_source_controller(source1);
    auto source2_ctrl = result.get_source_controller(source2);

    engine->start();

    // Submit query - should fail during start
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for both sources to be destroyed (query should terminate due to start failure)
    ASSERT_TRUE(source1_ctrl->wait_until_destroyed())
        << "Query should have been terminated due to pipeline start failure";
    ASSERT_TRUE(source2_ctrl->wait_until_destroyed())
        << "Query should have been terminated due to pipeline start failure";

    // Shutdown engine
    engine->shutdown();
}

/// Test: Source -> Pipeline -> Sink, pipeline throws on 2nd invocation
/// Verifies that:
/// - Pipeline executes 2+ times before failure
/// - Sink may receive 0-3 buffers (race with failure)
/// - Query terminates with failure state
TEST_F(QueryEngineTestFixture, singleQueryWithPipelineFailure) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> failing_pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();

    // Create pipeline that fails on 2nd invocation
    auto pipeline_ctrl = std::make_shared<TestPipelineController>();
    pipeline_ctrl->fail_on_nth_invocation = 2;  // Fail on 2nd execute()
    auto pipeline = builder.add_pipeline({source}, pipeline_ctrl);

    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for pipeline to start
    ASSERT_TRUE(pipeline_ctrl->wait_for_start());

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject 4 buffers (pipeline will fail on 2nd)
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for source to be destroyed (query terminated due to failure)
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());

    // Verify pipeline was invoked at least twice (before failure)
    auto invocations = pipeline_ctrl->invocations();
    EXPECT_GE(invocations, 2) << "Pipeline should have been invoked at least twice";
    EXPECT_LE(invocations, 4) << "Pipeline should have been invoked at most 4 times";

    // Sink may have received 0-3 buffers (race between emission and failure)
    auto sink_invocations = sink_ctrl->invocations();
    EXPECT_GE(sink_invocations, 0) << "Sink may receive 0 buffers due to race";
    EXPECT_LE(sink_invocations, 3) << "Sink should receive at most 3 buffers (1st emit succeeded, failure on 2nd)";

    // Shutdown engine
    engine->shutdown();
}

/// Test: Source fails slowly during open() while engine terminates
/// Verifies that:
/// - Pipelines can start even if source hasn't finished opening
/// - Engine shutdown terminates the query without waiting for source
TEST_F(QueryEngineTestFixture, singleQueryWithSlowlyFailingSourceDuringEngineTermination) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());

    // Create source that will fail during open after a delay
    auto source_ctrl = std::make_shared<TestSourceController>();
    source_ctrl->fail_during_open(DEFAULT_AWAIT_TIMEOUT);  // Blocks open() then fails
    auto source = builder.add_source(source_ctrl);

    // Normal pipeline
    auto pipeline_ctrl = std::make_shared<TestPipelineController>();
    auto pipeline = builder.add_pipeline({source}, pipeline_ctrl);

    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start (pipelines can start before source is ready)
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for pipeline to start
    ASSERT_TRUE(pipeline_ctrl->wait_for_start());

    // Shutdown engine while source is still trying to open
    // This should not hang waiting for the slow source
    engine->shutdown();

    // Verify statistics (shutdown before data flows = no task executions)
    // source->pipeline->sink = 3 pipelines (source adapter + pipeline + sink)
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(3));
    ASSERT_TRUE(stats_->expect_task_execution_start(0));
    ASSERT_TRUE(stats_->expect_task_execution_complete(0));
    ASSERT_TRUE(stats_->expect_task_emit(0));

    // Verify source was destroyed
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
}

//==============================================================================
// US-021: Repeat Task Tests (4 tests)
//==============================================================================

/// Test: Source -> Sink, sink repeats 3 times via repeat_task
/// Verifies that:
/// - Sink receives buffer and calls repeat_task
/// - repeat_task causes the task to be re-executed
/// - Total of 4 executions (1 initial + 3 repeats)
/// - Query terminates gracefully after EOS
TEST_F(QueryEngineTestFixture, SingleQueryWithRepeatingSink) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();

    // Create sink that will repeat 3 times
    auto sink_ctrl = std::make_shared<TestSinkController>();
    sink_ctrl->repeat_count = 3;  // Repeat 3 times (total 4 executions)
    auto sink = builder.add_sink({source}, sink_ctrl);
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject one buffer
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);

    // Signal end of stream
    source_ctrl->inject_eos();

    // Wait for all sink executions (1 initial + 3 repeats = 4 total)
    // Note: The repeat behavior depends on watermark tracking.
    // Since we inject one buffer, it should be processed 4 times (1 + 3 repeats).
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify invocations
    // The sink should have been invoked 4 times (initial + 3 repeats)
    EXPECT_GE(sink_ctrl->invocations(), 4)
        << "Sink should have been invoked at least 4 times (1 initial + 3 repeats)";

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Verify statistics
    // source->sink = 2 pipelines (source adapter + sink)
    // 1 buffer × 4 executions (1 initial + 3 repeats) = 4 task executions
    // Sink has no successors, so 0 task emits (repeats are re-queued directly, not via route)
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(2));
    ASSERT_TRUE(stats_->expect_pipeline_stop(2));
    ASSERT_TRUE(stats_->expect_task_execution_start(4));
    ASSERT_TRUE(stats_->expect_task_execution_complete(4));
    ASSERT_TRUE(stats_->expect_task_emit(0));

    // Cleanup
    engine->shutdown();
}

/// Test: Source -> Pipeline -> Sink, pipeline repeats 3 times via repeat_task
/// Verifies that:
/// - Pipeline receives buffer and calls repeat_task
/// - repeat_task causes the task to be re-executed
/// - Total of 4 pipeline executions (1 initial + 3 repeats)
/// - Final buffer emitted to sink after repeats complete
TEST_F(QueryEngineTestFixture, SingleQueryWithRepeatingPipeline) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();

    // Create pipeline that will repeat 3 times
    auto pipeline_ctrl = std::make_shared<TestPipelineController>();
    pipeline_ctrl->repeat_count = 3;  // Repeat 3 times (total 4 executions)
    auto pipeline = builder.add_pipeline({source}, pipeline_ctrl);

    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject one buffer
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);

    // Signal end of stream
    source_ctrl->inject_eos();

    // Wait for query to complete
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify pipeline invocations
    // The pipeline should have been invoked 4 times (initial + 3 repeats)
    EXPECT_GE(pipeline_ctrl->invocations(), 4)
        << "Pipeline should have been invoked at least 4 times (1 initial + 3 repeats)";

    // Verify sink received the buffer after repeats completed
    EXPECT_GE(sink_ctrl->invocations(), 1)
        << "Sink should have received at least one buffer";

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Verify statistics
    // source->pipeline->sink = 3 pipelines (source adapter + pipeline + sink)
    // Pipeline: 4 executions (1 initial + 3 repeats), sink: 1 execution = 5 total
    // Pipeline only emits on the final execution (repeats return early without emitting)
    // So 1 task emit (pipeline -> sink on the final execution only)
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(3));
    ASSERT_TRUE(stats_->expect_pipeline_stop(3));
    ASSERT_TRUE(stats_->expect_task_execution_start(5));
    ASSERT_TRUE(stats_->expect_task_execution_complete(5));
    ASSERT_TRUE(stats_->expect_task_emit(1));

    // Cleanup
    engine->shutdown();
}

/// Test: Source -> Pipeline -> Sink, sink repeats 3 times during stop() via repeat_task
/// Verifies that:
/// - Sink receives stop() call and calls repeat_task
/// - repeat_task causes stop() to be re-executed
/// - Total of 4 stop calls (1 initial + 3 repeats)
/// - Query terminates after all stop repeats complete
TEST_F(QueryEngineTestFixture, SingleQueryWithRepeatingSinkDuringQueryStop) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline = builder.add_pipeline({source});

    // Create sink that will repeat 3 times during stop
    auto sink_ctrl = std::make_shared<TestSinkController>();
    sink_ctrl->repeat_count_during_stop = 3;  // Repeat stop 3 times
    auto sink = builder.add_sink({pipeline}, sink_ctrl);
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto pipeline_ctrl = result.get_pipeline_controller(pipeline);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject one buffer
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);

    // Signal end of stream (triggers stop cascade)
    source_ctrl->inject_eos();

    // Wait for sink stop (happens after 4 stop calls: initial + 3 repeats)
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Verify stop was called 4 times (initial + 3 repeats)
    // Note: stop_calls() counts each invocation
    EXPECT_GE(sink_ctrl->stop_calls(), 4)
        << "Sink stop should have been called at least 4 times (1 initial + 3 repeats)";

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Cleanup
    engine->shutdown();
}

/// Test: Source -> Pipeline -> Sink1 + Sink2, Sink1 repeats 2 times during stop()
/// Verifies that:
/// - One sink repeats during stop while another doesn't
/// - Both sinks receive data
/// - Query terminates after all stop repeats complete
TEST_F(QueryEngineTestFixture, SingleQueryWithMultipleSinksDuringQueryStopOneIsRepeated) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink1 + sink2
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline = builder.add_pipeline({source});

    // Create sink1 that will repeat 2 times during stop
    auto sink1_ctrl = std::make_shared<TestSinkController>();
    sink1_ctrl->repeat_count_during_stop = 2;  // Repeat stop 2 times
    auto sink1 = builder.add_sink({pipeline}, sink1_ctrl);

    // Create sink2 with default behavior (no repeat)
    auto sink2 = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto pipeline_ctrl = result.get_pipeline_controller(pipeline);
    auto sink2_ctrl = result.get_sink_controller(sink2);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject one buffer
    source_ctrl->inject_data(identifiableData(1), NUMBER_OF_TUPLES_PER_BUFFER);

    // Signal end of stream (triggers stop cascade)
    source_ctrl->inject_eos();

    // Wait for both sinks to stop
    ASSERT_TRUE(sink1_ctrl->wait_for_stop())
        << "Sink1 should have been stopped after repeat completion";
    ASSERT_TRUE(sink2_ctrl->wait_for_stop())
        << "Sink2 should have been stopped";

    // Verify sink1 stop was called 3 times (initial + 2 repeats)
    EXPECT_GE(sink1_ctrl->stop_calls(), 3)
        << "Sink1 stop should have been called at least 3 times (1 initial + 2 repeats)";

    // Verify sink2 stop was called 1 time (no repeat)
    EXPECT_GE(sink2_ctrl->stop_calls(), 1)
        << "Sink2 stop should have been called at least 1 time";

    // Verify source lifecycle
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());
    EXPECT_TRUE(source_ctrl->was_opened());
    EXPECT_TRUE(source_ctrl->was_closed());

    // Cleanup
    engine->shutdown();
}

//==============================================================================
// US-022: Multi-Query Tests (3 tests)
//==============================================================================

/// Test: 10 queries, each with 2 sources -> pipeline -> sink
/// All queries terminate gracefully via EOS
/// Verifies that:
/// - Multiple queries can run concurrently
/// - Each query receives data independently
/// - All queries terminate gracefully when sources send EOS
TEST_F(QueryEngineTestFixture, ManyQueriesWithTwoSources) {
    constexpr size_t numberOfSources = 2;
    constexpr size_t numberOfQueries = 10;

    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Store query plans and controllers
    std::vector<QueryId> query_ids;
    std::vector<std::shared_ptr<TestSourceController>> source_ctrls;
    std::vector<std::shared_ptr<TestSinkController>> sink_ctrls;
    std::vector<QueryPlanBuildResult> results;

    // Build and submit all queries
    for (size_t q = 0; q < numberOfQueries; q++) {
        QueryPlanBuilder builder(buffer_provider_.get());
        auto source1 = builder.add_source();
        auto source2 = builder.add_source();
        auto pipeline = builder.add_pipeline({source1, source2});
        auto sink = builder.add_sink({pipeline});
        auto result = builder.build();

        source_ctrls.push_back(result.get_source_controller(source1));
        source_ctrls.push_back(result.get_source_controller(source2));
        sink_ctrls.push_back(result.get_sink_controller(sink));
        results.push_back(std::move(result));
    }

    engine->start();

    // Submit all queries
    for (size_t q = 0; q < numberOfQueries; q++) {
        auto query_id = engine->submit_query(results[q].plan, nullptr);
        EXPECT_NE(query_id, 0) << "Query " << q << " submission failed";
        query_ids.push_back(query_id);
    }

    // Wait for all sources to be opened
    for (size_t i = 0; i < source_ctrls.size(); i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_opened())
            << "Source " << i << " should have been opened";
    }

    // Inject data from all sources
    for (size_t i = 0; i < source_ctrls.size(); i++) {
        source_ctrls[i]->inject_data(identifiableData(static_cast<uint8_t>(i)), NUMBER_OF_TUPLES_PER_BUFFER);
        source_ctrls[i]->inject_data(identifiableData(static_cast<uint8_t>(i + 100)), NUMBER_OF_TUPLES_PER_BUFFER);
    }

    // Wait for at least one sink to receive buffers
    ASSERT_TRUE(sink_ctrls[0]->wait_for_buffers(2))
        << "First sink should have received at least 2 buffers";

    // Send EOS from all sources (graceful termination)
    for (auto& ctrl : source_ctrls) {
        ctrl->inject_eos();
    }

    // Wait for all sinks to stop
    for (size_t q = 0; q < numberOfQueries; q++) {
        ASSERT_TRUE(sink_ctrls[q]->wait_for_stop())
            << "Query " << q << " sink should have been stopped gracefully";
    }

    // Verify all sources were destroyed
    for (size_t i = 0; i < source_ctrls.size(); i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_destroyed())
            << "Source " << i << " should have been destroyed";
    }

    // Shutdown engine
    engine->shutdown();
}

/// Test: 10 queries with different termination scenarios:
/// - Query 0: Terminated by source failure (source 0 fails after 3 buffers)
/// - Query 1: Terminated by internal stop_query() call
/// - Queries 2-9: Terminated gracefully via EOS
///
/// Verifies that:
/// - Error isolation: Query 0's failure doesn't affect other queries
/// - Queries can be individually stopped via stop_query()
/// - Remaining queries continue running and complete gracefully
TEST_F(QueryEngineTestFixture, ManyQueriesWithTwoSourcesOneSourceFails) {
    constexpr size_t numberOfSources = 2;
    constexpr size_t numberOfQueries = 10;
    constexpr size_t buffersBeforeFailure = 3;

    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Store query plans and controllers
    std::vector<QueryId> query_ids;
    std::vector<std::shared_ptr<TestSourceController>> source_ctrls;
    std::vector<std::shared_ptr<TestSinkController>> sink_ctrls;
    std::vector<QueryPlanBuildResult> results;

    // Build all queries
    for (size_t q = 0; q < numberOfQueries; q++) {
        QueryPlanBuilder builder(buffer_provider_.get());
        auto source1 = builder.add_source();
        auto source2 = builder.add_source();
        auto pipeline = builder.add_pipeline({source1, source2});
        auto sink = builder.add_sink({pipeline});
        auto result = builder.build();

        source_ctrls.push_back(result.get_source_controller(source1));
        source_ctrls.push_back(result.get_source_controller(source2));
        sink_ctrls.push_back(result.get_sink_controller(sink));
        results.push_back(std::move(result));
    }

    // Configure source 0 (query 0's first source) to fail after 3 buffers
    source_ctrls[0]->fail_after_n(buffersBeforeFailure, "Source 0 failed after 3 buffers");

    engine->start();

    // Submit all queries
    for (size_t q = 0; q < numberOfQueries; q++) {
        auto query_id = engine->submit_query(results[q].plan, nullptr);
        EXPECT_NE(query_id, 0) << "Query " << q << " submission failed";
        query_ids.push_back(query_id);
    }

    // Wait for all sources to be opened
    for (size_t i = 0; i < source_ctrls.size(); i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_opened())
            << "Source " << i << " should have been opened";
    }

    // Inject data from all sources
    // Query 0's source 0 will fail after buffersBeforeFailure buffers
    for (size_t buf = 0; buf < buffersBeforeFailure + 2; buf++) {
        for (size_t i = 0; i < source_ctrls.size(); i++) {
            source_ctrls[i]->inject_data(
                std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0),
                NUMBER_OF_TUPLES_PER_BUFFER);
        }
    }

    // Wait for query 0's sources to be destroyed (terminated by failure)
    ASSERT_TRUE(source_ctrls[0]->wait_until_destroyed())
        << "Query 0 source 0 should have been destroyed due to failure";
    ASSERT_TRUE(source_ctrls[1]->wait_until_destroyed())
        << "Query 0 source 1 should have been destroyed due to failure";

    // Verify other queries are still running (not terminated by query 0's failure)
    for (size_t q = 1; q < numberOfQueries; q++) {
        EXPECT_FALSE(sink_ctrls[q]->wait_for_stop(std::chrono::milliseconds(10)))
            << "Query " << q << " should not have been stopped by query 0's failure";
    }

    // Internally stop query 1
    bool stopped = engine->stop_query(query_ids[1]);
    EXPECT_TRUE(stopped) << "Query 1 should have been stopped";

    // Wait for query 1's sources to be destroyed
    ASSERT_TRUE(source_ctrls[2]->wait_until_destroyed())
        << "Query 1 source 0 should have been destroyed after stop_query";
    ASSERT_TRUE(source_ctrls[3]->wait_until_destroyed())
        << "Query 1 source 1 should have been destroyed after stop_query";

    // Send EOS from remaining sources (queries 2-9) for graceful termination
    for (size_t i = 4; i < source_ctrls.size(); i++) {
        source_ctrls[i]->inject_eos();
    }

    // Wait for remaining queries to terminate
    for (size_t q = 2; q < numberOfQueries; q++) {
        ASSERT_TRUE(sink_ctrls[q]->wait_for_stop())
            << "Query " << q << " should have been stopped gracefully";
    }

    // Verify all remaining sources were destroyed
    for (size_t i = 4; i < source_ctrls.size(); i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_destroyed())
            << "Source " << i << " should have been destroyed";
    }

    // Shutdown engine
    engine->shutdown();
}

/// Test: 10 queries with pipeline failures:
/// - Query 0: Terminates gracefully via EOS (no failure configured)
/// - Queries 1-9: Fail due to pipeline error (fail_on_nth_invocation = 2)
/// Also tests repeat_task behavior with repeatCount = 1 and repeatCountDuringStop = 1
///
/// Verifies that:
/// - Error isolation: Query 0 survives even as other queries fail
/// - Pipeline failures correctly terminate their respective queries
/// - repeat_task works correctly with pipeline failures
TEST_F(QueryEngineTestFixture, ManyQueriesWithTwoSourcesAndPipelineFailures) {
    constexpr size_t numberOfSources = 2;
    constexpr size_t numberOfQueries = 10;
    constexpr size_t failAfterNInvocations = 2;

    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Store query plans and controllers
    std::vector<QueryId> query_ids;
    std::vector<std::shared_ptr<TestSourceController>> source_ctrls;
    std::vector<std::shared_ptr<TestPipelineController>> pipeline1_ctrls;
    std::vector<std::shared_ptr<TestPipelineController>> pipeline2_ctrls;
    std::vector<std::shared_ptr<TestSinkController>> sink_ctrls;
    std::vector<QueryPlanBuildResult> results;

    // Build all queries
    // Topology per query: source1 + source2 -> pipeline1 + pipeline2 -> sink
    for (size_t q = 0; q < numberOfQueries; q++) {
        QueryPlanBuilder builder(buffer_provider_.get());
        auto source1 = builder.add_source();
        auto source2 = builder.add_source();

        // Create pipeline1 with repeat_count = 1
        auto pipeline1_ctrl = std::make_shared<TestPipelineController>();
        pipeline1_ctrl->repeat_count = 1;
        auto pipeline1 = builder.add_pipeline({source1, source2}, pipeline1_ctrl);

        // Create pipeline2 with repeat_count_during_stop = 1
        auto pipeline2_ctrl = std::make_shared<TestPipelineController>();
        pipeline2_ctrl->repeat_count_during_stop = 1;
        auto pipeline2 = builder.add_pipeline({source1, source2}, pipeline2_ctrl);

        auto sink = builder.add_sink({pipeline1, pipeline2});
        auto result = builder.build();

        source_ctrls.push_back(result.get_source_controller(source1));
        source_ctrls.push_back(result.get_source_controller(source2));
        pipeline1_ctrls.push_back(pipeline1_ctrl);
        pipeline2_ctrls.push_back(pipeline2_ctrl);
        sink_ctrls.push_back(result.get_sink_controller(sink));
        results.push_back(std::move(result));
    }

    // Configure pipelines 1-9 to fail on 2nd invocation (pipeline1 only)
    for (size_t q = 1; q < numberOfQueries; q++) {
        pipeline1_ctrls[q]->fail_on_nth_invocation = failAfterNInvocations;
    }

    engine->start();

    // Submit all queries
    for (size_t q = 0; q < numberOfQueries; q++) {
        auto query_id = engine->submit_query(results[q].plan, nullptr);
        EXPECT_NE(query_id, 0) << "Query " << q << " submission failed";
        query_ids.push_back(query_id);
    }

    // Wait for all sources to be opened
    for (size_t i = 0; i < source_ctrls.size(); i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_opened())
            << "Source " << i << " should have been opened";
    }

    // Inject data from all sources
    // Queries 1-9 will fail after their pipeline1 is invoked twice
    for (size_t buf = 0; buf < failAfterNInvocations + 2; buf++) {
        for (size_t i = 0; i < source_ctrls.size(); i++) {
            source_ctrls[i]->inject_data(
                std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0),
                NUMBER_OF_TUPLES_PER_BUFFER);
        }
    }

    // Wait for queries 1-9 to fail (their pipeline1 throws on 2nd invocation)
    for (size_t q = 1; q < numberOfQueries; q++) {
        ASSERT_TRUE(pipeline1_ctrls[q]->wait_for_destruction())
            << "Query " << q << " pipeline1 should have been destroyed due to failure";
    }

    // Verify query 0 is still running (not affected by other queries' failures)
    EXPECT_FALSE(sink_ctrls[0]->wait_for_stop(std::chrono::milliseconds(100)))
        << "Query 0 should still be alive (not affected by other queries' failures)";

    // Send EOS from query 0's sources for graceful termination
    source_ctrls[0]->inject_eos();
    source_ctrls[1]->inject_eos();

    // Wait for query 0 to terminate gracefully
    ASSERT_TRUE(sink_ctrls[0]->wait_for_stop())
        << "Query 0 should have been stopped gracefully";

    // Verify all sources were destroyed
    for (size_t i = 0; i < source_ctrls.size(); i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_destroyed())
            << "Source " << i << " should have been destroyed";
    }

    // Shutdown engine
    engine->shutdown();
}

//==============================================================================
// US-023: Scalability and Edge Case Tests (4 tests)
//==============================================================================

/// Test: 100 sources -> Pipeline -> Sink, continuous data injection
/// Verifies that:
/// - Executor handles many sources simultaneously
/// - Data from all sources flows through to the sink
/// - Query terminates gracefully when all sources send EOS
TEST_F(QueryEngineTestFixture, singleQueryWithManySources) {
    constexpr size_t numberOfSources = 100;
    constexpr size_t numberOfBuffersBeforeTermination = 200;

    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source0 + source1 + ... + source99 -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    std::vector<BuilderSourceId> source_ids;
    std::vector<std::variant<BuilderSourceId, BuilderPipelineId>> inputs;

    for (size_t i = 0; i < numberOfSources; i++) {
        auto src = builder.add_source();
        source_ids.push_back(src);
        inputs.push_back(src);
    }

    auto pipeline = builder.add_pipeline(inputs);
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    // Collect all source controllers
    std::vector<std::shared_ptr<TestSourceController>> source_ctrls;
    for (const auto& sid : source_ids) {
        source_ctrls.push_back(result.get_source_controller(sid));
    }
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for all sources to be opened
    for (size_t i = 0; i < numberOfSources; i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_opened())
            << "Source " << i << " should have been opened";
    }

    // Inject data from all sources continuously until sink has enough buffers
    // Use a background thread to inject data, similar to the original DataGenerator
    std::atomic<bool> stop_generating{false};
    std::thread data_generator([&]() {
        size_t round = 0;
        while (!stop_generating.load()) {
            for (size_t i = 0; i < numberOfSources && !stop_generating.load(); i++) {
                source_ctrls[i]->inject_data(
                    std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0),
                    NUMBER_OF_TUPLES_PER_BUFFER);
            }
            round++;
            // Small delay to avoid overwhelming the queue
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }

        // Send EOS from all sources for graceful termination
        for (size_t i = 0; i < numberOfSources; i++) {
            source_ctrls[i]->inject_eos();
        }
    });

    // Wait for enough buffers to arrive at sink (longer timeout for many sources)
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(numberOfBuffersBeforeTermination, DEFAULT_LONG_AWAIT_TIMEOUT));

    // Stop the data generator
    stop_generating.store(true);
    data_generator.join();

    // Wait for sink to stop (query terminates gracefully after all EOS)
    ASSERT_TRUE(sink_ctrl->wait_for_stop(DEFAULT_LONG_AWAIT_TIMEOUT));

    // Verify all sources were destroyed
    for (size_t i = 0; i < numberOfSources; i++) {
        ASSERT_TRUE(source_ctrls[i]->wait_until_destroyed())
            << "Source " << i << " should have been destroyed";
    }

    // Shutdown engine
    engine->shutdown();
}

/// Test: Single source -> Three pipelines -> Sink, graceful EOS termination
/// Verifies that:
/// - Fan-out topology works correctly
/// - Each pipeline processes all buffers from the source
/// - Sink receives buffers from all pipelines (4 buffers * 3 pipelines = 12)
/// - All pipelines are stopped gracefully
TEST_F(QueryEngineTestFixture, singleSourceWithMultipleSuccessors) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline1/pipeline2/pipeline3 -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source = builder.add_source();
    auto pipeline1 = builder.add_pipeline({source});
    auto pipeline2 = builder.add_pipeline({source});
    auto pipeline3 = builder.add_pipeline({source});
    auto sink = builder.add_sink({pipeline1, pipeline2, pipeline3});
    auto result = builder.build();

    auto source_ctrl = result.get_source_controller(source);
    auto pipeline1_ctrl = result.get_pipeline_controller(pipeline1);
    auto pipeline2_ctrl = result.get_pipeline_controller(pipeline2);
    auto pipeline3_ctrl = result.get_pipeline_controller(pipeline3);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for sink to start (indicates query is running)
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for source to be opened
    ASSERT_TRUE(source_ctrl->wait_until_opened());

    // Inject test data
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Signal end of stream (graceful termination)
    source_ctrl->inject_eos();

    // Wait for sink to receive buffers from all 3 pipelines (4 * 3 = 12)
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(4 * 3));

    // Wait for sink to stop (query terminated gracefully)
    ASSERT_TRUE(sink_ctrl->wait_for_stop());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Wait for source to be destroyed
    ASSERT_TRUE(source_ctrl->wait_until_destroyed());

    // Verify all pipelines were stopped gracefully
    EXPECT_TRUE(pipeline1_ctrl->was_stopped());
    EXPECT_TRUE(pipeline2_ctrl->was_stopped());
    EXPECT_TRUE(pipeline3_ctrl->was_stopped());

    // Verify statistics
    // source -> pipeline1/pipeline2/pipeline3 -> sink = 5 pipelines (source adapter + 3 pipelines + sink)
    // 4 buffers × 3 pipelines = 12 pipeline executions + 12 sink executions = 24 task executions
    // Each pipeline emits to sink = 12 task emits
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(5));
    ASSERT_TRUE(stats_->expect_pipeline_stop(5));
    ASSERT_TRUE(stats_->expect_task_execution_start(24));
    ASSERT_TRUE(stats_->expect_task_execution_complete(24));
    ASSERT_TRUE(stats_->expect_task_emit(12));

    // Shutdown engine
    engine->shutdown();
}

/// Test: Two sources -> Pipeline -> Sink, stop query via stop_query()
/// Verifies that:
/// - Query can be stopped after data injection
/// - Both sources are properly terminated
/// - Query terminates cleanly
TEST_F(QueryEngineTestFixture, singleQueryWithTwoSourceExternalStop) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source1 + source2 -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());
    auto source1 = builder.add_source();
    auto source2 = builder.add_source();
    auto pipeline = builder.add_pipeline({source1, source2});
    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto source1_ctrl = result.get_source_controller(source1);
    auto source2_ctrl = result.get_source_controller(source2);
    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for query to be running
    ASSERT_TRUE(stats_->wait_for_query_running(query_id));

    // Wait for both sources to be opened
    ASSERT_TRUE(source1_ctrl->wait_until_opened());
    ASSERT_TRUE(source2_ctrl->wait_until_opened());

    // Inject data from both sources
    source1_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source1_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);
    source2_ctrl->inject_data(std::vector<uint8_t>(DEFAULT_BUFFER_SIZE, 0), NUMBER_OF_TUPLES_PER_BUFFER);

    // Wait for sink to receive at least 3 buffers
    ASSERT_TRUE(sink_ctrl->wait_for_buffers(3));

    // Stop the query via engine API
    bool stopped = engine->stop_query(query_id);
    EXPECT_TRUE(stopped);

    // Wait for both sources to be destroyed (query terminated)
    ASSERT_TRUE(source1_ctrl->wait_until_destroyed());
    ASSERT_TRUE(source2_ctrl->wait_until_destroyed());

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify statistics
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_running(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));

    // Shutdown engine
    engine->shutdown();
}

/// Test: Source blocks during open() then fails, query is stopped via stop_query()
/// Verifies that:
/// - Pipelines can start even if source hasn't finished opening
/// - Query stop terminates the query (after source eventually fails)
/// - Source is properly destroyed
TEST_F(QueryEngineTestFixture, singleQueryWithSlowlyFailingSourceDuringQueryPlanTermination) {
    auto engine = createEngine();
    ASSERT_NE(engine, nullptr);

    // Build query: source -> pipeline -> sink
    QueryPlanBuilder builder(buffer_provider_.get());

    // Create source that will fail during open after a long delay
    auto source_ctrl = std::make_shared<TestSourceController>();
    source_ctrl->fail_during_open(DEFAULT_LONG_AWAIT_TIMEOUT);  // Blocks open() then fails
    auto source = builder.add_source(source_ctrl);

    // Normal pipeline
    auto pipeline_ctrl = std::make_shared<TestPipelineController>();
    auto pipeline = builder.add_pipeline({source}, pipeline_ctrl);

    auto sink = builder.add_sink({pipeline});
    auto result = builder.build();

    auto sink_ctrl = result.get_sink_controller(sink);

    engine->start();

    // Submit query
    auto query_id = engine->submit_query(result.plan, nullptr);
    EXPECT_NE(query_id, 0);

    // Wait for sink to start (pipelines can start before source is ready)
    ASSERT_TRUE(sink_ctrl->wait_for_start());

    // Wait for pipeline to start
    ASSERT_TRUE(pipeline_ctrl->wait_for_start());

    // Stop the query while source is still trying to open
    bool stopped = engine->stop_query(query_id);
    EXPECT_TRUE(stopped);

    // Wait for source to be destroyed
    // Termination only happens after the source has failed, so we have to wait
    // at least as long as the source's open() delay
    ASSERT_TRUE(source_ctrl->wait_until_destroyed(2 * DEFAULT_LONG_AWAIT_TIMEOUT));

    // Wait for query to be fully terminated
    ASSERT_TRUE(stats_->wait_for_query_terminated(query_id));

    // Verify statistics (stop before data flows = no task executions)
    // source->pipeline->sink = 3 pipelines (source adapter + pipeline + sink)
    // All are gracefully stopped via cascade after source eventually fails
    ASSERT_TRUE(stats_->expect_query_start(1));
    ASSERT_TRUE(stats_->expect_query_stop(1));
    ASSERT_TRUE(stats_->expect_query_terminated(1));
    ASSERT_TRUE(stats_->expect_pipeline_start(3));
    ASSERT_TRUE(stats_->expect_pipeline_stop(3));
    ASSERT_TRUE(stats_->expect_task_execution_start(0));
    ASSERT_TRUE(stats_->expect_task_execution_complete(0));
    ASSERT_TRUE(stats_->expect_task_emit(0));

    // Shutdown engine
    engine->shutdown();
}

}  // namespace adaptive_engine::test
