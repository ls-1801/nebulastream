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
