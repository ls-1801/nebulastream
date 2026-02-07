fn main() {
    #[cfg(feature = "cpp-ffi")]
    {
        cxx_build::bridge("src/ffi/mod.rs")
            .flag_if_supported("-std=c++17")
            .compile("adaptive_engine_cxx");
    }

    println!("cargo:rerun-if-changed=src/ffi/mod.rs");
    println!("cargo:rerun-if-changed=src/ffi/engine.rs");
    println!("cargo:rerun-if-changed=src/ffi/callbacks.rs");
}
