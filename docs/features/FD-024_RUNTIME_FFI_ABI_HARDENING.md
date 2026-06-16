# FD-024: Runtime FFI ABI-Handshake & Catalog-Decode Hardening

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Medium (2-4 hours)
**Impact:** Makes the FD-015 ABI handshake actually run in shipped code (today it's validated only inside a test), handles the runtime-declines path the sole caller currently throws away, and refactors `load_catalog` so its (majority) defensive code becomes testable.

## Problem

The post-FD-018 review rated `cb-runtime-sys` the lowest-quality area (3/5) — not for what it does, but for guards that are decorative and error paths that are structurally untestable. No backend leakage was found; the issues are FFI-safety and coverage.

1. **The ABI `size`/`abi_version` handshake is never validated in production.** `CbHostApi` carries `size`/`abi_version` documented as caller-set ABI guards (`lib.rs:79-87`) and `CbRuntimeHooks.size` is callee-set, but nothing on the live path checks them: the C `cb_runtime_init` stores the host pointer unconditionally (`cb_host.cpp:25-30`), and the Rust `runtime_init` (`lib.rs:155-162`) only null-checks the returned pointer, never comparing `hooks.size` to `size_of::<CbRuntimeHooks>()`. The only size assertion lives inside the `runtime_init_roundtrip` *test* (`lib.rs:489`). The FD-015 handshake the design relies on is effectively decorative in shipped code.
2. **The sole production caller discards `runtime_init`'s result.** `cb-backend-interp` does `let _ = cb_runtime_sys::runtime_init(&HOST_API);` (`interp.rs:105`), throwing away the `Option<&CbRuntimeHooks>`. If the runtime ever returns `None` (declines) or an ABI-incompatible table, the interpreter proceeds as if the trap channel were wired. (Practically benign with today's statically-linked runtime, which always returns `&g_hooks` — but the structural gap is real and matters for the FD-009 plugin loader.)
3. **Duplicate type tags silently overwrite; duplicate symbols unvalidated.** `tag_to_name.insert(td.tag, name)` (`lib.rs:207`) silently overwrites a prior type sharing a tag, so a malformed catalog resolves all references to whichever came last; nothing checks duplicate `c_symbol`s or function names either. Inconsistent with the otherwise-rigorous null/UTF-8 defensiveness in the same loop.

Coverage / consistency items folded in:

- **Invalid-UTF-8 handling is inconsistent:** type/func/symbol names return `Err` (`lib.rs:199`/`:233`/`:241`) but param names silently become `"_"` (`:262`).
- **`type_tag_to_ir_type` is pure and FFI-free but has zero unit tests** (`lib.rs:293-313`), including its custom-tag lookup and unknown-tag `Err` branch.
- **`load_catalog`'s defensive branches are structurally untestable** (`lib.rs:168-291`): every version-mismatch/null-pointer/reserved-tag/bad-UTF-8 path depends on a malformed `CbCatalog`, but tests only call the real linked `cb_runtime_get_catalog()`, which always returns a valid v5 catalog. The function takes no catalog parameter, so there is no injection seam.
- **`CbFuncDesc.flags` is decode-dead and unpinned** (`lib.rs:39`): present in the `repr(C)` mirror but never read into `FuncDesc`, with no static layout assertion to catch ABI drift in this trailing field.
- **`build.rs::strip_unc` double-unwraps `to_str()`** (`build.rs:6-14`) and panics on a non-UTF-8 `OUT_DIR`/runtime path.

## Resolved Decisions (2026-06-16)

1. **`runtime_init` returns `Result<&'static CbRuntimeHooks, String>`** (not `Option`). This distinguishes "runtime declined" from "ABI-incompatible hook table" with a diagnostic string, matching `load_catalog`'s style.
2. **Host-API ABI is decoupled from the catalog version.** Introduce a new `CB_HOST_ABI_VERSION` constant (C header + Rust mirror) for the trap-channel/host ABI; the host sets `abi_version = CB_HOST_ABI_VERSION` and `cb_runtime_init` validates against that, *not* `CB_CATALOG_VERSION`. A catalog data bump (e.g. FD-029 v6) no longer forces a host re-version.
3. **Init failure is a hard-fail.** When `cb_runtime_init` returns null / ABI-incompatible, the interpreter aborts at startup (panic, like `string_api()`'s `assert!`), since proceeding risks a null `cb_host()` deref inside `cb_rt_*`.
4. **Duplicate detection covers type tags ONLY.** *(Revised during implementation — supersedes the original "tags + c_symbols" decision.)* Duplicate type tags are a real bug: `tag_to_name.insert` silently overwrites, misresolving type references. But `c_symbol` is **not** a uniqueness key: the interpreter dispatches by `fn_ptr` + IR signature (`interp.rs:1412`), and the live catalog deliberately shares one C symbol across distinct CB names (`putpixel` / `putpixel2` → `cb_rt_put_pixel_argb`, `catalog.cpp:335-336`) as an alias. Function *names* are likewise overloaded by design (`abs` → `cb_rt_abs_int` / `cb_rt_abs_float`). So neither name nor symbol is deduped — only tags.
5. **No scoped `#[allow(unsafe_code)]` needed for tests.** *(Revised — the original note assumed the crate denied unsafe.)* `cb-runtime-sys` already sets `unsafe_code = "allow"` crate-wide (`Cargo.toml:11-12`), so the decoder fixtures can construct raw `*const c_char` directly. The workspace-wide `deny` is overridden only for this FFI crate.

## Solution

In `cb-runtime-sys` (+ a touch of C and the interp caller):

- **Validate the handshake on the live path:** `runtime_init` returns `Err` unless `hooks.size >= size_of::<CbRuntimeHooks>()`; move the size check out of the test into the wrapper. On the C side, have `cb_runtime_init` reject hosts whose `size`/`abi_version` don't meet the minimum (`size >= sizeof(CbHostApi)` and `abi_version == CB_HOST_ABI_VERSION`), returning null when rejected.
- **Handle the result at the caller:** in `cb-backend-interp`, hard-fail (panic) if `runtime_init` returns `Err`, and stash the hooks for the `about_to_exit` teardown the FD-015 design reserves. If hooks are intentionally unused for now, document why instead of a bare `let _`.
- **Duplicate detection (type tags only):** check `HashMap::insert`'s returned previous value and return `Err` on a repeated type tag. Do **not** dedup `c_symbol`s or function names — the interpreter dispatches by `fn_ptr`, and the catalog deliberately reuses both (aliases like `putpixel`/`putpixel2`, overloads like `abs`). See Resolved Decision #4.
- **UTF-8 consistency:** make param-name decoding return `Err` on invalid UTF-8 to match the other three sites (or document the leniency).
- **Testability refactor:** split the decode into a private fn taking `&CbCatalog` (or raw parts), leaving `load_catalog` a thin fetch+call. Unit-test the decoder against hand-built `CbCatalog` fixtures (version-mismatch, null pointers, reserved tag `<10`, bad UTF-8, duplicate tags), plus a `type_tag_to_ir_type` table test (each primitive, a custom-tag hit, an unknown-tag miss).
- **Layout pinning:** add static `size_of`/`offset_of` assertions for `CbFuncDesc`/`CbCatalog`/`CbHostApi`/`CbRuntimeHooks` against the C definitions; mark `flags` as intentionally-unconsumed or wire it in.
- **`build.rs`:** use `to_string_lossy()` / gate the UNC strip behind `cfg!(windows)` so a non-UTF-8 path degrades instead of aborting the build.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | `size`/`abi_version` validation in `runtime_init`; duplicate-tag/symbol detection; param-UTF-8 consistency; decoder split for injectable testing; static layout asserts; `type_tag_to_ir_type` + decoder unit tests |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Non-panicking `strip_unc` |
| `runtime/cb_host.cpp` | MODIFY | Reject hosts failing the `size`/`abi_version` minimum in `cb_runtime_init` |
| `crates/cb-backend-interp/src/interp.rs` | MODIFY | Handle the `runtime_init` result (don't discard); stash hooks or document |

## Verification

- `cargo test -p cb-runtime-sys` green, with new tests:
  - `type_tag_to_ir_type` maps each `CB_TYPE_*` correctly; custom-tag hit; unknown-tag `Err`.
  - Decoder against hand-built malformed catalogs hits version-mismatch, null-pointer, reserved-tag, bad-UTF-8, and duplicate-tag branches.
  - `runtime_init` rejects a deliberately undersized/mismatched host.
  - Static layout assertions compile.
- `cargo test --workspace` + `clippy -- -D warnings` green; the live interpreter still initializes against the real runtime.

## Verification Results (2026-06-16)

All steps from the plan above passed:

- **`cargo test -p cb-runtime-sys`** — 19 passed. Covers `type_tag_to_ir_type` (each primitive, custom hit, unknown miss), the decoder against hand-built malformed catalogs (version mismatch, null pointers, reserved/duplicate tags, bad UTF-8 in type and param names, unsupported const tag), the legal alias/overload shapes (`putpixel`/`putpixel2`, `abs`), and `runtime_init` happy-path + ABI-mismatch rejection (the new C reject branch, exercised end-to-end).
- **Static layout pins compiled** on both sides — the Rust `const`-assertions and the C `static_assert`s built clean, confirming `CbHostApi`/`CbRuntimeHooks`/`CbFuncDesc`/`CbCatalog` layouts agree.
- **`cargo test --workspace`** — all suites green, no regressions (incl. `cb-backend-interp`, which now panics on a failed handshake instead of discarding the result).
- **`cargo clippy --workspace --all-targets -- -D warnings`** — exit 0 (only the environmental Windows incremental-compilation file-lock notice, present on untouched crates too).
- **Live interpreter** — `recursion_factorial.cb` (1/1/120/3628800) and `runtime_math.cb` ran correctly through the real `cb_runtime_init` handshake, confirming the validated host channel dispatches actual `cb_rt_*` runtime functions.

## Related

- Surfaced by the post-FD-018 codebase review (cb-runtime-sys area).
- [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) — the `cb_runtime_init` handshake, `CbHostApi`/`CbRuntimeHooks`, and the `about_to_exit` hook this makes load-bearing.
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — the plugin DLL loader that will rely on these ABI guards and duplicate-symbol handling.
- [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md) — `CbStringApi`/catalog versioning precedent.
