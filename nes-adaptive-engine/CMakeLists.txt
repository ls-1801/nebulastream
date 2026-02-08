# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at

#    https://www.apache.org/licenses/LICENSE-2.0

# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# NES integration for adaptive-engine.
# The adaptive_engine Corrosion target is created in cmake/EnableRust.cmake.
# This file adds the C++ INTERFACE sources, includes, and platform libraries.
# For standalone builds (QueryEngineTest only), use cpp/CMakeLists.txt directly.

target_sources(adaptive_engine INTERFACE
    ${CMAKE_CURRENT_SOURCE_DIR}/cpp/src/Engine.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/cpp/src/FfiCallbacks.cpp
)

target_include_directories(adaptive_engine INTERFACE
    ${CMAKE_CURRENT_SOURCE_DIR}/cpp/include
)

if(UNIX AND NOT APPLE)
    target_link_libraries(adaptive_engine INTERFACE pthread dl)
elseif(APPLE)
    target_link_libraries(adaptive_engine INTERFACE pthread)
endif()

if(NES_ENABLES_TESTS)
    add_subdirectory(cpp/tests)
endif()
