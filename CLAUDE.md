# NebulaStream Build Instructions

## Development Container

This project is designed to be built inside the development container. The container is detected by the presence of the `NES_PREBUILT_VCPKG_ROOT` environment variable. When this variable is set, all dependencies (vcpkg, Clang, Ninja, mold, ccache, Rust, etc.) are already installed and the CMake presets are available.

**The CMake presets will only work when `NES_PREBUILT_VCPKG_ROOT` is set.** If you are not inside the dev container, the presets are disabled.

## Building

```bash
# Configure (debug)
cmake --preset debug

# Build everything
cmake --build --preset debug -j$(nproc)

# Build a specific target
cmake --build --preset debug --target QueryEngineTest

# Configure + build (relwithdebinfo)
cmake --preset relwithdebinfo
cmake --build --preset relwithdebinfo -j$(nproc)
```

Build outputs:
- Debug: `build-debug/`
- RelWithDebInfo: `build-relwithdebinfo/`

## Running Tests

```bash
# Run all tests
ctest --test-dir build-debug --output-on-failure

# Run QueryEngineTest (adaptive engine)
ctest --test-dir build-debug -R QueryEngineTest --output-on-failure

# Run systests
build-debug/nes-systests/systest/systest --sequential \
    -t nes-systests/tuples/OneTuple.test \
    -- \
    --worker.default_query_execution.execution_mode=INTERPRETER \
    --worker.default_query_execution.join_strategy=NESTED_LOOP_JOIN
```

## Quality Gate

Run `./quality_gate.sh` to execute all quality checks. It runs Rust checks (including unit tests), the NES build, QueryEngineTest, and systests.

## Adaptive Engine (Rust)

The adaptive engine Rust code can be checked independently for faster iteration:

```bash
cargo check --manifest-path nes-adaptive-engine/Cargo.toml
cargo clippy --manifest-path nes-adaptive-engine/Cargo.toml -- -D warnings
cargo fmt --manifest-path nes-adaptive-engine/Cargo.toml --check
cargo test --manifest-path nes-adaptive-engine/Cargo.toml
```

This runs all Rust tests: library unit tests, integration tests, and doc-tests.

The C++ tests (QueryEngineTest) are built as part of the NES root CMake system.
