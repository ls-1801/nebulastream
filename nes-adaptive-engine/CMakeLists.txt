# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at

#    https://www.apache.org/licenses/LICENSE-2.0

# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Add Rust bindings subdirectory
add_subdirectory(nes-rust-bindings/executor)

# Create the nes-adaptive-engine library
add_library(nes-adaptive-engine
        src/AdaptiveEngineStage.cpp
)

# Set up include directories
target_include_directories(nes-adaptive-engine
        PUBLIC include
        PRIVATE .
)

# Link against the Rust bindings and NES libraries
target_link_libraries(nes-adaptive-engine
        PUBLIC
            nes-adaptive-engine-bindings
            nes-executable
            nes-memory
            nes-common
)

# Add tests if enabled
add_tests_if_enabled(tests)
