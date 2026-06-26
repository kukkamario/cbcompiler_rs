# FD-048: Basic LLVM Codegen & Tooling Driver

**Status:** Open
**Priority:** High
**Effort:** High (> 4 hours) — the IR is ignored, but the object-emit + linker plumbing (CRT-aware linking and the runtime closure on Windows) is fiddly and load-bearing.
**Impact:** First native executable the project produces. Proves the back half of the AOT pipeline end-to-end — in-memory `inkwell::Module` → object file → linked (against the full CoolBasic runtime + Allegro closure, `/MD`-consistent) → runnable exe. No IR is read and no runtime function is *called* yet, but the runtime is on the link line, so later lowering FDs add only codegen, not toolchain work.

## Problem

FD-047 wired `inkwell`/LLVM 18 as an opt-in dependency: with `--features llvm` the backend links a real LLVM and can construct a `Context`/`Module`, but `Backend::execute` still returns `BackendError::unimplemented` (driver exit 3) — nothing emits a native artifact.

The output half of an AOT compiler is independent of IR→LLVM instruction selection and worth standing up on its own:

- In-memory `inkwell::Module` → **object file**: initialize an LLVM target, build a `TargetMachine` for the host triple, write object code.
- Object → **runnable exe**: a linker *driver* that pulls in CRT startup glue (so `main` is actually called) plus the CoolBasic runtime. On Windows (the primary dev env) the CRT model must stay `/MD` to match Rust and the future plugin-DLL system (CLAUDE.md).

A fixed "empty program" isolates toolchain risk from codegen risk.

## Solution

Under `#[cfg(feature = "codegen")]`, replace the stub `execute` with a minimal AOT pipeline that **ignores the IR body** and emits a fixed empty program — a module with a single `main` returning `i32 0`:

1. **Build module** — `Context`/`Module`, define `i32 @main() { ret i32 0 }`. (`program` is accepted but unread; the empty `main` is independent of it.)
2. **Emit object** — `Target::initialize_native`, build a `TargetMachine` for the host triple (default reloc/codegen model; `RelocMode::PIC` on Linux), write `.obj`/`.o` via `TargetMachine::write_to_file`.
3. **Link** — invoke the `clang`/`cc` driver (decision 1) to link the object + CRT startup + the CoolBasic runtime closure (decision 2) into a native exe, `/MD`-consistent on Windows. The generated `main` calls nothing, so the runtime is present but inert.
4. **Return `BackendOutcome::Produced { artifact }`** — the driver already prints `cb: wrote <path>` and exits 0 (`main.rs:160`).

The no-`codegen` stub still returns `unimplemented`. Frontend/sema/IR are untouched; the `interp` path is unaffected (all new code is gated on `codegen`).

## Design decisions

### 1. Linker driver: `clang` (Windows) / `cc` (Linux)

A runnable exe needs CRT startup glue, so we need a compiler/linker *driver* that knows the CRT — not a bare `ld`/`lld`. `clang` auto-discovers the toolchain with no manual lib-path wrangling; `lld-link`/`link.exe` would force hand-supplied UCRT/VC-runtime/SDK `/libpath:` entries.

**Windows recipe:**

```
<clang> <obj> -o <exe> -fms-runtime-lib=dll -Xlinker /nodefaultlib:libcmt  <runtime libs + closure>
```

`-fms-runtime-lib=dll` alone is **not** enough: it only sets the compile-phase `--dependent-lib=msvcrt`, while the clang driver still hardcodes `-defaultlib:libcmt` (static CRT) on the link line. The two collide (`LNK4098`) and static `libucrt.lib` wins → a mixed/static CRT, violating the `/MD` rule. `-Xlinker /nodefaultlib:libcmt` neutralizes the static default so the object's dynamic `msvcrt.lib` directive wins (the exe then imports `VCRUNTIME140.dll` / `api-ms-win-crt-*` / `MSVCP140.dll`). Use `clang.exe` (gnu-style driver), **not** `clang-cl.exe`. clang auto-discovers MSVC + the Windows SDK and delegates the actual link to MSVC `link.exe` — no `vcvars` environment needed.

**Linux/CI:** invoke the platform `cc` (gcc). Emit objects with `RelocMode::PIC` (gcc links PIE by default). `cc` is a C driver, so name the C++ runtime explicitly (`-lstdc++`), mirroring `cb-runtime-sys/build.rs`. The CI `linux-llvm-smoke` job has `gcc`/`cc` but not clang, so defaulting to `cc` needs no CI change.

**clang discovery:** anchor on `LLVM_SYS_181_PREFIX` (no new env var). Probe `<prefix>/bin/clang.exe`, then `<prefix>/tools/llvm/clang.exe`; then `cc`-crate discovery; then `clang`/`cc` on `PATH`; on failure return `BackendError::failed`. The two-location probe exists because vcpkg ships clang under `tools/llvm/`, surfaced at `bin/` only via the junction prefix.

`LLVM_SYS_181_PREFIX` must be the **junction** prefix `…\installed\x64-windows-static-md\llvm-sys-prefix` — the only layout where `<prefix>/bin/llvm-config.exe` exists, which `llvm-sys` requires.

### 2. Scope: bare `main`, full runtime closure linked (no runtime calls)

Emit `i32 @main() { ret i32 0 }` — the body calls **nothing** — but wire the link step to pull in the runtime: `cb_runtime` + `cb_runtime_core` (or the SDK-free `cb_runtime_sdkfree` archive where Allegro is unavailable) plus the transitive Allegro/fmt/png/zlib/brotli/webp/freetype/OpenAL/FLAC/ogg/vorbis/opus closure and the C++ runtime, all `/MD`-consistent. This completes the back half as real tooling: when the first lowering FD emits `call @cb_runtime_init`, the symbol resolves against libs already on the link line — adding the runtime then is pure codegen.

**Lazy vs. whole-archive.** The production link lists the runtime libs **normally (lazy)**: an empty `main` references no runtime symbol, so nothing is pulled in and the exe stays small; the closure is merely *available*. The gated test additionally links a **whole-archive** variant (`/WHOLEARCHIVE:` on Windows, `--whole-archive` on Linux) to prove the closure resolves, not just that the args parse.

**Runtime-path discovery.** `cb-backend-llvm` does not know where `cb-runtime-sys` dropped its `OUT_DIR` libs. Give `cb-runtime-sys` a `links = "cb_runtime"` manifest key; its `build.rs` emits metadata (`cargo:lib_dir=…`, the runtime lib names, and the path to the CMake-generated `cb_runtime_link_libs_*.txt` closure list, or an `sdkfree` marker). `cb-backend-llvm` (under `codegen`) takes a build-dep on `cb-runtime-sys`, and its new `build.rs` re-exports the `DEP_CB_RUNTIME_*` values as `cargo:rustc-env=…` so `link.rs` reads them via `env!`. The driver already depends on `cb-runtime-sys`, so the runtime is already built — only the paths are new. (Baked `OUT_DIR` paths are dev-local, not relocatable — a sysroot/install layout is a later FD.)

### 3. Output path & CLI: `-o` flag + constructor injection

Add `#[arg(short = 'o', long = "output")] output: Option<PathBuf>` to the `cb` CLI. Default the artifact next to the source: `<stem>` + `std::env::consts::EXE_SUFFIX`. The intermediate `.obj` goes in a `tempfile` temp dir (already a workspace dep; auto-cleaned). `cb_ir::Program` carries no source path and `execute` only gets `&Program + &Interner`, so codegen cannot name the artifact itself — inject `{ source, output }` into `LlvmBackend` by changing `make_backend(sel)` → `make_backend(sel, &path, output)` (interp ignores the extra args). This keeps the FD-044 `Backend::execute` signature and the `cb-backend-api` crate untouched; a `BackendOptions` param on `execute` is deferred until a second AOT backend needs it. For interp, `-o` is accepted and ignored.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-llvm/src/lib.rs` | MODIFY | `#[cfg(feature = "codegen")]` `execute` runs the emit→link pipeline and returns `Produced { artifact }`; `LlvmBackend` gains `{ source, output }` fields (constructor injection); no-feature stub still returns `unimplemented` |
| `crates/cb-backend-llvm/src/emit.rs` | CREATE | Build the empty-`main` module; `Target::initialize_native` (host-portable, un-gated — **no `target-x86` feature**); write object via `TargetMachine::write_to_file` (`RelocMode::PIC` on Linux) |
| `crates/cb-backend-llvm/src/link.rs` | CREATE | Resolve `clang.exe` under `<prefix>/bin` then `<prefix>/tools/llvm` (Win), else `cc`/`clang` (Linux/PATH); assemble args: object + runtime libs + closure (from the `env!`'d metadata) + `-fms-runtime-lib=dll -Xlinker /nodefaultlib:libcmt` (Win) / `-lstdc++` (Linux); invoke driver → exe; failures → `BackendError::failed` |
| `crates/cb-backend-llvm/build.rs` | CREATE | Read `DEP_CB_RUNTIME_*` (lib dir, lib names, closure-list path / sdk-free marker) and re-export as `cargo:rustc-env=…` so `link.rs` reads the runtime location via `env!`. Gated to the `codegen` build |
| `crates/cb-backend-llvm/Cargo.toml` | MODIFY | Under `codegen`: add `cb-runtime-sys` as a dependency (for the `DEP_CB_RUNTIME_*` metadata) and `cc` as a build-dependency (linker discovery). **No `inkwell` target feature** — `["llvm18-1"], default-features = false` is unchanged |
| `crates/cb-runtime-sys/Cargo.toml` | MODIFY | Add `links = "cb_runtime"` so the build-script link metadata flows to dependents as `DEP_CB_RUNTIME_*` |
| `crates/cb-runtime-sys/build.rs` | MODIFY | In both build paths, emit `cargo:lib_dir=…`, the runtime lib names, and the closure-list path (full) or an SDK-free marker — purely additive metadata; existing `rustc-link-*` directives unchanged |
| `crates/cb-driver/src/main.rs` | MODIFY | Add `-o`/`--output` flag; thread `{source, output}` into the backend via `make_backend(sel, &path, output)` |
| `CLAUDE.md` / `docs/` | MODIFY | Document the `clang`/`cc` link-driver requirement, the `-fms-runtime-lib=dll` **+ `/nodefaultlib:libcmt`** `/MD` rule, the runtime-closure link, and the `--features llvm` AOT build/run path |

## Verification

- **Empty program builds & runs:** with `LLVM_SYS_181_PREFIX` set to the junction prefix (decision 1), `cargo build -p cb-driver --features llvm`, then `cb --backend llvm empty.cb` writes a runtime-linked executable, prints `cb: wrote <path>`, and exits 0; running that executable exits 0.
- **Runtime closure resolves:** the gated test links a whole-archive variant (decision 2) and asserts it links + runs exit 0 — proving the closure resolves, not just that the link args parse.
- **Default path untouched:** `cargo build --workspace` / `cargo test --workspace --locked` stay LLVM-free and green (all new code is under `codegen`); `cb --backend interp` unchanged.
- **Gated test:** extend the FD-047 smoke with a `#[cfg(all(test, feature = "codegen"))]` test that emits the module, links it (full closure locally, SDK-free core on CI), runs the exe, and asserts exit 0. Runs on the existing CI `linux-llvm-smoke` job (`cargo test -p cb-backend-llvm --features codegen`) using `cc` — no CI workflow change.
- **Exit-code contract:** a link/emit failure returns `BackendError::failed` → driver exit 1; the `Unimplemented` → exit 3 path remains for the no-`codegen` build.

## Known risks

- **Linux PIE/relocation.** gcc links PIE by default; emit objects with `RelocMode::PIC` (or pass `-no-pie`). Verify on the CI `linux-llvm-smoke` job (not testable on the Windows dev box).
- **compiler-rt builtins absent from the vcpkg install** (`clang_rt.builtins-*`). Irrelevant for an empty `main`; a latent risk for later real codegen (128-bit math, some float ops). Revisit when lowering lands.
- **CI links only the SDK-free runtime.** The `linux-llvm-smoke` runner has no Allegro/CMake, so `cb-runtime-sys` falls back to `cb_runtime_sdkfree`. `link.rs` must link *whatever runtime `cb-runtime-sys` built* — driven by the discovery metadata (decision 2), not a hardcoded lib list — so the gated test passes on CI (SDK-free core) and locally (full closure) alike.
- **Non-relocatable paths.** Runtime lib paths are `cb-runtime-sys`'s `OUT_DIR` under `target/`, baked into the compiler at build time. Fine for a dev waypoint; a shipped compiler needs a sysroot/install layout (a later FD).
- **Startup static initializers.** Whole-archiving the runtime runs its global constructors before `main`; fine for the empty case (Allegro isn't ctor-initialized), but keep in mind once the real wrapper `main` exists.

## Related

- [FD-047](archive/FD-047_LLVM_DEPENDENCY_SETUP.md) — added `inkwell`/LLVM 18 as the opt-in dep this FD exercises; `Target::initialize_native` makes the anticipated `target-x86` feature unnecessary.
- [FD-044](archive/FD-044_BACKEND_TRAIT_SEAM.md) — the `Backend` trait + `BackendOutcome::Produced { artifact }` this FD returns.
- [FD-025](archive/FD-025_DRIVER_BACKEND_SELECTION_AND_EXIT_CODES.md) — `--backend llvm` exit-code contract (3 = unimplemented) this FD transitions away from.
- CLAUDE.md "Architectural ground rules" — pluggable backends, `/MD` CRT model, LLVM-free default build.
- Follow-ups (not this FD): the first **IR→LLVM lowering** FD (emits the real `cb_runtime_init` → body → exit calls; codegen-only, since the runtime is already linked); a later FD for a relocatable runtime **sysroot/install layout**.
