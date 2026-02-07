#pragma once

#include <adaptive_engine/Buffer.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include <adaptive_engine/PipelineStage.hpp>
#include "TestBuffer.hpp"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <future>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

namespace adaptive_engine::test {

/// Captured buffer data for test assertions.
/// Stores a copy of the buffer data along with its metadata.
struct CapturedBuffer {
    std::vector<uint8_t> data;
    BufferMetadata metadata;

    CapturedBuffer(const void* src, size_t size, const BufferMetadata& meta)
        : data(size), metadata(meta) {
        if (src != nullptr && size > 0) {
            std::memcpy(data.data(), src, size);
        }
    }
};

/// Controller for TestSink used in C++ tests.
///
/// The controller allows tests to:
/// - Wait for a specific number of buffers to be received
/// - Retrieve captured buffers for verification
/// - Verify sink lifecycle (started, stopped, destroyed)
/// - Configure repeat behavior for testing repeat_task
///
/// Usage:
/// ```cpp
/// auto controller = std::make_shared<TestSinkController>();
/// auto sink = std::make_unique<TestSink>("sink-1", controller, &buffer_provider);
///
/// // Wait for buffers
/// ASSERT_TRUE(controller->wait_for_buffers(10));  // Wait for at least 10 buffers
///
/// // Get captured buffers (sorted by sequence_number)
/// auto buffers = controller->take_buffers();
/// ASSERT_EQ(buffers.size(), 10);
///
/// // Configure repeat behavior
/// controller->repeat_count = 2;  // Repeat each buffer 2 times
/// ```
class TestSinkController {
public:
    static constexpr std::chrono::milliseconds DEFAULT_TIMEOUT{10000};

    TestSinkController() = default;
    ~TestSinkController() = default;

    TestSinkController(const TestSinkController&) = delete;
    TestSinkController& operator=(const TestSinkController&) = delete;
    TestSinkController(TestSinkController&&) = delete;
    TestSinkController& operator=(TestSinkController&&) = delete;

    // Configuration flags (set before sink execution)

    /// Number of times to repeat each buffer via repeat_task during execute().
    /// Uses watermark field to track repeat count.
    std::atomic<size_t> repeat_count{0};

    /// Number of times to repeat during stop() via repeat_task.
    std::atomic<size_t> repeat_count_during_stop{0};

    // Buffer collection

    /// Wait until at least `n` buffers have been received.
    /// @param n Minimum number of buffers to wait for
    /// @param timeout Maximum time to wait
    /// @return true if at least n buffers received, false if timeout
    [[nodiscard]] bool wait_for_buffers(
        size_t n,
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) {
        std::unique_lock<std::mutex> lock(mutex_);
        return buffer_cv_.wait_for(lock, timeout, [this, n] {
            return buffers_.size() >= n;
        });
    }

    /// Insert a buffer into the captured buffer list.
    /// Called by TestSink::execute().
    void insert_buffer(CapturedBuffer buffer) {
        std::lock_guard<std::mutex> lock(mutex_);
        buffers_.push_back(std::move(buffer));
        buffer_cv_.notify_all();
    }

    /// Take all captured buffers, sorted by sequence_number.
    /// Clears the internal buffer list.
    /// @return Vector of captured buffers sorted by sequence_number
    std::vector<CapturedBuffer> take_buffers() {
        std::lock_guard<std::mutex> lock(mutex_);
        std::vector<CapturedBuffer> result = std::move(buffers_);
        buffers_.clear();

        // Sort by sequence_number
        std::sort(result.begin(), result.end(),
            [](const CapturedBuffer& a, const CapturedBuffer& b) {
                return a.metadata.sequence_number < b.metadata.sequence_number;
            });

        return result;
    }

    /// Get the number of buffers currently captured (non-blocking).
    [[nodiscard]] size_t buffer_count() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return buffers_.size();
    }

    // Lifecycle synchronization

    /// Wait until the sink has been started.
    /// @param timeout Maximum time to wait
    /// @return true if started, false if timeout
    [[nodiscard]] bool wait_for_start(
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_future(start_future_, timeout);
    }

    /// Wait until the sink has been stopped.
    /// @param timeout Maximum time to wait
    /// @return true if stopped, false if timeout
    [[nodiscard]] bool wait_for_stop(
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_future(stop_future_, timeout);
    }

    /// Wait until the sink has been destroyed.
    /// @param timeout Maximum time to wait
    /// @return true if destroyed, false if timeout
    [[nodiscard]] bool wait_for_destruction(
        std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const {
        return wait_for_future(destruction_future_, timeout);
    }

    /// Check if the sink was started (non-blocking).
    [[nodiscard]] bool was_started() const {
        return wait_for_future(start_future_, std::chrono::milliseconds(0));
    }

    /// Check if the sink was stopped (non-blocking).
    [[nodiscard]] bool was_stopped() const {
        return wait_for_future(stop_future_, std::chrono::milliseconds(0));
    }

    /// Check if the sink was destroyed (non-blocking).
    [[nodiscard]] bool was_destroyed() const {
        return wait_for_future(destruction_future_, std::chrono::milliseconds(0));
    }

    /// Check if the sink is still running (not stopped).
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
    friend class TestSink;

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

    // Buffer collection
    mutable std::mutex mutex_;
    std::condition_variable buffer_cv_;
    std::vector<CapturedBuffer> buffers_;

    // Per-buffer repeat counter map
    std::mutex repeat_mutex_;
    std::unordered_map<void*, uint64_t> repeat_counters_;

public:
    uint64_t get_and_increment_repeat(void* buffer_addr) {
        std::lock_guard<std::mutex> lock(repeat_mutex_);
        return repeat_counters_[buffer_addr]++;
    }

private:
    // Statistics
    std::atomic<size_t> invocations_{0};
    std::atomic<size_t> stop_calls_{0};
};

/// Test sink implementing adaptive_engine::PipelineStage.
///
/// This sink captures all incoming buffers and stores them for test assertions.
/// It is controlled by a TestSinkController which allows tests to wait for
/// buffers and verify lifecycle events.
class TestSink : public PipelineStage {
public:
    /// Create a test sink with a controller.
    /// @param id Sink identifier
    /// @param controller Controller for this sink
    /// @param provider Buffer provider for accessing buffer data
    TestSink(std::string id,
             std::shared_ptr<TestSinkController> controller,
             test::TestBufferProvider* provider)
        : id_(std::move(id))
        , controller_(std::move(controller))
        , provider_(provider) {}

    ~TestSink() override {
        try {
            controller_->destruction_promise_.set_value();
        } catch (const std::future_error&) {
            // Already set, ignore
        }
    }

    TestSink(const TestSink&) = delete;
    TestSink& operator=(const TestSink&) = delete;
    TestSink(TestSink&&) = delete;
    TestSink& operator=(TestSink&&) = delete;

    void start(ExecutionContext& /*ctx*/) override {
        try {
            controller_->start_promise_.set_value();
        } catch (const std::future_error&) {
            // Already set, ignore
        }
    }

    void execute(ExecutionContext& ctx, BufferHandle input) override {
        controller_->invocations_.fetch_add(1);

        // Capture the buffer data
        void* data = provider_->get_data(input);
        size_t size = provider_->get_size(input);
        auto* test_buf = static_cast<test::TestBuffer*>(input.opaque);
        controller_->insert_buffer(CapturedBuffer(data, size, test_buf->metadata));

        // Handle repeat functionality
        size_t max_repeats = controller_->repeat_count.load();
        if (max_repeats > 0) {
            void* buf_addr = provider_->get_data(input);
            uint64_t current_repeat = controller_->get_and_increment_repeat(buf_addr);
            if (current_repeat < max_repeats) {
                ctx.repeat_task();
            }
        }
        // Note: Sink is terminal stage, no emit_buffer call
    }

    void stop(ExecutionContext& ctx) override {
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
    std::shared_ptr<TestSinkController> controller_;
    test::TestBufferProvider* provider_;
};

}  // namespace adaptive_engine::test
