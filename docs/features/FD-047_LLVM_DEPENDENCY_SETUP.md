# FD-047: LLVM Dependency Setup

**Status:** Open
**Priority:** Medium
**Effort:** Low–Medium (1-2 hours) — pure Cargo/CI wiring; the Windows LLVM toolchain is already installed (`G:\tools\llvm-22.1.0`), so there is no acquisition work.
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
- **Workspace `Cargo.toml`** — add `inkwell` to `[workspace.dependencies]` pinned to LLVM 22: `inkwell = { version = "0.9", features = ["llvm22-1"], default-features = false }`. (Note the `-1` suffix: LLVM 18+ uses `llvmNN-1`, not `llvmNN-0`. Real codegen will later add a target feature such as `target-x86`; the linkage smoke needs none.)
- **Docs/CI** — document the `LLVM_SYS_<ver>_PREFIX` setup for Windows + Linux; decide on a CI job that builds (not necessarily runs) `--features llvm`.

### Decisions (resolved — FD-deep analysis 2026-06-26)

1. **LLVM/inkwell version → LLVM 22.** Pin inkwell `0.9` with feature `llvm22-1` → `llvm-sys 221.x`. This is inkwell's newest supported major *and* exactly matches the LLVM 22.1.0 already installed on the dev box, so there is zero acquisition friction. Env var is `LLVM_SYS_221_PREFIX`. Fallback if Linux CI can't provision 22: pin one major back (`llvm21-1`), at the cost of a second local install.
2. **CI scope → Linux-only, build-only smoke job.** Add `cargo build -p cb-backend-llvm --features codegen` on Linux, provisioning LLVM 22 via apt.llvm.org and setting `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`. **No Windows CI** (not worth it for a link check). The existing `linux-sdk-free` job must **never** gain `--all-features`/`--features llvm` — that's the one realistic way the LLVM-free invariant regresses. This guard is what makes adding the dep *now* worthwhile rather than dead weight ahead of codegen.
3. **Windows toolchain source → consume the existing install.** A complete MSVC-ABI static LLVM 22.1.0 distribution (246 static `.lib`s + headers + `llvm-config`, `--shared-mode = static`) is already at `G:\tools\llvm-22.1.0`. No install needed — just set `LLVM_SYS_221_PREFIX=G:\tools\llvm-22.1.0` via a user env var or a **git-ignored** `.cargo/config.toml [env]` (never commit the machine-specific absolute path). Must use the **MSVC** Rust toolchain (matches the MSVC-built LLVM); a `-gnu`/MinGW toolchain or a stray MinGW `llvm-config` on PATH would fail to link.

### Known snags / risks

- **`xml2s.lib` first-build failure (verified).** `llvm-config --system-libs` on the installed LLVM emits `xml2s.lib` (static libxml2), which is **not** present in `G:\tools\llvm-22.1.0\lib`. The first `cargo build --features codegen` will likely fail at link with an unresolved `xml2s.lib`. Mitigate by either supplying a static libxml2 on the lib path (the repo already vendors vcpkg → `vcpkg install libxml2:x64-windows-static-md`) or, cleaner, using an LLVM built with `-DLLVM_ENABLE_LIBXML2=OFF`.
- **`Cargo.lock` regen under `--locked`.** Adding `inkwell` changes the resolved graph; the lockfile (currently inkwell-free) must be regenerated and committed in the same change or CI's `cargo test --workspace --locked` fails. The lock will then list `inkwell`/`llvm-sys` as nodes, but they are still not *built* in the default path.
- **No `unsafe_code` exception needed yet.** The smoke path (`Context::create()` + `create_module`) is entirely *safe* inkwell API, so `cb-backend-llvm` keeps `[lints] workspace = true` (inherits `deny`). A later lowering FD that trips `unsafe` can mirror `cb-runtime-sys`'s local `unsafe_code = "allow"`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-llvm/Cargo.toml` | MODIFY | Add optional `inkwell` dep + `codegen` feature; replace the "kept out for now" comment |
| `crates/cb-backend-llvm/src/lib.rs` | MODIFY | `#[cfg(feature = "codegen")]` path that constructs an `inkwell::Context`/empty `Module` to prove linkage; keep the no-feature stub returning `unimplemented` |
| `crates/cb-driver/Cargo.toml` | MODIFY | `llvm` feature also enables `cb-backend-llvm/codegen` |
| `Cargo.toml` (workspace) | MODIFY | Add `inkwell = { version = "0.9", features = ["llvm22-1"], default-features = false }` to `[workspace.dependencies]` |
| `Cargo.lock` | MODIFY | Regenerate (now resolves `inkwell`/`llvm-sys`) so CI's `--locked` stays green |
| `.github/workflows/ci.yml` | MODIFY | Add a Linux-only build-only smoke job: provision LLVM 22 (apt.llvm.org), set `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, run `cargo build -p cb-backend-llvm --features codegen`. Leave the existing `linux-sdk-free` job untouched (no `--all-features`). |
| `CLAUDE.md` / `docs/` | MODIFY | Document `LLVM_SYS_221_PREFIX` setup (Windows: `G:\tools\llvm-22.1.0`; Linux: `/usr/lib/llvm-22`), the MSVC-toolchain requirement, the `xml2s.lib` workaround, and the `--features llvm` build path |

## Verification

- **Default build untouched:** `cargo build --workspace` and `cargo test --workspace --locked` succeed on a machine with **no** LLVM installed (the core invariant this FD must not break). `inkwell`/`llvm-sys` appear in `Cargo.lock` but are not fetched or built.
- **Feature build links LLVM:** with `LLVM_SYS_221_PREFIX` set (`G:\tools\llvm-22.1.0` on Windows, `/usr/lib/llvm-22` on Linux), `cargo build -p cb-backend-llvm --features codegen` and `cargo build -p cb-driver --features llvm` both compile and link. (First Windows build may hit the `xml2s.lib` snag above — apply the mitigation.)
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
