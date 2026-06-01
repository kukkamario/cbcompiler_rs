# cbcompiler_rs

A from-scratch reimplementation of the **CoolBasic** compiler in **Rust**.
CoolBasic is a BASIC-dialect game programming language; this project compiles it
with a pluggable backend design:

- **Interpreter backend** (`cb-backend-interp`) — the reference implementation,
  built for debuggability. Default.
- **LLVM backend** (`cb-backend-llvm`) — for AOT native codegen. Currently a
  stub; no LLVM toolchain is required yet.

See [`CLAUDE.md`](CLAUDE.md) for architecture and [`docs/`](docs/) for the
language and runtime references.

---

## Prerequisites (all platforms)

Building the workspace **always** compiles the C++ runtime in [`runtime/`](runtime/)
(`cb-runtime-sys` is an unconditional dependency of the `cb` driver, even for the
interp-only default build). That runtime links **Allegro 5** and its addons
(`primitives`, `image`, `font`, `ttf`, `audio`, `acodec`). So every build needs:

| Tool | Minimum | Notes |
|------|---------|-------|
| Rust | recent stable (edition 2024) | `rustup` recommended |
| CMake | ≥ 3.20 | drives the runtime build |
| C++ compiler | C++20 with `<format>` | MSVC on Windows; **GCC ≥ 13** on Linux (see note) |
| Allegro 5 + addons | via vcpkg | see per-platform setup below |

### Why vcpkg?

`runtime/CMakeLists.txt` uses `find_package(Allegro CONFIG REQUIRED)` (CMake
**CONFIG** mode), which needs Allegro's `AllegroConfig.cmake` package files.
vcpkg's `allegro5` port installs those; most distro packages ship only
pkg-config and will **not** satisfy this build. The build script
(`crates/cb-runtime-sys/build.rs`) automatically uses vcpkg when it finds
`runtime/vcpkg/scripts/buildsystems/vcpkg.cmake`. The dependency set is declared
in [`runtime/vcpkg.json`](runtime/vcpkg.json).

---

## Linux setup (Debian/Ubuntu)

1. **Toolchain + system libraries** that vcpkg's `allegro5` port needs to compile
   Allegro from source (X11, OpenGL, audio):

   ```sh
   sudo apt-get update && sudo apt-get install -y \
     build-essential cmake git curl zip unzip tar pkg-config python3 \
     autoconf automake autoconf-archive libtool m4 \
     libx11-dev libxcursor-dev libxinerama-dev libxi-dev libxrandr-dev \
     libxss-dev libxext-dev libgl1-mesa-dev libglu1-mesa-dev \
     libasound2-dev libpulse-dev
   ```

   > The autotools (`autoconf`/`automake`/`libtool`/`m4`) are required because
   > vcpkg builds Allegro's `alsa` dependency from source via autotools; without
   > them the `alsa` port fails in a few seconds and aborts the whole install.

   **GCC 13+ is required on Linux.** vcpkg builds `openal-soft` (Allegro's audio
   backend), which `#include <format>` — a header libstdc++ only ships in GCC 13+.
   Ubuntu **24.04+** ships GCC 13 as the default `gcc`/`g++`, so nothing extra is
   needed. On **22.04** the default tops out at g++-12 (no `<format>`); add the
   toolchain PPA and select it for the build:

   ```sh
   sudo add-apt-repository -y ppa:ubuntu-toolchain-r/test
   sudo apt-get update && sudo apt-get install -y g++-13 gcc-13
   export CC=gcc-13 CXX=g++-13   # so vcpkg's ports and the runtime both use it
   ```

2. **Rust**, if not already installed:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. **Bootstrap vcpkg** into the runtime directory (where `build.rs` looks for it):

   ```sh
   git clone https://github.com/microsoft/vcpkg runtime/vcpkg
   runtime/vcpkg/bootstrap-vcpkg.sh
   ```

4. **Build.** The first build has vcpkg compile Allegro and its dependencies
   from source (slow, one-time); subsequent builds are fast:

   ```sh
   cargo build
   cargo test --workspace
   ```

> **Stale CMake cache:** if you ran `cargo build` *before* bootstrapping vcpkg,
> a `CMakeCache.txt` was written without the vcpkg toolchain, and CMake honors a
> toolchain file only on a fresh build tree — so later builds keep failing with
> `Could not find ... AllegroConfig.cmake`. Fix: `cargo clean -p cb-runtime-sys`
> then rebuild.

> **Font note:** family-name font resolution on Linux is gated behind a
> `FONTCONFIG_FOUND` define that the CMake build does not currently set, so
> `LoadFont` by family name returns nothing and falls back to the builtin /
> file-path path. This is a known gap, not a setup error.

---

## Windows setup

1. **Visual Studio Build Tools** with the *Desktop development with C++* workload
   (provides MSVC + the Windows SDK).

2. **CMake** ≥ 3.20 (the Visual Studio installer can provide it, or install
   standalone and ensure it's on `PATH`).

3. **Rust** via [rustup](https://rustup.rs/) — use the default MSVC toolchain
   (`x86_64-pc-windows-msvc`).

4. **Bootstrap vcpkg** into the runtime directory:

   ```powershell
   git clone https://github.com/microsoft/vcpkg runtime/vcpkg
   runtime\vcpkg\bootstrap-vcpkg.bat
   ```

5. **Build.** `build.rs` automatically selects the `x64-windows-static-md`
   triplet, producing a `cb.exe` that statically links Allegro (no Allegro DLLs
   to ship). The first build compiles Allegro from source via vcpkg:

   ```powershell
   cargo build
   cargo test --workspace
   ```

---

## Building & running

```sh
cargo build                       # whole workspace (interp backend, default)
cargo build --features llvm       # also build the (stub) LLVM backend
cargo build --no-default-features  # dump-only binary, no backend
cargo check                       # type-check only, faster
cargo test                        # run all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Run the `cb` driver:

```sh
cargo run -p cb-driver -- [--backend <name>] [--dump-ast] [--dump-ir] <file.cb>
```

- `--backend <interp|llvm>` — select a backend (default: `interp`). Backends not
  compiled into the binary are rejected with a helpful message.
- `--dump-ast` / `--dump-ir` — dump intermediate representations.

Example programs live in [`examples/`](examples/):

```sh
cargo run -p cb-driver -- examples/bounce.cb
```

---

## Workspace layout

| Crate | Role |
|-------|------|
| `crates/cb-diagnostics` | Diagnostics, `Span`, `FileId`, `Symbol`, `Interner` |
| `crates/cb-frontend` | Lexer, parser, AST |
| `crates/cb-ir` | Backend-agnostic IR + passes |
| `crates/cb-sema` | Semantic analysis + AST→IR lowering |
| `crates/cb-backend-interp` | Interpreter backend (reference impl) |
| `crates/cb-backend-llvm` | LLVM backend (stub) |
| `crates/cb-runtime-sys` | Rust bindings + CMake build of the C++ runtime |
| `crates/cb-driver` | CLI (`cb` binary) wiring everything together |
| `runtime/` | C++ runtime (Allegro-backed graphics/input/audio/text) |
