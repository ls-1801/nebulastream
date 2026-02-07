# CMakeLists.txt for adaptive-engine C++ tests

cmake_minimum_required(VERSION 3.24)

# Only build tests if testing is enabled (inherited from parent)
if(NOT NES_ENABLES_TESTS)
    message(STATUS "Adaptive engine tests disabled (NES_ENABLES_TESTS is OFF)")
    return()
endif()

# Fetch GTest if not already available
include(FetchContent)
FetchContent_Declare(
    googletest
    GIT_REPOSITORY https://github.com/google/googletest.git
    GIT_TAG v1.15.2
    FIND_PACKAGE_ARGS NAMES GTest
)
FetchContent_MakeAvailable(googletest)
include(GoogleTest)

# Create the QueryEngineTest executable
add_executable(QueryEngineTest
    QueryEngineTest.cpp
)

# Include the test infrastructure headers
target_include_directories(QueryEngineTest PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
)

# Link against the adaptive engine (includes both Rust and C++ code)
target_link_libraries(QueryEngineTest PRIVATE
    adaptive_engine
    GTest::gtest
    GTest::gtest_main
)

# Register tests with CTest via gtest_discover_tests
gtest_discover_tests(QueryEngineTest
    WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}
    DISCOVERY_MODE PRE_TEST
    DISCOVERY_TIMEOUT 30
)

message(STATUS "Added QueryEngineTest for adaptive-engine")
