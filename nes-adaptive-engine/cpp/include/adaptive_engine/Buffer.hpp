#pragma once

#include <cstddef>
#include <cstdint>

namespace adaptive_engine {

/// Base class for buffer handle objects. Enables provider-free clone/release
/// via virtual dispatch through the FFI boundary.
struct BufferHandleBase {
    virtual ~BufferHandleBase() = default;
    /// Clone this handle (e.g., copy-construct wrapper, increment refcount).
    virtual BufferHandleBase* do_clone() = 0;
    /// Release this handle. Default: delete this.
    virtual void do_release() { delete this; }
};

/// Opaque handle to a buffer managed via BufferHandleBase virtual dispatch
struct BufferHandle {
    BufferHandleBase* opaque{nullptr};
};

}  // namespace adaptive_engine
