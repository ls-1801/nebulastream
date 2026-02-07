# CMakeLists.txt for adaptive-engine C++ tests

if(NOT NES_ENABLES_TESTS)
    message(STATUS "Adaptive engine tests disabled (NES_ENABLES_TESTS is OFF)")
    return()
endif()

include(GoogleTest)

# Create the QueryEngineTest executable
add_executable(QueryEngineTest
    QueryEngineTest.cpp
)

target_include_directories(QueryEngineTest PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
)

target_link_libraries(QueryEngineTest PRIVATE
    adaptive_engine
    ${GTEST_LIBRARIES}
)

gtest_discover_tests(QueryEngineTest
    WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}
    DISCOVERY_MODE PRE_TEST
    DISCOVERY_TIMEOUT 30
)

message(STATUS "Added QueryEngineTest for adaptive-engine")
