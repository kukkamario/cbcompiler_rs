# Overlay of vcpkg's built-in x64-windows-static-md triplet (static libs, dynamic
# /MD CRT) that builds the RELEASE configuration only. vcpkg builds both debug
# and release by default; for a huge port like LLVM that roughly doubles the
# build for a debug half we never link. The standalone AOT link and the interp
# `cb` both consume the release libraries (cb-runtime-sys configures CMake with
# CMAKE_BUILD_TYPE=Release), so the debug half is pure waste.
#
# Activated only where VCPKG_OVERLAY_TRIPLETS points here (the release workflow);
# a plain dev `vcpkg install` keeps using the built-in debug+release triplet.
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_BUILD_TYPE release)
