# NebulaStream Development Guide

## CMake Presets

This project uses CMake presets for environment-aware configuration. Presets are filtered based on environment variables set by the devcontainer.

### Environment Variables

- `NES_STDLIB`: The C++ standard library (`libcxx` or `libstdcxx`)
- `NES_SANITIZER`: The sanitizer to use (`none`, `address`, `thread`, or `undefined`)

### Available Presets

Presets follow the naming pattern: `<stdlib>-<sanitizer>-<buildtype>`

Examples:
- `libcxx-none-debug` - libc++, no sanitizer, Debug build
- `libcxx-asan-release` - libc++, AddressSanitizer, Release build
- `libstdcxx-tsan-relwithdebinfo` - libstdc++, ThreadSanitizer, RelWithDebInfo build

### Build Directory Naming

Build directories follow the pattern: `build-<stdlib>-<sanitizer>-<buildtype>`

Examples:
- `build-libcxx-none-debug`
- `build-libcxx-asan-release`
- `build-libstdcxx-none-relwithdebinfo`

### Usage

List available presets for your current environment:
```bash
cmake --list-presets
```

Configure with a preset:
```bash
cmake --preset libcxx-none-debug
```

Build with a preset:
```bash
cmake --build --preset libcxx-none-debug
```

Run tests with a preset:
```bash
ctest --preset libcxx-none-debug
```

### Testing Preset Filtering

To verify presets are filtered correctly:
```bash
export NES_STDLIB=libcxx NES_SANITIZER=none
cmake --list-presets
# Should only show libcxx-none-* presets
```
