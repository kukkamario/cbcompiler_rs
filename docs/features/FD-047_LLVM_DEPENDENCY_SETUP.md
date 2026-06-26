# FD-047: LLVM Dependency Setup

**Status:** Open
**Priority:** Medium
**Effort:** Medium — Cargo/CI wiring is quick, but the Windows toolchain requires a one-time (multi-hour) vcpkg build of LLVM 18 with the dynamic-CRT `x64-windows-static-md` triplet.
**Impact:** Unblocks the first real LLVM codegen FD by adding `inkwell`/LLVM as an *opt-in* dependency without breaking the default LLVM-free build, test, or CI paths.

## Problem

`cb-backend-llvm` is a stub: its `Backend::execute` returns `BackendError::unimplemented` (driver exit 3) and the crate carries no LLVM dependency. CLAUDE.md states `inkwell` was intentionally left out "so the workspace builds without an LLVM toolchain installed; add it when codegen starts." Codegen is about to start, so we need the dependency and toolchain plumbing in place *first*, as its own step, because it is load-bearing and easy to get wrong:

- **The default build must stay LLVM-free.** `cb-backend-llvm` is an unconditional workspace member (`Cargo.toml` `members`), so `cargo build` / `cargo test --workspace` build it even when the driver's `llvm` feature is off. If we add `inkwell` as a *non-optional* dependency, every plain `cargo build` and CI run would suddenly require an LLVM toolchain — violating the ground rule. The dependency must be gated *inside* `cb-backend-llvm`, not only at the driver.
- **The LLVM toolchain is fiddly, especially on Windows (primary dev env).** `inkwell` wraps `llvm-sys`, which needs an `LLVM_SYS_<ver>_PREFIX` pointing at an LLVM install exposing `llvm-config` and the static libs. Prebuilt LLVM release binaries frequently omit what `llvm-sys` needs. We must pin a concrete LLVM version, document how to install/point at it on Windows and Linux, and decide whether CI builds the `llvm` feature at all (else it bitrots).

This FD is dependency + toolchain plumbing only. **No IR→LLVM lowering** — `execute` still returns `unimplemented` after this lands; it just compiles against a live `inkwell::Context` so the next FD can start emitting.

## Solution

Add `inkwell` as an **optional** dependency of `cb-backend-llvm`, gated behind an internal crate feature (e.g. `codegen`). The stub `Backend` impl keeps compiling with the feature *off* (so `cargo build --workspace` stays LLVM-free); with the feature *on* the crate links a real LLVM and can construct an `inkwell::Context`. The driver's existing `llvm` feature turns on `cb-backend-llvm/codegen` (`llvm = ["dep:cb-backend-llvm", "cb-backend-llvm/codegen"]`), so `cargo build --features llvm` is the single switch that pulls in LLVM end-to-end.

Affected crates:
- **`cb-backend-llvm`** — add optional `inkwell` dep + `codegen` feature; `#[cfg(feature = "codegen")]` a minimal "context boots" path (build a `Context`, an empty `Module`) to prove linkage; keep the no-feature stub as the default-build path.
- **`cb-driver`** — chain its `llvm` feature to `cb-backend-llvm/codegen`. No behavior change when `llvm` is off.
- **Workspace `Cargo.toml`** — add `inkwell` to `[workspace.dependencies]` pinned to LLVM 18: `inkwell = { version = "0.9", features = ["llvm18-1"], default-features = false }`. (Note the `-1` suffix: LLVM 18+ uses `llvmNN-1`, not `llvmNN-0`. Real codegen will later add a target feature such as `target-x86`; the linkage smoke needs none.)
- **Docs/CI** — document the `LLVM_SYS_<ver>_PREFIX` setup for Windows + Linux; decide on a CI job that builds (not necessarily runs) `--features llvm`.

### Decisions (resolved — FD-deep analysis + implementation 2026-06-26)

1. **LLVM/inkwell version → LLVM 18.1.6 via vcpkg.** Pin inkwell `0.9` with feature `llvm18-1` → `llvm-sys 181.x` (env var `LLVM_SYS_181_PREFIX`). The version is dictated by the project's vendored vcpkg baseline, which pins the `llvm` port at **18.1.6**; building LLVM through the existing vcpkg keeps the toolchain reproducible and consistent with the Allegro runtime. LLVM 18 is also more battle-tested in inkwell than 22. (Originally scoped to LLVM 22 against a prebuilt `G:\tools` install — rejected, see Decision 3.)
2. **CI scope → Linux-only smoke job that links LLVM.** Add `cargo test -p cb-backend-llvm --features codegen` on Linux, provisioning LLVM 18 via apt (`llvm-18-dev`) and setting `LLVM_SYS_181_PREFIX=/usr/lib/llvm-18`. Use `cargo test`, **not** `cargo build`: building the rlib alone compiles against `inkwell` but never *links* LLVM, so a build-only job would miss link breakage — the trivial smoke test forces the link. **No Windows CI** (not worth it for a link check). The existing `linux-sdk-free` job must **never** gain `--all-features`/`--features llvm` — that's the one realistic way the LLVM-free invariant regresses. This guard is what makes adding the dep *now* worthwhile rather than dead weight ahead of codegen.
3. **Windows toolchain source → build LLVM via the vendored vcpkg, `x64-windows-static-md` triplet.** That triplet is static library linkage + **dynamic CRT (`/MD`)**: static LLVM libs embedded into the `cb` binary (no LLVM DLL to ship) but the dynamic CRT, which (a) matches the Rust MSVC default — no `libcmt`/`msvcrt` conflict — and (b) is required for a future plugin-DLL system (`CallDLL`) so the EXE and plugin DLLs share one CRT heap. It is the same triplet the Allegro runtime already uses, landing the whole project on one CRT model. Point `LLVM_SYS_181_PREFIX` at the vcpkg install (with `bin/llvm-config.exe` available — see snags).
   *Rejected — the prebuilt `G:\tools\llvm-22.1.0`:* it is a **static-CRT (`/MT`)** build (wrong for the plugin-DLL direction) and was built with a newer MSVC toolset than the local one, leaving an unresolved STL symbol (`__std_find_first_of_trivial_pos_1`) at link. Using it would force `+crt-static` on the entire Rust build.

### Known snags / risks

- **vcpkg `llvm-config` layout.** vcpkg installs `llvm-config.exe` under `installed/x64-windows-static-md/tools/llvm/`, but `llvm-sys` looks for `$LLVM_SYS_181_PREFIX/bin/llvm-config.exe`. Point the prefix at a directory whose `bin/` exposes `llvm-config` (a junction/copy into the install's `bin/`, or add `tools/llvm` to `PATH` and leave the prefix unset). Exact value finalized once the vcpkg build lands.
- **One-time vcpkg LLVM build is heavy** (multi-hour, tens of GB), but cached in vcpkg's binary cache afterward and reproducible from the pinned baseline. libxml2/zlib/etc. arrive as vcpkg dependencies, so there is no hand-managed `xml2s.lib` (the earlier prebuilt-`G:\tools` path hit a missing `xml2s.lib` because that distribution listed libxml2 in `--system-libs` without shipping it).
- **`Cargo.lock` regen under `--locked`.** Adding `inkwell` changes the resolved graph; the lockfile (currently inkwell-free) must be regenerated and committed in the same change or CI's `cargo test --workspace --locked` fails. The lock lists `inkwell`/`llvm-sys` as nodes, but they are still not *built* in the default path.
- **No `unsafe_code` exception needed yet.** The smoke path (`Context::create()` + `create_module`) is entirely *safe* inkwell API, so `cb-backend-llvm` keeps `[lints] workspace = true` (inherits `deny`). A later lowering FD that trips `unsafe` can mirror `cb-runtime-sys`'s local `unsafe_code = "allow"`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-llvm/Cargo.toml` | MODIFY | Add optional `inkwell` dep + `codegen` feature; replace the "kept out for now" comment |
| `crates/cb-backend-llvm/src/lib.rs` | MODIFY | `#[cfg(feature = "codegen")]` path that constructs an `inkwell::Context`/empty `Module` to prove linkage; keep the no-feature stub returning `unimplemented` |
| `crates/cb-driver/Cargo.toml` | MODIFY | `llvm` feature also enables `cb-backend-llvm/codegen` |
| `Cargo.toml` (workspace) | MODIFY | Add `inkwell = { version = "0.9", features = ["llvm18-1"], default-features = false }` to `[workspace.dependencies]` |
| `Cargo.lock` | MODIFY | Regenerate (now resolves `inkwell`/`llvm-sys 181.x`) so CI's `--locked` stays green |
| `.github/workflows/ci.yml` | MODIFY | Add a Linux-only smoke job: provision LLVM 18 (`llvm-18-dev`), set `LLVM_SYS_181_PREFIX=/usr/lib/llvm-18`, run `cargo test -p cb-backend-llvm --features codegen` (links LLVM + runs the trivial smoke test). Leave the existing `linux-sdk-free` job untouched (no `--all-features`). |
| `CLAUDE.md` / `docs/` | MODIFY | Document the `LLVM_SYS_181_PREFIX` setup (Windows: vcpkg `x64-windows-static-md` install; Linux: `/usr/lib/llvm-18`), the dynamic-CRT rationale, and the `--features llvm` build path |

## Verification

- **Default build untouched:** `cargo build --workspace` and `cargo test --workspace --locked` succeed on a machine with **no** LLVM installed (the core invariant this FD must not break). `inkwell`/`llvm-sys` appear in `Cargo.lock` but are not fetched or built.
- **Feature build links LLVM:** with `LLVM_SYS_181_PREFIX` set (the vcpkg `x64-windows-static-md` install on Windows, `/usr/lib/llvm-18` on Linux), `cargo test -p cb-backend-llvm --features codegen` and `cargo build -p cb-driver --features llvm` both compile and link — no `libcmt`/`msvcrt` CRT conflict (the `static-md` triplet's dynamic CRT matches Rust's default).
- **Runtime behavior unchanged:** `cb --backend llvm <file>` still exits 3 (`unimplemented`) — this FD adds the dependency, not codegen. The existing FD-025 exit-code test stays green.
- **Linkage smoke:** a `#[cfg(all(test, feature = "codegen"))]` unit test that constructs an `inkwell::Context` and an empty `Module` passes (`cargo test -p cb-backend-llvm --features codegen`). Keep it in a gated `mod` so the no-feature build has no dead code.
- **CI guard:** the new Linux smoke job goes green; the existing `linux-sdk-free` job is unchanged and still runs LLVM-free.

## Related

- Stub being upgraded: `crates/cb-backend-llvm/src/lib.rs`
- [FD-044](archive/FD-044_BACKEND_TRAIT_SEAM.md) — the `Backend` trait this backend already implements
- [FD-045](archive/FD-045_CATALOG_METADATA_DECOUPLING.md) — lets a native backend type-check/emit catalog calls without linking the Allegro runtime
- [FD-025](archive/FD-025_DRIVER_BACKEND_SELECTION_AND_EXIT_CODES.md) — `--backend llvm` → exit 3 contract this FD preserves
- CLAUDE.md "Architectural ground rules" — pluggable backends, `llvm` opt-in via cargo features
- Follow-up (not this FD): the first IR→LLVM lowering FD that replaces `unimplemented` with real emission
