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

#include <cstddef>
#include <cstdint>

namespace adaptive_engine
{

/// Base class for buffer handle objects. Enables provider-free clone/release
/// via virtual dispatch through the FFI boundary.
struct BufferHandleBase
{
    virtual ~BufferHandleBase() = default;
    /// Clone this handle (e.g., copy-construct wrapper, increment refcount).
    virtual BufferHandleBase* do_clone() = 0;

    /// Release this handle. Default: delete this.
    virtual void do_release() { delete this; }
};

/// Opaque handle to a buffer managed via BufferHandleBase virtual dispatch
struct BufferHandle
{
    BufferHandleBase* opaque{nullptr};
};

} /// namespace adaptive_engine
