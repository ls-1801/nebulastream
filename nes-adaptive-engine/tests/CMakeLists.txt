# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at

#    https://www.apache.org/licenses/LICENSE-2.0

# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

add_executable(adaptive-engine-stage-test AdaptiveEngineStageTest.cpp FFIStubs.cpp)
target_include_directories(adaptive-engine-stage-test PRIVATE ${CMAKE_SOURCE_DIR}/nes-common/tests/Util/include)
target_link_libraries(adaptive-engine-stage-test
        PRIVATE
            nes-adaptive-engine
            nes-executable
            nes-memory
            nes-common
            GTest::gtest
            GTest::gtest_main
)
# Allow duplicate symbols from multiple CXX bridges (nes_adaptive_engine_bindings and nes_rust_bindings)
# The CXX runtime is compiled into both libraries, but the symbols are identical
target_link_options(adaptive-engine-stage-test PRIVATE "LINKER:--allow-multiple-definition")
