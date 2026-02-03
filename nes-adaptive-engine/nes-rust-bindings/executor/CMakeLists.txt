# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at

#    https://www.apache.org/licenses/LICENSE-2.0

# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Create the cxxbridge target for the Rust bindings
# FILES path is relative to crate manifest directory
corrosion_add_cxxbridge(
        nes-adaptive-engine-bindings
        CRATE nes_adaptive_engine_bindings
        FILES ../executor/lib.rs
)

# Add C++ sources that implement the C++ side of the bridge
target_sources(nes-adaptive-engine-bindings PRIVATE src/AdaptiveEngineBindings.cpp)

# Set up include directories
target_include_directories(nes-adaptive-engine-bindings
        PRIVATE .
        PUBLIC include
)

# Link against NES libraries needed for TupleBuffer and PipelineExecutionContext
target_link_libraries(nes-adaptive-engine-bindings PUBLIC nes-memory nes-common nes-executable)
