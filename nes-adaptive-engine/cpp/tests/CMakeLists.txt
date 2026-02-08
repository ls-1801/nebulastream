# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at

#    https://www.apache.org/licenses/LICENSE-2.0

# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# CMakeLists.txt for adaptive-engine C++ tests

if(NOT NES_ENABLES_TESTS)
    message(STATUS "Adaptive engine tests disabled (NES_ENABLES_TESTS is OFF)")
    return()
endif()

include(GoogleTest)
find_package(cpptrace CONFIG REQUIRED)

# Create the QueryEngineTest executable
add_executable(QueryEngineTest
    QueryEngineTest.cpp
)

target_include_directories(QueryEngineTest PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
)

target_link_libraries(QueryEngineTest PRIVATE
    adaptive_engine
    cpptrace::cpptrace
    ${GTEST_LIBRARIES}
)

gtest_discover_tests(QueryEngineTest
    WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}
    DISCOVERY_MODE PRE_TEST
    DISCOVERY_TIMEOUT 30
)

message(STATUS "Added QueryEngineTest for adaptive-engine")
