cmake_minimum_required(VERSION 3.20)
project(cb_runtime C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

find_package(Allegro CONFIG REQUIRED)

add_library(cb_runtime STATIC
    catalog.c
)

target_include_directories(cb_runtime PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})

target_link_libraries(cb_runtime PRIVATE
    Allegro::allegro
    Allegro::allegro_primitives
    Allegro::allegro_image
    Allegro::allegro_font
    Allegro::allegro_ttf
    Allegro::allegro_audio
    Allegro::allegro_acodec
)
