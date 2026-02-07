//! Buffer types for FFI.
//!
//! This module defines buffer types that match the C++ Buffer.hpp interface,
//! enabling zero-copy buffer passing between Rust and C++ code.

/// Metadata associated with a buffer, matching C++ BufferMetadata.
#[derive(Debug, Clone, Default)]
pub struct BufferMetadata {
    /// Sequence number for ordering
    pub sequence_number: u64,
    /// Identifier for the data source
    pub origin_id: u64,
    /// Watermark timestamp for event-time processing
    pub watermark: u64,
    /// Number of tuples in the buffer
    pub num_tuples: u64,
    /// Current chunk number in a multi-chunk transfer
    pub chunk_number: u32,
    /// True if this is the last chunk
    pub last_chunk: bool,
}

/// Opaque handle to a buffer managed by the C++ BufferProvider.
///
/// This struct wraps a C++ buffer handle and ensures proper cleanup
/// when the Rust side is done with the buffer.
pub struct BufferHandle {
    /// Opaque pointer to provider-specific buffer data (from C++ BufferHandle)
    opaque: usize,
    /// Pointer to the C++ BufferProvider for release callback
    provider_ptr: usize,
    /// Cached data pointer for fast access
    data_ptr: *const u8,
    /// Cached size for fast access
    size: usize,
    /// Buffer metadata
    metadata: BufferMetadata,
}

// SAFETY: BufferHandle is Send because:
// - The opaque handle is just an identifier
// - The provider_ptr is thread-safe (C++ BufferProvider is thread-safe)
// - The data_ptr is read-only and the buffer is owned
unsafe impl Send for BufferHandle {}

// SAFETY: BufferHandle is Sync because:
// - All access is read-only (no mutable state)
// - C++ BufferProvider is thread-safe
unsafe impl Sync for BufferHandle {}

impl BufferHandle {
    /// Create a new buffer handle from C++ components.
    ///
    /// # Arguments
    /// * `opaque` - Opaque pointer from C++ BufferHandle
    /// * `provider_ptr` - Pointer to C++ BufferProvider
    /// * `data_ptr` - Pointer to buffer data
    /// * `size` - Size of buffer in bytes
    /// * `metadata` - Buffer metadata
    ///
    /// # Safety
    /// The caller must ensure:
    /// - `opaque` is a valid handle from the given provider
    /// - `provider_ptr` points to a valid BufferProvider
    /// - `data_ptr` is valid for `size` bytes
    /// - The buffer will remain valid until this handle is dropped
    pub unsafe fn new(
        opaque: usize,
        provider_ptr: usize,
        data_ptr: *const u8,
        size: usize,
        metadata: BufferMetadata,
    ) -> Self {
        Self {
            opaque,
            provider_ptr,
            data_ptr,
            size,
            metadata,
        }
    }

    /// Get the opaque handle value.
    pub fn opaque(&self) -> usize {
        self.opaque
    }

    /// Get a slice of the buffer data.
    ///
    /// # Safety
    /// The buffer data is valid for the lifetime of this handle.
    pub fn data(&self) -> &[u8] {
        if self.data_ptr.is_null() || self.size == 0 {
            &[]
        } else {
            // SAFETY: data_ptr and size are set by the C++ provider and
            // guaranteed to be valid for the lifetime of this handle
            unsafe { std::slice::from_raw_parts(self.data_ptr, self.size) }
        }
    }

    /// Get the size of the buffer in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get the buffer metadata.
    pub fn metadata(&self) -> &BufferMetadata {
        &self.metadata
    }

    /// Get the provider pointer.
    pub fn provider_ptr(&self) -> usize {
        self.provider_ptr
    }
}

impl Drop for BufferHandle {
    fn drop(&mut self) {
        if self.provider_ptr != 0 && self.opaque != 0 {
            // Call C++ BufferProvider::release via FFI
            // SAFETY: provider_ptr was validated at construction time
            unsafe {
                buffer_provider_release(self.provider_ptr, self.opaque);
            }
        }
    }
}

// External C functions for buffer provider callbacks
extern "C" {
    /// Release a buffer handle back to the provider.
    ///
    /// # Safety
    /// - `provider_ptr` must point to a valid BufferProvider
    /// - `handle` must be a valid handle from that provider
    fn buffer_provider_release(provider_ptr: usize, handle: usize);
}

/// Convert a Rust Buffer to an FFI BufferHandle.
///
/// This is used when the Rust side needs to pass a buffer to C++.
/// Note: This creates a copy since Rust Buffers own their data.
impl From<&crate::pipeline::Buffer> for BufferMetadata {
    fn from(buffer: &crate::pipeline::Buffer) -> Self {
        Self {
            sequence_number: buffer.sequence().as_u64(),
            origin_id: buffer.origin_id().unwrap_or(0),
            watermark: buffer.watermark().unwrap_or(0),
            num_tuples: buffer.number_of_tuples(),
            chunk_number: buffer.chunk_number().unwrap_or(0) as u32,
            last_chunk: buffer.is_last_chunk(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_default() {
        let meta = BufferMetadata::default();
        assert_eq!(meta.sequence_number, 0);
        assert_eq!(meta.origin_id, 0);
        assert_eq!(meta.watermark, 0);
        assert_eq!(meta.num_tuples, 0);
        assert_eq!(meta.chunk_number, 0);
        assert!(!meta.last_chunk);
    }

    #[test]
    fn test_metadata_from_buffer() {
        use crate::pipeline::Buffer;
        use crate::sequence::SequenceNumber;

        let buffer = Buffer::new(vec![1, 2, 3], SequenceNumber::new(42))
            .with_origin(100)
            .with_watermark(1000)
            .with_tuple_count(5)
            .with_chunk_info(2, true);

        let meta = BufferMetadata::from(&buffer);
        assert_eq!(meta.sequence_number, 42);
        assert_eq!(meta.origin_id, 100);
        assert_eq!(meta.watermark, 1000);
        assert_eq!(meta.num_tuples, 5);
        assert_eq!(meta.chunk_number, 2);
        assert!(meta.last_chunk);
    }
}
