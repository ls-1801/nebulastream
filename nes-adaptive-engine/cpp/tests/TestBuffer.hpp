#pragma once

#include <adaptive_engine/Buffer.hpp>

#include <atomic>
#include <cstring>
#include <memory>
#include <mutex>
#include <set>
#include <vector>

namespace adaptive_engine::test {

/// Test-only metadata for buffer tracking in test assertions.
/// This is NOT part of the adaptive engine public API.
struct TestBufferMetadata {
    uint64_t sequence_number{0};
    uint64_t origin_id{0};
    uint64_t watermark{0};
    uint64_t num_tuples{0};
    uint32_t chunk_number{0};
    bool last_chunk{false};
};

/// Test buffer implementation for C++ tests without NES dependencies.
/// Inherits BufferHandleBase for provider-free clone/release via virtual dispatch.
struct TestBuffer : BufferHandleBase {
    std::vector<uint8_t> data;
    TestBufferMetadata metadata;
    std::atomic<int> ref_count{1};

    TestBuffer(size_t size, const TestBufferMetadata& meta)
        : data(size), metadata(meta) {}

    TestBuffer(const void* src, size_t size, const TestBufferMetadata& meta)
        : data(size), metadata(meta) {
        if (src != nullptr && size > 0) {
            std::memcpy(data.data(), src, size);
        }
    }

    BufferHandleBase* do_clone() override {
        ref_count.fetch_add(1);
        return this;
    }

    void do_release() override {
        if (ref_count.fetch_sub(1) == 1) {
            delete this;
        }
    }
};

/// Test implementation of buffer management for C++ tests.
/// Standalone utility (not a BufferProvider subclass). Tracks active
/// buffers for leak detection in tests.
class TestBufferProvider {
public:
    TestBufferProvider() = default;
    ~TestBufferProvider() = default;

    /// Wrap an existing buffer with the provider.
    /// Creates a copy of the data for test isolation.
    BufferHandle wrap(void* data, size_t size, const TestBufferMetadata& metadata) {
        auto* buffer = new TestBuffer(data, size, metadata);
        BufferHandle handle{buffer};

        std::lock_guard<std::mutex> lock(mutex_);
        active_buffers_.insert(buffer);
        buffer->ref_count.store(1);
        return handle;
    }

    /// Get the data pointer for a buffer.
    void* get_data(BufferHandle handle) {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->data.data() : nullptr;
    }

    /// Get the size of a buffer.
    size_t get_size(BufferHandle handle) {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->data.size() : 0;
    }

    /// Allocate a new buffer of the specified size.
    BufferHandle allocate(size_t size) {
        TestBufferMetadata empty_metadata{};
        auto* buffer = new TestBuffer(size, empty_metadata);
        BufferHandle handle{buffer};

        std::lock_guard<std::mutex> lock(mutex_);
        active_buffers_.insert(buffer);
        return handle;
    }

    // Test utilities

    /// Get the number of active buffers (useful for leak detection in tests).
    /// Note: This checks which tracked buffers still have ref_count > 0.
    size_t active_buffer_count() const {
        std::lock_guard<std::mutex> lock(mutex_);
        size_t count = 0;
        for (auto* buf : active_buffers_) {
            if (buf->ref_count.load() > 0) {
                ++count;
            }
        }
        return count;
    }

    /// Get the reference count of a buffer (for test assertions).
    int get_ref_count(BufferHandle handle) const {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->ref_count.load() : 0;
    }

private:
    mutable std::mutex mutex_;
    std::set<TestBuffer*> active_buffers_;
};

}  // namespace adaptive_engine::test
