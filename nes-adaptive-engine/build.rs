fn main() {
    // Build CXX bridge for FFI module
    cxx_build::bridge("src/ffi/mod.rs")
        .flag_if_supported("-std=c++17")
        .compile("adaptive_engine_cxx");

    // Rerun if FFI sources change
    println!("cargo:rerun-if-changed=src/ffi/mod.rs");
    println!("cargo:rerun-if-changed=src/ffi/buffer.rs");
    println!("cargo:rerun-if-changed=src/ffi/engine.rs");
    println!("cargo:rerun-if-changed=src/ffi/callbacks.rs");
}
