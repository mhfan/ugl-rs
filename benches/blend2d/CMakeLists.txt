cmake_minimum_required(VERSION 3.20)
project(ugl_blend2d_bench LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(BLEND2D_STATIC TRUE CACHE BOOL "Link Blend2D statically" FORCE)
set(BLEND2D_TEST FALSE CACHE BOOL "Disable Blend2D tests" FORCE)

if(NOT DEFINED BLEND2D_DIR)
  message(FATAL_ERROR "Pass -DBLEND2D_DIR=/absolute/path/to/blend2d")
endif()

add_subdirectory("${BLEND2D_DIR}" blend2d EXCLUDE_FROM_ALL)
add_executable(blend2d_bench blend2d_bench.cpp)
target_link_libraries(blend2d_bench PRIVATE blend2d::blend2d)
if(CMAKE_CXX_COMPILER_ID MATCHES "Clang|GNU")
  target_compile_options(blend2d_bench PRIVATE -Wall -Wextra -Wconversion)
endif()
