# FD-013: Extending Runtime Support

**Status:** Open
**Priority:** TBD
**Effort:** High (> 4 hours) — likely split across multiple sub-FDs
**Impact:** Brings the runtime catalog closer to feature parity with the legacy CBCompiler runtime, enabling real CoolBasic programs (graphics, input, file I/O, math, string ops, memblocks) to execute end-to-end.

## Problem

FD-012 closed the catalog DSL story — adding a runtime function is now a one-line `CB_FN(...)` entry. What we don't have is the *breadth* of functions a real CoolBasic program expects. Today's catalog covers:

- System: `print`, `abs(int)`, `abs(float)`
- Math: `sqrt`
- Graphics: `screen`, `drawscreen`, `color`, `line`, `screenwidth`, `screenheight`
- Input: `mousex`, `mousey`
- Test handles: `createtesthandle`, `usetesthandle`

`docs/cb_runtime.md` documents the legacy surface area: graphics primitives, image handles, pixel ops, text rendering, input (keyboard/mouse/joystick), math library, string functions, file I/O, memblocks, system/window. The legacy implementation lives at `../CBCompiler/Runtime/` (sibling project) and can be used as a reference for behavior, semantics, and edge cases.

## Solution

Port the legacy runtime in batches by subsystem, each batch following the FD-012 pattern: declare the symbol in `cb_runtime.h`, implement in a `.c` / `.cpp` TU (or here in `catalog.cpp` for trivial ones), add a `CB_FN(...)` line to `catalog_funcs[]`. New opaque handle types (`Image`, `File`, `Memblock`) follow FD-011's pointer-only convention via `type_tag<T*>` specializations and a `CbTypeDesc` entry.

Likely batching (each could become its own sub-FD if scope warrants):

1. **Math** — full set from `cb_math.cpp` (`Sin`, `Cos`, `Tan`, `ATan`, `ATan2`, `Min`, `Max`, `Floor`, `Ceil`, `Pow`, `Log`, `Exp`, `Rand`, `RandSeed`, …). Pure functions, no state — cheapest first batch.
2. **String** — `cb_string.cpp` / `lstring.*` (`Len`, `Left`, `Right`, `Mid`, `Upper`, `Lower`, `Trim`, `Replace`, `InStr`, `Str`, `Chr`, `Asc`, …). Touches the string ABI — confirm `CB_TYPE_STRING` ownership / lifetime rules before porting.
3. **System / Time** — `cb_system.cpp` (`Timer`, `WaitKey`, `Wait`, `RND`, `Command`, …).
4. **Graphics** — finish `gfx.c`: `Cls`, `ClsColor`, `Circle`, `Box`, `Dot`, `Text`, `Lock`/`Unlock`, `PutPixel`/`GetPixel`, `FPS`, plus the `Image` opaque-handle type from `cb_image.cpp`.
5. **Input** — finish `input.c`: keyboard (`KeyDown`, `KeyHit`, `GetKey`), full mouse (`MouseDown`, `MouseHit`, `MouseWheel`, …), joystick.
6. **File I/O** — new `File` opaque handle from `cb_file.cpp` / `fileinterface.cpp` (`OpenFile`, `CloseFile`, `ReadLine`, `WriteLine`, `EOF`, …).
7. **Memblock** — new `Memblock` opaque handle from `cb_mem.cpp` / `memblock.cpp` (`MakeMEMBlock`, `PeekByte`, `PokeByte`, `PeekShort`, …).
8. **Text rendering / fonts** — `cb_text.cpp` / `textinterface.cpp` if scoped in.

Each batch needs:
- Backend-agnostic surface: types live in `cb-ir`/`cb-sema` as catalog-loaded entries, no special-casing per function.
- Test fixtures in `crates/cb-driver/tests/fixtures/` exercising the new calls through the interpreter (FD-010 is the reference impl per CLAUDE.md).
- Update `docs/cb_runtime.md` if the legacy doc disagrees with what we actually expose.

The interpreter dispatches all runtime calls through libffi (FD-012); only `cb_rt_print` is currently intrinsic-overridden for test capture. New functions get routed through libffi automatically — no Rust-side work needed per function.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime.h` | MODIFY | Add prototypes for every new `cb_rt_*` symbol, plus new opaque `Cb*` handle structs. |
| `runtime/catalog.cpp` | MODIFY | Add `CB_FN(...)` entries in `catalog_funcs[]`; add `CbTypeDesc` entries + `type_tag<T*>` specializations for new opaque types. |
| `runtime/cb_math.cpp` (new) | CREATE | Math functions ported from `../CBCompiler/Runtime/cb_math.cpp`. |
| `runtime/cb_string.cpp` (new) | CREATE | String functions; needs decision on string ABI / ownership. |
| `runtime/cb_gfx.cpp`, `cb_image.cpp` | CREATE / extend existing `gfx.c` | Finish graphics surface; Image opaque handle. |
| `runtime/cb_input.cpp` | CREATE / extend existing `input.c` | Full keyboard + mouse + joystick. |
| `runtime/cb_file.cpp`, `cb_mem.cpp`, `cb_text.cpp` | CREATE | File / Memblock / Text subsystems. |
| `runtime/CMakeLists.txt` | MODIFY | Add new TUs to the build. |
| `crates/cb-driver/tests/fixtures/runtime/*` | CREATE | One fixture per subsystem (golden-output `.cb` programs). |
| `docs/cb_runtime.md` | MODIFY | Keep in sync as functions land; mark which are implemented. |

## Verification

- `cargo test --workspace` — all existing tests still pass.
- `cargo test -p cb-driver runtime_` — per-subsystem fixture tests prove each new function calls through.
- Compile-time DSL drift checks (FD-012): renaming a `cb_rt_*` symbol without updating its prototype is a link error.
- Manual smoke: a small CoolBasic game (e.g., bouncing-ball demo from the legacy project) runs under `cargo run -p cb-driver -- run examples/<name>.cb`.

## Open questions

- String ABI: legacy runtime uses `lstring` (Pascal-style length-prefixed). Current catalog only has `CB_TYPE_STRING = const char*`. Decide ownership / lifetime before porting `cb_string.cpp`.
- Should each subsystem be its own FD (FD-014 Math, FD-015 String, …) or one umbrella? Suggest splitting once Math lands as a proof-of-pattern.
- LLVM backend (`cb-backend-llvm`) is still empty — does the runtime expansion happen before or after LLVM codegen exists? Per CLAUDE.md, interp is the reference, so the answer is "before is fine."
- Which legacy dependencies (allegro, Pascal-string runtime, render targets, reference counter) carry over vs. get rewritten on top of Allegro 5 (already wired via vcpkg)?

## Related

- [`docs/cb_runtime.md`](../cb_runtime.md) — legacy runtime surface reference.
- `../CBCompiler/Runtime/` — sibling project, original implementation.
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — runtime library architecture (catalog ABI v1).
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque handle types (catalog ABI v2).
- [FD-012](archive/FD-012_CATALOG_CPP_TEMPLATE_DSL.md) — catalog DSL via C++ templates + libffi dispatch (catalog ABI v3).
