cmake_minimum_required(VERSION 3.15)
project(rnqcc_cmake_smoke C)

add_library(math_helpers STATIC math.c)
target_include_directories(math_helpers PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/include)

add_executable(cmake_smoke main.c)
target_include_directories(cmake_smoke PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/include)
target_link_libraries(cmake_smoke PRIVATE math_helpers)
