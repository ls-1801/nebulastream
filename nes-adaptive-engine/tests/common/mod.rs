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

//! Common test utilities for pipeline testing.
//!
//! This module provides helper functions and utilities for testing
//! pipeline graphs, including buffer generation.

#![allow(dead_code)]

pub mod capturing_sink;
pub mod controlled_pipeline;
pub mod controlled_source;
pub mod stats_collector;

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
