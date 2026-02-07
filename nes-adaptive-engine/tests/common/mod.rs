//! Common test utilities for pipeline testing.
//!
//! This module provides helper functions and utilities for testing
//! pipeline graphs, including buffer generation, sequence validation,
//! and exactly-once guarantees.

#![allow(dead_code)]

use adaptive_engine::pipeline::Buffer;
use adaptive_engine::sequence::SequenceNumber;
use std::collections::HashSet;

/// Generate a sequence of test buffers with sequential sequence numbers.
///
/// # Arguments
///
/// * `count` - Number of buffers to generate
/// * `data_size` - Size of data in each buffer (in bytes)
/// * `starting_seq` - Starting sequence number
///
/// # Returns
///
/// A vector of buffers with sequential sequence numbers.
///
/// # Examples
///
/// ```no_run
/// use tests::common::generate_test_buffers;
///
/// let buffers = generate_test_buffers(10, 8, 1);
/// assert_eq!(buffers.len(), 10);
/// ```
pub fn generate_test_buffers(count: usize, data_size: usize, starting_seq: u64) -> Vec<Buffer> {
    (0..count)
        .map(|i| {
            let data = vec![((i + 1) % 256) as u8; data_size];
            let seq = SequenceNumber::new(starting_seq + i as u64);
            Buffer::new(data, seq)
        })
        .collect()
}

/// Validate that all sequence numbers in a collection are unique.
///
/// This is useful for testing exactly-once processing guarantees.
///
/// # Arguments
///
/// * `buffers` - Slice of buffers to validate
///
/// # Returns
///
/// `true` if all sequence numbers are unique, `false` otherwise.
///
/// # Examples
///
/// ```no_run
/// use tests::common::{generate_test_buffers, validate_unique_sequences};
///
/// let buffers = generate_test_buffers(10, 8, 1);
/// assert!(validate_unique_sequences(&buffers));
/// ```
pub fn validate_unique_sequences(buffers: &[Buffer]) -> bool {
    let mut seen = HashSet::new();

    for buffer in buffers {
        let seq_str = buffer.sequence().to_string();
        if !seen.insert(seq_str) {
            return false;
        }
    }

    true
}

/// Collect all sequence numbers from a collection of buffers.
///
/// # Arguments
///
/// * `buffers` - Slice of buffers
///
/// # Returns
///
/// A set of sequence number strings.
pub fn collect_sequences(buffers: &[Buffer]) -> HashSet<String> {
    buffers.iter().map(|b| b.sequence().to_string()).collect()
}

/// Validate buffer ordering properties.
///
/// Checks that for buffers with hierarchical sequences (e.g., 1.1, 1.2),
/// the parent sequence appears before all child sequences.
///
/// # Arguments
///
/// * `buffers` - Slice of buffers in execution order
///
/// # Returns
///
/// `true` if ordering is valid, `false` otherwise.
pub fn validate_buffer_ordering(buffers: &[Buffer]) -> bool {
    let mut seen_prefixes = HashSet::new();

    for buffer in buffers {
        let seq = buffer.sequence();
        let components = seq.components();

        // Check that all parent sequences have been seen
        if components.len() > 1 {
            for prefix_len in 1..components.len() {
                let prefix = components[..prefix_len].to_vec();
                let prefix_seq = SequenceNumber::from(prefix);
                let prefix_str = prefix_seq.to_string();

                if !seen_prefixes.contains(&prefix_str) {
                    return false;
                }
            }
        }

        seen_prefixes.insert(seq.to_string());
    }

    true
}

/// Count buffers by sequence number depth.
///
/// # Arguments
///
/// * `buffers` - Slice of buffers
///
/// # Returns
///
/// A vector where index i contains the count of buffers at depth i+1.
pub fn count_by_depth(buffers: &[Buffer]) -> Vec<usize> {
    let max_depth = buffers
        .iter()
        .map(|b| b.sequence().depth())
        .max()
        .unwrap_or(0);

    let mut counts = vec![0; max_depth];

    for buffer in buffers {
        let depth = buffer.sequence().depth();
        if depth > 0 && depth <= max_depth {
            counts[depth - 1] += 1;
        }
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_test_buffers() {
        let buffers = generate_test_buffers(5, 10, 1);
        assert_eq!(buffers.len(), 5);
        assert_eq!(buffers[0].data().len(), 10);
        assert_eq!(buffers[0].sequence().to_string(), "1");
        assert_eq!(buffers[4].sequence().to_string(), "5");
    }

    #[test]
    fn test_validate_unique_sequences() {
        let buffers = generate_test_buffers(5, 10, 1);
        assert!(validate_unique_sequences(&buffers));

        // Create duplicate
        let mut with_duplicate = buffers.clone();
        with_duplicate.push(buffers[0].clone());
        assert!(!validate_unique_sequences(&with_duplicate));
    }

    #[test]
    fn test_collect_sequences() {
        let buffers = generate_test_buffers(3, 10, 1);
        let sequences = collect_sequences(&buffers);

        assert_eq!(sequences.len(), 3);
        assert!(sequences.contains("1"));
        assert!(sequences.contains("2"));
        assert!(sequences.contains("3"));
    }

    #[test]
    fn test_count_by_depth() {
        let mut buffers = Vec::new();
        buffers.push(Buffer::new(vec![1], SequenceNumber::new(1)));
        buffers.push(Buffer::new(vec![2], SequenceNumber::new(1).child(1)));
        buffers.push(Buffer::new(vec![3], SequenceNumber::new(1).child(2)));
        buffers.push(Buffer::new(
            vec![4],
            SequenceNumber::new(1).child(1).child(1),
        ));

        let counts = count_by_depth(&buffers);
        assert_eq!(counts[0], 1); // Depth 1: 1 buffer
        assert_eq!(counts[1], 2); // Depth 2: 2 buffers
        assert_eq!(counts[2], 1); // Depth 3: 1 buffer
    }
}
