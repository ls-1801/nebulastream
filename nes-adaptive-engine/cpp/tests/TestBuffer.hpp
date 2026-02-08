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

#include <atomic>
#include <cstring>
#include <vector>

namespace adaptive_engine::test
{

/// Test-only metadata for buffer tracking in test assertions.
/// This is NOT part of the adaptive engine public API.
struct TestBufferMetadata
{
    uint64_t sequence_number{0};
    uint64_t origin_id{0};
    uint64_t watermark{0};
    uint64_t num_tuples{0};
    uint32_t chunk_number{0};
    bool last_chunk{false};
};

/// Test buffer implementation for C++ tests without NES dependencies.
/// Inherits BufferHandleBase for provider-free clone/release via virtual dispatch.
struct TestBuffer : BufferHandleBase
{
    std::vector<uint8_t> data;
    TestBufferMetadata metadata;
    std::atomic<int> ref_count{1};
    std::atomic<int>* active_counter{nullptr};

    TestBuffer(size_t size, const TestBufferMetadata& meta, std::atomic<int>* counter = nullptr)
        : data(size), metadata(meta), active_counter(counter)
    {
    }

    TestBuffer(const void* src, size_t size, const TestBufferMetadata& meta, std::atomic<int>* counter = nullptr)
        : data(size), metadata(meta), active_counter(counter)
    {
        if (src != nullptr && size > 0)
        {
            std::memcpy(data.data(), src, size);
        }
    }

    BufferHandleBase* do_clone() override
    {
        ref_count.fetch_add(1);
        return this;
    }

    void do_release() override
    {
        if (ref_count.fetch_sub(1) == 1)
        {
            if (active_counter)
            {
                active_counter->fetch_sub(1);
            }
            delete this;
        }
    }
};

/// Test implementation of buffer management for C++ tests.
/// Standalone utility (not a BufferProvider subclass). Tracks active
/// buffers for leak detection in tests via an atomic counter.
class TestBufferProvider
{
public:
    TestBufferProvider() = default;
    ~TestBufferProvider() = default;

    /// Wrap an existing buffer with the provider.
    /// Creates a copy of the data for test isolation.
    BufferHandle wrap(void* data, size_t size, const TestBufferMetadata& metadata)
    {
        active_count_.fetch_add(1);
        auto* buffer = new TestBuffer(data, size, metadata, &active_count_);
        BufferHandle handle{buffer};
        return handle;
    }

    /// Get the data pointer for a buffer.
    void* get_data(BufferHandle handle)
    {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->data.data() : nullptr;
    }

    /// Get the size of a buffer.
    size_t get_size(BufferHandle handle)
    {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->data.size() : 0;
    }

    /// Allocate a new buffer of the specified size.
    BufferHandle allocate(size_t size)
    {
        active_count_.fetch_add(1);
        TestBufferMetadata empty_metadata{};
        auto* buffer = new TestBuffer(size, empty_metadata, &active_count_);
        BufferHandle handle{buffer};
        return handle;
    }

    /// Test utilities

    /// Get the number of active buffers (useful for leak detection in tests).
    size_t active_buffer_count() const { return static_cast<size_t>(active_count_.load()); }

    /// Get the reference count of a buffer (for test assertions).
    int get_ref_count(BufferHandle handle) const
    {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->ref_count.load() : 0;
    }

private:
    std::atomic<int> active_count_{0};
};

} /// namespace adaptive_engine::test
