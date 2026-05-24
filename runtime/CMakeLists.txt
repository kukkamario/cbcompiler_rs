cmake_minimum_required(VERSION 3.20)
project(cb_runtime C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

add_library(cb_runtime STATIC
    catalog.c
)

target_include_directories(cb_runtime PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
