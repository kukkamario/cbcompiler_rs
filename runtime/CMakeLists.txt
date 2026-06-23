cmake_minimum_required(VERSION 3.20)
project(cb_runtime C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
# C++20 needed for `template<auto Fn>` deduction used by the catalog DSL.
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Allegro CONFIG REQUIRED)

# ─── Core library (FD-016) ───────────────────────────────────────────────
# The irreducible, plugin-facing runtime: the opaque string type + its
# primitives and the CbStringApi table. ZERO Allegro / functionality
# dependency — this is what a plugin statically links to handle String
# parameters. (FD-015 will add cb_host.cpp here for the trap-channel
# handshake.)
add_library(cb_runtime_core STATIC
    cb_string.cpp
    cb_host.cpp
)
target_include_directories(cb_runtime_core PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})

# ─── Functionality library ───────────────────────────────────────────────
# The feature subsystems built on top of core: the catalog assembly, the
# String library, Math, System/Time, Graphics, Input. Links core + Allegro;
# the whole Allegro transitive-closure machinery below is attached here only.
add_library(cb_runtime STATIC
    catalog.cpp
    cb_math.cpp
    cb_strfuncs.cpp
    cb_system.cpp
    cb_memblock.cpp
    cb_file.cpp
    cb_gfx.cpp
    cb_camera.cpp
    cb_map.cpp
    cb_object.cpp
    cb_font.cpp
    cb_input.cpp
    cb_sound.cpp
)

target_include_directories(cb_runtime PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(cb_runtime PUBLIC cb_runtime_core)

set(CB_ALLEGRO_MODULES
    Allegro::allegro
    Allegro::allegro_primitives
    Allegro::allegro_image
    Allegro::allegro_font
    Allegro::allegro_ttf
    Allegro::allegro_audio
    Allegro::allegro_acodec
)

# Allegro's public headers gate function declarations on
# ALLEGRO_STATICLINK — without it they're declared __declspec(dllimport),
# producing __imp_al_* symbols the static .libs can't satisfy. vcpkg's
# port doesn't propagate this via INTERFACE_COMPILE_DEFINITIONS, so set
# it explicitly when targeting a static triplet.
set(CB_LINK_SEEDS ${CB_ALLEGRO_MODULES})
if(VCPKG_TARGET_TRIPLET MATCHES "static")
    target_compile_definitions(cb_runtime PRIVATE ALLEGRO_STATICLINK)
    # vcpkg's allegro5 port forgets to list shell32 in its static-build
    # INTERFACE_LINK_LIBRARIES, but allegro_win_get_path needs SHGetFolderPathW.
    if(WIN32)
        list(APPEND CB_LINK_SEEDS shell32)
        # allegro_audio's WASAPI backend (openal-soft) boosts its mixer thread via
        # MMCSS — AvSetMmThreadCharacteristicsW / AvRevertMmThreadCharacteristics
        # from avrt.lib (FD-041). Like shell32 and fmt below, the static openal
        # references it but the vcpkg port doesn't surface it to the transitive
        # walk (openal is recorded as an absolute path), so add it explicitly.
        list(APPEND CB_LINK_SEEDS avrt)
    endif()
endif()

# openal-soft (Allegro's audio backend, pulled in via allegro_audio) de-vendors
# its bundled {fmt} to the standalone `fmt` package. Allegro records openal as
# an absolute library path rather than an imported target, so the transitive
# walk below never reaches openal's own `fmt` dependency. Add it to the seeds
# explicitly so libfmt lands on the final link line — after openal, which
# references it (GNU ld resolves left-to-right).
find_package(fmt CONFIG QUIET)
if(TARGET fmt::fmt)
    list(APPEND CB_LINK_SEEDS fmt::fmt)
endif()

target_link_libraries(cb_runtime PRIVATE ${CB_LINK_SEEDS})

# ─── Font resolution backend (FD-022) ────────────────────────────────────
# On non-Windows, cb_font.cpp resolves system font families through fontconfig,
# but only when FONTCONFIG_FOUND is defined — otherwise the carefully-ported
# branch compiles out and LoadFont("Arial") silently falls back to Allegro's
# builtin 8x8 font. Define it (and link the lib) when fontconfig is available;
# otherwise say so. Windows uses a static font-name table and needs none of this.
if(NOT WIN32)
    find_package(Fontconfig QUIET)
    if(Fontconfig_FOUND)
        target_compile_definitions(cb_runtime PRIVATE FONTCONFIG_FOUND)
        target_link_libraries(cb_runtime PRIVATE Fontconfig::Fontconfig)
    else()
        message(STATUS
            "Fontconfig not found: named-font resolution falls back to the Allegro builtin font")
    endif()
endif()

# With static linking, Rust must be told about every transitive dependency
# (libpng, zlib, ogg, vorbis, ...) and Windows system lib (dwmapi, mfplat,
# ...) that CMake would otherwise inject automatically. Walk the
# INTERFACE_LINK_LIBRARIES of the seed Allegro modules to collect the
# full closure, then emit it via file(GENERATE) for build.rs to parse.
function(cb_collect_transitive_links seeds out_var)
    set(_visited "")
    set(_pending ${seeds})
    while(_pending)
        list(POP_FRONT _pending _item)
        if(_item IN_LIST _visited)
            continue()
        endif()
        list(APPEND _visited "${_item}")
        if(TARGET ${_item})
            get_target_property(_deps ${_item} INTERFACE_LINK_LIBRARIES)
            if(_deps)
                foreach(_dep IN LISTS _deps)
                    string(REGEX REPLACE "^\\$<LINK_ONLY:(.+)>$" "\\1" _dep "${_dep}")
                    list(APPEND _pending "${_dep}")
                endforeach()
            endif()
        endif()
    endwhile()
    set(${out_var} "${_visited}" PARENT_SCOPE)
endfunction()

cb_collect_transitive_links("${CB_LINK_SEEDS}" _cb_all_links)

set(_cb_link_content "")
foreach(_link IN LISTS _cb_all_links)
    if(TARGET ${_link})
        get_target_property(_type ${_link} TYPE)
        if(NOT _type STREQUAL "INTERFACE_LIBRARY")
            string(APPEND _cb_link_content "$<TARGET_LINKER_FILE:${_link}>\n")
        endif()
    elseif(_link AND NOT _link MATCHES "-NOTFOUND$")
        string(APPEND _cb_link_content "${_link}\n")
    endif()
endforeach()

file(GENERATE
    OUTPUT "${CMAKE_BINARY_DIR}/cb_runtime_link_libs_$<CONFIG>.txt"
    CONTENT "${_cb_link_content}"
)

# ─── Native C++ unit tests (FD-022) ──────────────────────────────────────
# OFF by default so the normal cargo build (build.rs runs `cmake --build .`)
# never configures GoogleTest or builds this target. To run the tests:
#
#   cmake -S runtime -B build-tests -DCB_RUNTIME_TESTS=ON \
#         -DVCPKG_MANIFEST_FEATURES=tests \
#         -DCMAKE_TOOLCHAIN_FILE=runtime/vcpkg/scripts/buildsystems/vcpkg.cmake \
#         -DVCPKG_TARGET_TRIPLET=x64-windows-static-md
#   cmake --build build-tests --config Release
#   ctest --test-dir build-tests -C Release --output-on-failure
#
# The pure-logic suites (geom, utf8, camera, map, object, collision, particle)
# only #include cb_geom.h / cb_utf8.h / cb_camera_math.h / cb_map_data.h /
# cb_object_data.h / cb_collision_data.h / cb_particle.h; the input, memblock,
# and file suites drive the extern "C" entry points in cb_runtime directly (no
# display), so the target links cb_runtime + Allegro.
option(CB_RUNTIME_TESTS "Build the native C++ unit tests (requires GoogleTest)" OFF)
if(CB_RUNTIME_TESTS)
    enable_testing()
    find_package(GTest CONFIG REQUIRED)
    add_executable(cb_runtime_tests
        tests/test_geom.cpp
        tests/test_utf8.cpp
        tests/test_camera.cpp
        tests/test_map.cpp
        tests/test_object.cpp
        tests/test_collision.cpp
        tests/test_particle.cpp
        tests/test_memblock.cpp
        tests/test_file.cpp
        tests/test_input.cpp
        tests/test_masking.cpp
        tests/test_sound.cpp
    )
    target_link_libraries(cb_runtime_tests PRIVATE
        cb_runtime ${CB_LINK_SEEDS} GTest::gtest GTest::gtest_main)
    if(VCPKG_TARGET_TRIPLET MATCHES "static")
        target_compile_definitions(cb_runtime_tests PRIVATE ALLEGRO_STATICLINK)
    endif()
    include(GoogleTest)
    gtest_discover_tests(cb_runtime_tests)
endif()
