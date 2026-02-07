# CMakeLists.txt for adaptive-engine C++ interface
#
# This project uses Corrosion to integrate with the Rust adaptive-engine crate.
# C++ source files (FFI callbacks, Engine wrapper) are added as INTERFACE
# sources to the Corrosion target, so consumers compile them directly -
# no circular linking issues.

cmake_minimum_required(VERSION 3.24)
project(adaptive_engine_cpp VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Fetch Corrosion for Rust integration
include(FetchContent)
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5
)
FetchContent_MakeAvailable(Corrosion)

# Import the Rust crate
corrosion_import_crate(MANIFEST_PATH ${CMAKE_CURRENT_SOURCE_DIR}/../Cargo.toml FEATURES cpp-ffi)

# Add C++ sources as INTERFACE - consumers compile them alongside the Rust lib,
# ensuring FFI callback symbols are available without circular linking.
target_sources(adaptive_engine INTERFACE
    ${CMAKE_CURRENT_SOURCE_DIR}/src/Engine.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/src/FfiCallbacks.cpp
)

target_include_directories(adaptive_engine INTERFACE
    ${CMAKE_CURRENT_SOURCE_DIR}/include
)

# Platform-specific link dependencies for Rust runtime
if(UNIX AND NOT APPLE)
    target_link_libraries(adaptive_engine INTERFACE
        pthread
        dl
    )
elseif(APPLE)
    target_link_libraries(adaptive_engine INTERFACE
        pthread
    )
endif()

# Add tests subdirectory
if(NES_ENABLES_TESTS)
    enable_testing()
    add_subdirectory(tests)
endif()
