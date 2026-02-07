//! Common test utilities for pipeline testing.
//!
//! This module provides helper functions and utilities for testing
//! pipeline graphs, including buffer generation.

#![allow(dead_code)]

use adaptive_engine::pipeline::Buffer;

/// Generate a sequence of test buffers.
///
/// # Arguments
///
/// * `count` - Number of buffers to generate
/// * `data_size` - Size of data in each buffer (in bytes)
///
/// # Returns
///
/// A vector of buffers with test data.
///
/// # Examples
///
/// ```no_run
/// use tests::common::generate_test_buffers;
///
/// let buffers = generate_test_buffers(10, 8);
/// assert_eq!(buffers.len(), 10);
/// ```
pub fn generate_test_buffers(count: usize, data_size: usize) -> Vec<Buffer> {
    (0..count)
        .map(|i| {
            let data = vec![((i + 1) % 256) as u8; data_size];
            Buffer::new(data)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_test_buffers() {
        let buffers = generate_test_buffers(5, 10);
        assert_eq!(buffers.len(), 5);
        assert_eq!(buffers[0].data().len(), 10);
    }
}
