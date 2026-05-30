# FD-013: Extending Runtime Support

**Status:** In Progress
**Priority:** TBD
**Effort:** High (> 4 hours) — likely split across multiple sub-FDs
**Impact:** Brings the runtime catalog closer to feature parity with the legacy CBCompiler runtime, enabling real CoolBasic programs (graphics, input, file I/O, math, string ops, memblocks) to execute end-to-end.

## Progress

### Batch 1 — Math ✅ (branch `fd-013-runtime`)

Full math surface ported to `runtime/cb_math.cpp` and registered in `catalog.cpp`:
`Sin`, `Cos`, `Tan`, `ASin`, `ACos`, `ATan`, `Sqrt` (moved here from `catalog.cpp`),
`Log`, `Log10`, `RoundUp`, `RoundDown`, `Max`/`Min` (int + float overloads),
`Distance`, `GetAngle`, `WrapAngle`, `Rnd`/`Rand` (1- and 2-arg overloads), `Randomize`.

Decisions made while porting (resolving the proof-of-pattern questions):
- **Double, not float.** CB `Float` is f64 at the interpreter boundary, so every
  math fn uses `double` (legacy used 32-bit `float`, which would lose precision
  against the wider ABI). Matches the existing `cb_rt_sqrt`/`cb_rt_line` convention.
- **Degrees.** Trig functions take/return degrees (CoolBasic quirk), preserved
  from legacy via deg↔rad conversion.
- **Overloads work as-is.** Sema's existing arity+exact-type overload resolution
  (the `abs` precedent) handles `Max`/`Min`/`Rnd`/`Rand` with no Rust-side changes —
  just multiple `CB_FN` entries sharing a CB name.
- **Modernized over the legacy port:** PI from `std::numbers::pi` (not a literal);
  randomness from a seedable `std::mt19937_64` + `std::uniform_*_distribution`
  (no modulo bias / short period of legacy `rand() % n`). Fixed default seed keeps
  programs reproducible until `Randomize` is called, matching legacy determinism.

Tested via `crates/cb-driver/tests/fixtures/programs/runtime_math.{cb,out}` (golden
output for deterministic fns; range assertions for the non-deterministic random fns).
`cargo test --workspace` green.

The Math batch validates the per-subsystem pattern. Per the open question below,
remaining subsystems (String, System/Time, Graphics, Input, File, Memblock, Text)
should each become their own sub-FD.

### Batch 2 — String ✅ (branch `fd-013-runtime`)

Documented string library ported to `runtime/cb_string.cpp` (extending the v4 ABI
TU) and registered in `catalog.cpp`: `Upper`, `Lower`, `Trim`, `Left`, `Right`,
`StrRemove`, `InStr` (2- and 3-arg overloads), `Chr`, `Hex`, plus string `Len`.
`Str` stays a sema intrinsic; `Mid`/`Replace`/`Asc` are out of scope (absent from
`docs/cb_runtime.md` and legacy `cb_string.cpp`).

Decisions (the FD-013 string ABI open question, now resolved against the v4 ABI):
- **Codepoint semantics.** The legacy ran on UTF-32 (O(1) char indexing); the v4 ABI
  stores UTF-8, so `Left`/`Right`/`StrRemove`/`InStr`/`Len` walk the bytes to map a
  1-based character index ↔ byte offset (new helpers `cp_len` / `byte_offset_of_cp`
  in `cb_string.cpp`). CB strings are UTF-8 (§3.1) and the docs count "characters".
- **Clamp, never abort.** Legacy `Left`/`Right`/`StrRemove` called a fatal `error()`
  on out-of-range args; there is no runtime→interpreter trap channel, so they
  saturate instead (`Left("hi",5)`→`"hi"`, `n<=0`→`""`). Building a trap mechanism is
  deferred as a separate cross-cutting concern.
- **ASCII-only casing** for `Upper`/`Lower` (bytes ≥0x80 pass through untouched, so
  multibyte sequences survive). Full Unicode casing needs case tables — out of scope.

String `Len` was the one cross-crate change: the array-only `Len` intrinsic now also
accepts a `String` (`cb-sema` check + lower), lowering to a new `InstKind::StrLen`
(`cb-ir`, with printer/verifier arms) which the interpreter answers by codepoint-
counting the handle's bytes in Rust (`cb-backend-interp`). The future LLVM backend
will instead emit a runtime char-length call; interp computing it directly is fine as
the reference impl.

Tested via `crates/cb-driver/tests/fixtures/programs/runtime_string.{cb,out}` (ASCII,
clamp, and multibyte/codepoint cases — e.g. `Len("äbc")`=3, `InStr("äbc","b")`=2) plus
sema unit tests for string `Len`. `cargo test --workspace` green.

### Batch 3 — System/Time ✅ (branch `fd-013-runtime`)

Documented System/Time surface (`docs/cb_runtime.md:245-248`): **Timer, Wait, End,
MakeError**. `Timer`/`Wait`/`MakeError`'s message land in a new `runtime/cb_system.cpp`;
`End` is a language statement, not a runtime call.

Decisions (confirmed with user):
- **Timer = wall-clock** via `std::chrono::steady_clock`, lazy epoch on first call.
  The legacy used `clock()` (CPU time), which drifts from wall time under load — wrong
  for a game loop. Returns `Int` ms.
- **Wait** via `std::this_thread::sleep_for`; `ms <= 0` is a no-op. (Legacy used
  `al_rest`; std avoids dragging Allegro init into a pure sleep.)
- **Clean IR `Halt` terminator** for termination (not C `exit()`). `End` → `Halt(0)`;
  `MakeError(msg)` → a normal call to `cb_rt_make_error` (writes `msg` to stderr) then
  `Halt(1)`. The interpreter stops cleanly and returns the process exit code; nothing
  calls `exit()` from inside a libffi call. Backend-agnostic and observable.

Cross-crate work for the termination machinery:
- `cb-ir`: new `Terminator::Halt { code }` (+ printer/verifier arms).
- `cb-frontend`: `Stmt::End` + a guarded `Kw::End` parser arm (a `Kw::End` followed by
  a block keyword stays a split closer — `End If` etc. — and falls through to the
  existing stray-closer recovery); ast-printer arm.
- `cb-sema`: `Stmt::End` lowers to `Halt(0)`; `MakeError` is a normal catalog
  `RuntimeFn` (String→Void), recognized at lowering by its resolved symbol to append
  `Halt(1)` (reuses existing arg-checking — no special sema).
- `cb-backend-interp` + `cb-driver`: `interpret`/`run`/`exec_loop` now return
  `Result<i32, _>` (the process exit code); `Halt{code}` → `Ok(code)`; the driver maps
  `Ok(code)` → `ExitCode::from(code)`, keeping `Err` for genuine traps/internal errors.

Tested via `runtime_system.{cb,out}` (Timer non-negative/monotonic asserted as booleans,
`Wait(1)`, `End` truncates output), `cli.rs` (`MakeError` → stderr + exit 1; `End` →
exit 0), and parser unit tests (bare `End` → `Stmt::End`; stray `End If` still
diagnoses). `cargo test --workspace` green.

### Batch 4 — Graphics & Images ✅ (branch `fd-013-runtime`)

Full documented graphics surface (`docs/cb_runtime.md:35-92`) **minus `Text`**
ported into a new `runtime/cb_gfx.cpp` (migrated from the old `gfx.c`, now C++ so
it can host the opaque-handle definition). Screen management (`Screen` +
windowed/fullscreen/resizable overload, `Cls`, `ClsColor`, `DrawToScreen`,
`Lock`/`Unlock`, `FPS`), drawing primitives (`Color` +alpha, `Circle`, `Box`,
`Dot`), pixel ops (`PutPixel` ×3 overloads, `GetPixel`), and the **`Image`
opaque handle** (`MakeImage`, `LoadImage`, `DrawImage`, `MaskImage`,
`DrawToImage`, `ImageWidth`/`ImageHeight`, `DeleteImage`).

This is the **first real use of the FD-011 opaque-handle machinery** (previously
exercised only by `TestHandle`). `Image` registers as `CB_TYPE_IMAGE` with
`type_tag<CbImage*>` specializations + a `CbTypeDesc` entry; everything else
(catalog load, sema `RuntimeTypeDef`/`RuntimeFn`, IR `RuntimeType` lowering,
libffi `OpaqueHandle` marshalling) is **already generic — zero Rust-side
changes** this batch. The legacy `RenderTarget`/`Window`/`Image` class hierarchy
was flattened to file-static state + a `struct CbImage { ALLEGRO_BITMAP* }`.

Decisions:
- **Scope minus Text.** `Text(x,y,s)` needs font loading; it stays with the
  future fonts batch (`cb_text.cpp`).
- **`DeleteImage` added + documented.** Not in the legacy surface (which leaked
  until exit); cbcompiler_rs adds explicit cleanup, mirroring
  `MakeMemblock`/`DeleteMemblock`. Documented in `docs/cb_runtime.md`.
- **Overloads** (Screen, Color, ClsColor, Circle, Box, Lock, Unlock, PutPixel,
  MaskImage) are multiple `CB_FN` entries sharing a CB name — sema's existing
  arity+type resolution handles them. The two same-arity `Lock` overloads
  (`Lock(state:Int)` vs `Lock(img:Image)`) are unambiguous (Int never converts
  to a `RuntimeType`).
- **`MakeImage`/`LoadImage` work headless.** When no display is open they fall
  back to `ALLEGRO_MEMORY_BITMAP`, so the opaque-handle + pixel round-trip runs
  in CI without a window.

Known wart (left as-is): `DrawScreen`'s window-close path calls `exit(0)` rather
than a clean IR `Halt` — routing it back needs a runtime→interpreter trap
channel that doesn't exist yet.

Tested via `runtime_image.{cb,out}` (headless: `MakeImage`→`ImageWidth/Height`→
`DrawToImage`+`Lock`+`PutPixel`+`Unlock`→`GetPixel` packed-ARGB round-trip,
all three `Lock` overloads, packed `PutPixel`, `DeleteImage`). Window-dependent
functions get a manual smoke demo (`examples/bounce.cb`). `cargo test --workspace`
green.

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
