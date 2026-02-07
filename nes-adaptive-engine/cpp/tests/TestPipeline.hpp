#pragma once

#include <adaptive_engine/Buffer.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include <adaptive_engine/PipelineStage.hpp>

#include <atomic>
#include <chrono>
#include <future>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>

namespace adaptive_engine::test {

/// Controller for TestPipeline used in C++ tests.
///
/// The controller allows tests to:
/// - Verify pipeline lifecycle (started, stopped, destroyed)
/// - Inject failures during start, stop, or nth invocation
/// - Configure repeat behavior for testing repeat_task
///
/// Usage:
/// ```cpp
/// auto controller = std::make_shared<TestPipelineController>();
/// auto pipeline = std::make_unique<TestPipeline>("pipeline-1", controller);
///
/// // Configure failure behavior
/// controller->fail_on_start = true;  // Fail during start()
/// controller->fail_on_stop = true;   // Fail during stop()
/// controller->fail_on_nth_invocation = 3;  // Fail on 3rd execute() call
///
/// // Configure repeat behavior
/// controller->repeat_count = 2;  // Repeat each buffer 2 times
///
/// // Wait for lifecycle events
/// ASSERT_TRUE(controller->wait_for_start());
/// ASSERT_TRUE(controller->wait_for_stop());
/// ```
class TestPipelineController {
public:
    static constexpr std::chrono::milliseconds DEFAULT_TIMEOUT{10000};

    TestPipelineController() = default;
    ~TestPipelineController() = default;

    TestPipelineController(const TestPipelineController&) = delete;
    TestPipelineController& operator=(const TestPipelineController&) = delete;
    TestPipelineController(TestPipelineController&&) = delete;
    TestPipelineController& operator=(TestPipelineController&&) = delete;

    // Configuration flags (set before pipeline execution)

    /// If true, start() will throw an exception
    std::atomic<bool> fail_on_start{false};

    /// If true, stop() will throw an exception
    std::atomic<bool> fail_on_stop{false};

    /// Fail on the nth execute() invocation (1-indexed). SIZE_MAX means never fail.
    std::atomic<size_t> fail_on_nth_invocation{SIZE_MAX};

    /// Number of times to repeat each buffer via repeat_task during execute().
    /// Uses watermark field to track repeat count.
    std::atomic<size_t> repeat_count{0};

    /// Number of times to repeat during stop() via repeat_task.
    std::atomic<size_t> repeat_count_during_stop{0};

    /// Duration to block during start()
    std::atomic<std::chrono::milliseconds> start_duration{std::chrono::milliseconds(0)};

    /// Duration to block during stop()
    std::atomic<std::chrono::milliseconds> stop_duration{std::chrono::milliseconds(0)};

    // Lifecycle synchronization

    /// Wait until the pipeline has been started.
    /// @param timeout Maximum time to wait
    /// @return true if started, false if timeout
    [[nodiscard]] bool wait_for_start(
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_future(start_future_, timeout);
    }

    /// Wait until the pipeline has been stopped.
    /// @param timeout Maximum time to wait
    /// @return true if stopped, false if timeout
    [[nodiscard]] bool wait_for_stop(
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_future(stop_future_, timeout);
    }

    /// Wait until the pipeline has been destroyed.
    /// @param timeout Maximum time to wait
    /// @return true if destroyed, false if timeout
    [[nodiscard]] bool wait_for_destruction(
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_future(destruction_future_, timeout);
    }

    /// Check if the pipeline was started (non-blocking).
    [[nodiscard]] bool was_started() const {
        return wait_for_future(start_future_, std::chrono::milliseconds(0));
    }

    /// Check if the pipeline was stopped (non-blocking).
    [[nodiscard]] bool was_stopped() const {
        return wait_for_future(stop_future_, std::chrono::milliseconds(0));
    }

    /// Check if the pipeline was destroyed (non-blocking).
    [[nodiscard]] bool was_destroyed() const {
        return wait_for_future(destruction_future_, std::chrono::milliseconds(0));
    }

    /// Check if the pipeline is still running (not stopped).
    /// Returns true if stop has NOT happened within timeout.
    [[nodiscard]] bool keep_running(
        std::chrono::milliseconds timeout = std::chrono::milliseconds(1000)) const {
        return !wait_for_future(stop_future_, timeout);
    }

    // Statistics

    /// Get the number of times execute() was called.
    [[nodiscard]] size_t invocations() const {
        return invocations_.load();
    }

    /// Get the number of times stop() was called.
    [[nodiscard]] size_t stop_calls() const {
        return stop_calls_.load();
    }

private:
    friend class TestPipeline;

    static bool wait_for_future(const std::shared_future<void>& fut,
                                std::chrono::milliseconds timeout) {
        return fut.wait_for(timeout) == std::future_status::ready;
    }

    // Lifecycle promises/futures
    std::promise<void> start_promise_;
    std::promise<void> stop_promise_;
    std::promise<void> destruction_promise_;
    std::shared_future<void> start_future_{start_promise_.get_future().share()};
    std::shared_future<void> stop_future_{stop_promise_.get_future().share()};
    std::shared_future<void> destruction_future_{destruction_promise_.get_future().share()};

    // Statistics
    std::atomic<size_t> invocations_{0};
    std::atomic<size_t> stop_calls_{0};
};

/// Test pipeline implementing adaptive_engine::PipelineStage.
///
/// This pipeline is controlled by a TestPipelineController which allows tests
/// to inject failures and verify lifecycle events.
class TestPipeline : public PipelineStage {
public:
    /// Create a test pipeline with a controller.
    /// @param id Pipeline identifier
    /// @param controller Controller for this pipeline
    TestPipeline(std::string id, std::shared_ptr<TestPipelineController> controller)
        : id_(std::move(id))
        , controller_(std::move(controller)) {}

    ~TestPipeline() override {
        try {
            controller_->destruction_promise_.set_value();
        } catch (const std::future_error&) {
            // Already set, ignore
        }
    }

    TestPipeline(const TestPipeline&) = delete;
    TestPipeline& operator=(const TestPipeline&) = delete;
    TestPipeline(TestPipeline&&) = delete;
    TestPipeline& operator=(TestPipeline&&) = delete;

    void start(ExecutionContext& /*ctx*/) override {
        // Block if configured
        auto duration = controller_->start_duration.load();
        if (duration.count() > 0) {
            std::this_thread::sleep_for(duration);
        }

        // Signal start
        try {
            controller_->start_promise_.set_value();
        } catch (const std::future_error&) {
            // Already set, ignore
        }

        // Fail if configured
        if (controller_->fail_on_start.load()) {
            throw std::runtime_error("Pipeline start failed");
        }
    }

    void execute(ExecutionContext& ctx, BufferHandle input) override {
        size_t invocation = controller_->invocations_.fetch_add(1) + 1;

        // Check fail_on_nth_invocation
        if (invocation == controller_->fail_on_nth_invocation.load()) {
            throw std::runtime_error("Pipeline execute failed on invocation " + std::to_string(invocation));
        }

        // Handle repeat functionality
        size_t max_repeats = controller_->repeat_count.load();
        if (max_repeats > 0) {
            // Get current repeat count from watermark
            // (watermark is used as a counter in tests)
            uint64_t current_repeat = ctx.get_buffer_provider()->get_metadata(input).watermark;
            if (current_repeat < max_repeats) {
                // Request repeat with incremented counter
                // Note: We need to modify metadata, but can't do that directly.
                // For test purposes, we use repeat_task
                ctx.repeat_task();
                return;
            }
        }

        // Pass buffer downstream
        ctx.emit_buffer(input);
    }

    void stop(ExecutionContext& ctx) override {
        // Block if configured
        auto duration = controller_->stop_duration.load();
        if (duration.count() > 0) {
            std::this_thread::sleep_for(duration);
        }

        // Check fail_on_stop before incrementing stop count
        if (controller_->fail_on_stop.load()) {
            throw std::runtime_error("Pipeline stop failed");
        }

        // Handle repeat during stop
        size_t stop_calls = controller_->stop_calls_.fetch_add(1);
        size_t repeats_during_stop = controller_->repeat_count_during_stop.load();

        if (stop_calls == repeats_during_stop) {
            // Final stop call - signal completion
            try {
                controller_->stop_promise_.set_value();
            } catch (const std::future_error&) {
                // Already set, ignore
            }
        } else if (stop_calls < repeats_during_stop) {
            // Request another stop via repeat_task
            ctx.repeat_task();
        }
        // If stop_calls > repeats_during_stop, we've been called too many times
        // but we don't throw here to avoid masking other errors
    }

    [[nodiscard]] std::string get_id() const override {
        return id_;
    }

private:
    std::string id_;
    std::shared_ptr<TestPipelineController> controller_;
};

}  // namespace adaptive_engine::test
