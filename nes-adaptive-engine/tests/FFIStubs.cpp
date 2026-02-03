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

/// Stub implementations of FFI functions expected by adaptive-engine.
/// These are only for unit testing basic stage construction without full integration.

#include <cstdint>

extern "C"
{
    // Pipeline callback stubs
    int32_t pipeline_callback_setup(uintptr_t /*context*/, const void* /*exec_context*/) { return 0; }
    void pipeline_callback_teardown(uintptr_t /*context*/, const void* /*exec_context*/) {}
    void* pipeline_callback_execute(const void* /*input_buffer*/, const void* /*exec_context*/, uintptr_t /*context*/)
    {
        return nullptr;
    }
    void* pipeline_callback_flush(const void* /*exec_context*/, uintptr_t /*context*/) { return nullptr; }

    // TupleBuffer stubs
    void tuple_buffer_retain(void* /*handle*/) {}
    void tuple_buffer_release(void* /*handle*/) {}
    const uint8_t* tuple_buffer_get_data(const void* /*handle*/) { return nullptr; }
    uint32_t tuple_buffer_get_size(const void* /*handle*/) { return 0; }
    uint64_t tuple_buffer_get_origin_id(const void* /*handle*/) { return 0; }
    uint64_t tuple_buffer_get_watermark(const void* /*handle*/) { return 0; }
    uint64_t tuple_buffer_get_number_of_tuples(const void* /*handle*/) { return 0; }
    uint32_t tuple_buffer_get_refcount(const void* /*handle*/) { return 1; }
}
