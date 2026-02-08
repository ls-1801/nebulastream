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

//! FFI (Foreign Function Interface) for C++ integration.
//!
//! This module provides the minimal interface for C++ code to interact
//! with the Rust adaptive execution engine. The design follows CXX bridge
//! conventions for safe, zero-cost FFI.
//!
//! # Architecture
//!
//! The FFI layer consists of:
//! - `engine`: EngineHandle wrapping the Rust Executor
//! - `callbacks`: C++ callback wrappers for pipeline stages
//!
//! # Thread Safety
//!
//! All FFI types are designed to be thread-safe:
//! - `EngineHandle` uses internal synchronization
//! - Buffer handles use reference counting
//! - Callbacks are `Send + Sync`

#[cfg(feature = "cpp-ffi")]
pub mod callbacks;
pub mod engine;

// Re-export main FFI types
pub use engine::EngineHandle;

/// Execution statistics for FFI.
///
/// This struct is shared between Rust and C++ via CXX bridge.
#[derive(Debug, Clone, Default)]
#[repr(C)]
pub struct ExecutionStats {
    pub buffers_processed: u64,
    pub bytes_processed: u64,
    pub tasks_executed: u64,
    pub active_queries: u64,
    pub avg_latency_ms: f64,
}

#[cfg(feature = "cpp-ffi")]
#[cxx::bridge(namespace = "adaptive_engine")]
#[allow(clippy::module_inception)]
mod ffi {
    /// Execution statistics returned from the engine.
    #[derive(Debug, Clone, Default)]
    struct FfiExecutionStats {
        pub buffers_processed: u64,
        pub bytes_processed: u64,
        pub tasks_executed: u64,
        pub active_queries: u64,
        pub avg_latency_ms: f64,
    }

    extern "Rust" {
        /// Opaque handle to the Rust execution engine.
        type EngineHandle;

        /// Create a new engine instance.
        ///
        /// # Arguments
        /// * `context_ptr` - Opaque context pointer
        ///
        /// # Returns
        /// Box containing the new EngineHandle
        fn engine_create(context_ptr: usize) -> Box<EngineHandle>;

        /// Start the engine's worker threads.
        ///
        /// # Arguments
        /// * `engine` - The engine handle
        fn engine_start(engine: &EngineHandle);

        /// Shutdown the engine.
        ///
        /// # Arguments
        /// * `engine` - The engine handle
        fn engine_shutdown(engine: &EngineHandle);

        /// Get global engine statistics.
        ///
        /// # Arguments
        /// * `engine` - The engine handle
        ///
        /// # Returns
        /// Current execution statistics
        fn engine_get_stats(engine: &EngineHandle) -> FfiExecutionStats;
    }
}

#[cfg(not(feature = "cpp-ffi"))]
#[allow(clippy::module_inception)]
mod ffi {
    /// Execution statistics returned from the engine (stub when cpp-ffi is disabled).
    #[derive(Debug, Clone, Default)]
    pub struct FfiExecutionStats {
        pub buffers_processed: u64,
        pub bytes_processed: u64,
        pub tasks_executed: u64,
        pub active_queries: u64,
        pub avg_latency_ms: f64,
    }
}

// Re-export FFI functions at module level for CXX bridge
pub use crate::executor::QueryId;
#[cfg(feature = "cpp-ffi")]
pub use engine::engine_submit_query;
pub use engine::{engine_create, engine_get_stats, engine_shutdown, engine_start};

// Re-export the CXX-generated types
pub use ffi::FfiExecutionStats;
