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

#include <adaptive_engine/Buffer.hpp>
#include <adaptive_engine/ExecutionContext.hpp>
#include <adaptive_engine/SourceHandle.hpp>
#include "TestBuffer.hpp"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <deque>
#include <future>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
#include <variant>
#include <vector>

namespace adaptive_engine::test
{

/// Controllable test source for C++ tests without NES dependencies.
///
/// TestSourceController is the control interface for injecting data, errors,
/// and end-of-stream signals into a TestSource. It also provides lifecycle
/// synchronization via promises/futures.
///
/// Usage:
/// ```cpp
/// auto controller = std::make_shared<TestSourceController>();
/// auto source = std::make_unique<TestSource>("source-1", controller, &buffer_provider);
///
/// /// Inject data
/// controller->inject_data({0x01, 0x02}, 10);  /// 10 tuples
/// controller->inject_eos();  /// Signal end of stream
///
/// /// Wait for lifecycle events
/// ASSERT_TRUE(controller->wait_until_opened());
/// ASSERT_TRUE(controller->wait_until_closed());
/// ```
class TestSourceController
{
public:
    static constexpr std::chrono::milliseconds DEFAULT_TIMEOUT{10000};

    TestSourceController() = default;
    ~TestSourceController() = default;

    TestSourceController(const TestSourceController&) = delete;
    TestSourceController& operator=(const TestSourceController&) = delete;
    TestSourceController(TestSourceController&&) = delete;
    TestSourceController& operator=(TestSourceController&&) = delete;

    /// Inject data into the source. The data will be returned by next_buffer().
    /// @param data Raw buffer data to inject
    /// @param num_tuples Number of tuples in this buffer
    /// @return true if data was successfully queued, false if source is already closed
    bool inject_data(std::vector<uint8_t> data, size_t num_tuples)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (closed_)
        {
            return false;
        }
        queue_.emplace_back(Data{std::move(data), num_tuples, next_sequence_number_++});
        queue_cv_.notify_one();
        return true;
    }

    /// Inject data with custom metadata.
    /// @param data Raw buffer data to inject
    /// @param metadata Buffer metadata
    /// @return true if data was successfully queued, false if source is already closed
    bool inject_data_with_metadata(std::vector<uint8_t> data, const TestBufferMetadata& metadata)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (closed_)
        {
            return false;
        }
        queue_.emplace_back(DataWithMetadata{std::move(data), metadata});
        queue_cv_.notify_one();
        return true;
    }

    /// Inject an end-of-stream signal. After this, next_buffer() will return nullopt.
    /// @return true if EOS was successfully queued, false if source is already closed
    bool inject_eos()
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (closed_)
        {
            return false;
        }
        queue_.emplace_back(EoS{});
        queue_cv_.notify_one();
        return true;
    }

    /// Inject an error. After this, next_buffer() will throw an exception.
    /// @param error Error message
    /// @return true if error was successfully queued, false if source is already closed
    bool inject_error(std::string error)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (closed_)
        {
            return false;
        }
        failed_ = true;
        queue_.emplace_back(Error{std::move(error)});
        queue_cv_.notify_one();
        return true;
    }

    /// Configure the source to fail after N buffers have been emitted.
    /// @param n Number of buffers to emit before failing
    /// @param error Error message for the failure
    void fail_after_n(size_t n, std::string error = "Source failure")
    {
        fail_after_n_ = n;
        fail_error_ = std::move(error);
    }

    /// Configure the source to fail during open().
    /// @param block_for Time to block before failing
    void fail_during_open(std::chrono::milliseconds block_for = std::chrono::milliseconds(0))
    {
        fail_during_open_ = true;
        fail_during_open_duration_ = block_for;
    }

    /// Configure the source to fail during close().
    /// @param block_for Time to block before failing
    void fail_during_close(std::chrono::milliseconds block_for = std::chrono::milliseconds(0))
    {
        fail_during_close_ = true;
        fail_during_close_duration_ = block_for;
    }

    /// Wait until the source has been opened.
    /// @param timeout Maximum time to wait
    /// @return true if opened, false if timeout
    [[nodiscard]] bool wait_until_opened(std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const
    {
        return wait_for_future(open_future_, timeout);
    }

    /// Wait until the source has been closed.
    /// @param timeout Maximum time to wait
    /// @return true if closed, false if timeout
    [[nodiscard]] bool wait_until_closed(std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const
    {
        return wait_for_future(close_future_, timeout);
    }

    /// Wait until the source has been destroyed.
    /// @param timeout Maximum time to wait
    /// @return true if destroyed, false if timeout
    [[nodiscard]] bool wait_until_destroyed(std::chrono::milliseconds timeout = DEFAULT_TIMEOUT) const
    {
        return wait_for_future(destroy_future_, timeout);
    }

    /// Check if the source was opened (non-blocking).
    [[nodiscard]] bool was_opened() const { return wait_for_future(open_future_, std::chrono::milliseconds(0)); }

    /// Check if the source was closed (non-blocking).
    [[nodiscard]] bool was_closed() const { return wait_for_future(close_future_, std::chrono::milliseconds(0)); }

    /// Check if the source was destroyed (non-blocking).
    [[nodiscard]] bool was_destroyed() const { return wait_for_future(destroy_future_, std::chrono::milliseconds(0)); }

    /// Check if the source failed.
    [[nodiscard]] bool has_failed() const { return failed_.load(); }

    /// Get the number of buffers emitted by this source.
    [[nodiscard]] size_t buffers_emitted() const { return buffers_emitted_.load(); }

private:
    friend class TestSource;

    struct EoS
    {
    };

    struct Data
    {
        std::vector<uint8_t> data;
        size_t num_tuples;
        uint64_t sequence_number;
    };

    struct DataWithMetadata
    {
        std::vector<uint8_t> data;
        TestBufferMetadata metadata;
    };

    struct Error
    {
        std::string message;
    };

    using QueueItem = std::variant<EoS, Data, DataWithMetadata, Error>;

    static bool wait_for_future(const std::shared_future<void>& fut, std::chrono::milliseconds timeout)
    {
        return fut.wait_for(timeout) == std::future_status::ready;
    }

    /// Lifecycle promises/futures
    std::promise<void> open_promise_;
    std::promise<void> close_promise_;
    std::promise<void> destroy_promise_;
    std::shared_future<void> open_future_{open_promise_.get_future().share()};
    std::shared_future<void> close_future_{close_promise_.get_future().share()};
    std::shared_future<void> destroy_future_{destroy_promise_.get_future().share()};

    /// Queue for data injection
    mutable std::mutex mutex_;
    std::condition_variable queue_cv_;
    std::deque<QueueItem> queue_;
    bool closed_{false};
    uint64_t next_sequence_number_{0};

    /// Failure configuration
    std::atomic<bool> failed_{false};
    std::atomic<size_t> fail_after_n_{SIZE_MAX};
    std::string fail_error_{"Source failure"};
    bool fail_during_open_{false};
    bool fail_during_close_{false};
    std::chrono::milliseconds fail_during_open_duration_{0};
    std::chrono::milliseconds fail_during_close_duration_{0};

    /// Statistics
    std::atomic<size_t> buffers_emitted_{0};
};

/// Test source implementing adaptive_engine::SourceHandle.
///
/// This source is controlled by a TestSourceController which allows tests
/// to inject data, errors, and end-of-stream signals.
class TestSource : public SourceHandle
{
public:
    /// Create a test source with a controller.
    /// @param id Source identifier
    /// @param controller Controller for this source
    /// @param provider Buffer provider for wrapping injected data
    TestSource(std::string id, std::shared_ptr<TestSourceController> controller, test::TestBufferProvider* provider)
        : id_(std::move(id)), controller_(std::move(controller)), provider_(provider)
    {
    }

    ~TestSource() override
    {
        try
        {
            controller_->destroy_promise_.set_value();
        }
        catch (const std::future_error&)
        {
            /// Already set, ignore
        }
    }

    TestSource(const TestSource&) = delete;
    TestSource& operator=(const TestSource&) = delete;
    TestSource(TestSource&&) = delete;
    TestSource& operator=(TestSource&&) = delete;

    void open(ExecutionContext& /*ctx*/) override
    {
        try
        {
            controller_->open_promise_.set_value();
        }
        catch (const std::future_error&)
        {
            /// Already set, ignore
        }

        if (controller_->fail_during_open_)
        {
            if (controller_->fail_during_open_duration_.count() > 0)
            {
                std::this_thread::sleep_for(controller_->fail_during_open_duration_);
            }
            throw std::runtime_error("Source open failed");
        }
    }

    std::optional<BufferHandle> next_buffer(ExecutionContext& /*ctx*/) override
    {
        /// Check fail_after_n
        if (controller_->buffers_emitted_.load() >= controller_->fail_after_n_.load())
        {
            controller_->failed_ = true;
            throw std::runtime_error(controller_->fail_error_);
        }

        /// Wait for next item in queue, close signal, or stop request
        std::unique_lock<std::mutex> lock(controller_->mutex_);
        while (controller_->queue_.empty() && !controller_->closed_ && !stop_requested_)
        {
            controller_->queue_cv_.wait(lock);
        }

        /// If (closed or stopped) and queue empty, return nullopt (source exhausted)
        if (controller_->queue_.empty())
        {
            return std::nullopt;
        }

        auto item = std::move(controller_->queue_.front());
        controller_->queue_.pop_front();

        /// Process item
        return std::visit(
            [this](auto&& arg) -> std::optional<BufferHandle>
            {
                using T = std::decay_t<decltype(arg)>;

                if constexpr (std::is_same_v<T, TestSourceController::EoS>)
                {
                    return std::nullopt;
                }
                else if constexpr (std::is_same_v<T, TestSourceController::Error>)
                {
                    controller_->failed_ = true;
                    throw std::runtime_error(arg.message);
                }
                else if constexpr (std::is_same_v<T, TestSourceController::Data>)
                {
                    /// TestBufferMetadata field order: sequence_number, origin_id, watermark, num_tuples, chunk_number, last_chunk
                    TestBufferMetadata metadata{
                        arg.sequence_number, /// sequence_number
                        0, /// origin_id (can be set by test)
                        0, /// watermark
                        arg.num_tuples, /// num_tuples
                        0, /// chunk_number
                        false /// last_chunk
                    };
                    controller_->buffers_emitted_.fetch_add(1);
                    return provider_->wrap(const_cast<void*>(static_cast<const void*>(arg.data.data())), arg.data.size(), metadata);
                }
                else if constexpr (std::is_same_v<T, TestSourceController::DataWithMetadata>)
                {
                    controller_->buffers_emitted_.fetch_add(1);
                    return provider_->wrap(const_cast<void*>(static_cast<const void*>(arg.data.data())), arg.data.size(), arg.metadata);
                }
            },
            item);
    }

    void close(ExecutionContext& /*ctx*/) override
    {
        {
            std::lock_guard<std::mutex> lock(controller_->mutex_);
            controller_->closed_ = true;
        }
        /// Wake up next_buffer() if it's waiting on the condition variable
        controller_->queue_cv_.notify_all();

        try
        {
            controller_->close_promise_.set_value();
        }
        catch (const std::future_error&)
        {
            /// Already set, ignore
        }

        if (controller_->fail_during_close_)
        {
            if (controller_->fail_during_close_duration_.count() > 0)
            {
                std::this_thread::sleep_for(controller_->fail_during_close_duration_);
            }
            throw std::runtime_error("Source close failed");
        }
    }

    void request_stop() override
    {
        {
            std::lock_guard<std::mutex> lock(controller_->mutex_);
            stop_requested_ = true;
        }
        /// Wake up next_buffer() if it's waiting on the condition variable
        controller_->queue_cv_.notify_all();
    }

    [[nodiscard]] std::string get_id() const override { return id_; }

private:
    std::string id_;
    std::shared_ptr<TestSourceController> controller_;
    test::TestBufferProvider* provider_;
    bool stop_requested_{false};
};

} /// namespace adaptive_engine::test
