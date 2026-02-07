#pragma once

#include <adaptive_engine/Buffer.hpp>

#include <atomic>
#include <cstring>
#include <memory>
#include <mutex>
#include <unordered_map>
#include <vector>

namespace adaptive_engine::test {

/// Test buffer implementation for C++ tests without NES dependencies.
/// Stores data in a vector with reference counting for lifecycle management.
struct TestBuffer {
    std::vector<uint8_t> data;
    BufferMetadata metadata;
    std::atomic<int> ref_count{1};

    TestBuffer(size_t size, const BufferMetadata& meta)
        : data(size), metadata(meta) {}

    TestBuffer(const void* src, size_t size, const BufferMetadata& meta)
        : data(size), metadata(meta) {
        if (src != nullptr && size > 0) {
            std::memcpy(data.data(), src, size);
        }
    }
};

/// Test implementation of BufferProvider for C++ tests.
/// Uses shared_ptr internally for memory management and tracks buffers
/// in a map for lifecycle verification in tests.
class TestBufferProvider : public BufferProvider {
public:
    TestBufferProvider() = default;
    ~TestBufferProvider() override = default;

    /// Wrap an existing buffer with the provider.
    /// Creates a copy of the data for test isolation.
    BufferHandle wrap(void* data, size_t size, const BufferMetadata& metadata) override {
        auto buffer = std::make_shared<TestBuffer>(data, size, metadata);
        BufferHandle handle{buffer.get()};

        std::lock_guard<std::mutex> lock(mutex_);
        buffers_[buffer.get()] = std::move(buffer);
        return handle;
    }

    /// Release a buffer handle, freeing associated resources.
    void release(BufferHandle handle) override {
        auto* ptr = static_cast<TestBuffer*>(handle.opaque);
        if (ptr == nullptr) {
            return;
        }

        // Decrement ref count
        int old_count = ptr->ref_count.fetch_sub(1);
        if (old_count == 1) {
            // Last reference - remove from map
            std::lock_guard<std::mutex> lock(mutex_);
            buffers_.erase(ptr);
        }
    }

    /// Get the data pointer for a buffer.
    void* get_data(BufferHandle handle) override {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->data.data() : nullptr;
    }

    /// Get the size of a buffer.
    size_t get_size(BufferHandle handle) override {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->data.size() : 0;
    }

    /// Get the metadata for a buffer.
    const BufferMetadata& get_metadata(BufferHandle handle) override {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer->metadata;
    }

    /// Allocate a new buffer of the specified size.
    BufferHandle allocate(size_t size) override {
        BufferMetadata empty_metadata{0, 0, 0, 0, 0, false};
        auto buffer = std::make_shared<TestBuffer>(size, empty_metadata);
        BufferHandle handle{buffer.get()};

        std::lock_guard<std::mutex> lock(mutex_);
        buffers_[buffer.get()] = std::move(buffer);
        return handle;
    }

    // Test utilities

    /// Get the number of active buffers (useful for leak detection in tests).
    size_t active_buffer_count() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return buffers_.size();
    }

    /// Increment the reference count on a buffer handle.
    /// Used when passing buffers to multiple consumers.
    void add_ref(BufferHandle handle) {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        if (buffer != nullptr) {
            buffer->ref_count.fetch_add(1);
        }
    }

    /// Get the reference count of a buffer (for test assertions).
    int get_ref_count(BufferHandle handle) const {
        auto* buffer = static_cast<TestBuffer*>(handle.opaque);
        return buffer ? buffer->ref_count.load() : 0;
    }

private:
    mutable std::mutex mutex_;
    std::unordered_map<TestBuffer*, std::shared_ptr<TestBuffer>> buffers_;
};

}  // namespace adaptive_engine::test
