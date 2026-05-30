# FD-013: Extending Runtime Support

**Status:** Complete
**Completed:** 2026-05-30
**Priority:** High
**Effort:** High (> 4 hours) — delivered across five batches
**Impact:** Brings the runtime catalog from a handful of stubs to the core subsystems a real CoolBasic program needs — math, strings, system/time, graphics & images, and input — so graphical, interactive programs execute end-to-end through the interpreter.

> **Scope (this FD):** the five batches below — **Math, String, System/Time, Graphics & Images, Input** — all implemented and tested on branch `fd-013-runtime`. The remaining legacy subsystems (**File I/O, Memblock, Text/fonts**) and **DLL-plugin catalog extension** are deferred to a **future "extend runtime further" FD**; File I/O in particular wants the runtime trap channel ([FD-015](FD-015_RUNTIME_TRAP_CHANNEL.md)) first for honest error reporting. This FD was scoped down once the five core batches landed; it no longer tracks the deferred work.

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

### Batch 5 — Input ✅ (branch `fd-013-runtime`)

Keyboard input ported and a mouse surface defined, in a new `runtime/cb_input.cpp`
(renamed from the old `input.c`, now C++ so it can host the state machine). The old
`input.c` had only `MouseX`/`MouseY` (kept). Added:
- **Keyboard** (ported 1:1 from legacy `cb_input.cpp` + `inputinterface.cpp`):
  `KeyDown`, `KeyUp`, `KeyHit`, `EscapeKey` — the full documented surface.
- **Mouse** (new cbcompiler_rs definitions — the legacy runtime exposed *no*
  mouse-button/wheel functions): `MouseDown`, `MouseHit`, `MouseUp`, `MouseZ`,
  `MouseMoveX`, `MouseMoveY`, `MouseMoveZ`.

No joystick (deferred). As with prior batches, **zero Rust-side changes** — all
the new functions are `Int→Int` / `()→Int` catalog entries dispatched through the
generic libffi path (FD-012).

Decisions:
- **Scancodes ported verbatim.** The legacy DirectInput-style CB scancode→Allegro
  keycode table (`sCBKeyMap`: 1=Esc, 16=Q, 30=A, 200=Up, …) is the authoritative
  CoolBasic mapping; ported as-is and documented in `docs/cb_runtime.md`. (The
  syntax reference had no input scancode spec, so the legacy table is the source
  of truth per the "don't invent semantics" rule.)
- **2-bit edge-state machine** (legacy model): per key/button, bit0 = down now,
  bit1 = changed since the frame began. `KeyDown` = down bit; `KeyHit` = `0b11`
  (down + changed); `KeyUp` = `0b10` (up + changed). Mouse buttons reuse it.
- **Frame boundary = `DrawScreen`.** `cb_gfx.cpp` owns the Allegro event queue, so
  `cb_rt_drawscreen` calls the internal hooks `cb_input_frame_begin()` (clears the
  changed bits, zeroes movement deltas) and `cb_input_handle_event(&ev)` per event.
  These hooks live in a new internal `runtime/cb_input.h` (they take `ALLEGRO_EVENT`,
  kept out of the Allegro-free `cb_runtime.h`); they are **not** catalog functions.
  Input state advances only when the program pumps `DrawScreen` — legacy-faithful.
- **Mouse functions are new definitions**, Allegro-backed, mirroring the keyboard
  edge model; buttons 1=left/2=right/3=middle. Documented as cbcompiler_rs additions
  in `docs/cb_runtime.md` (same precedent as Batch 4's `DeleteImage`).
- **`EscapeKey` is a pure query.** The legacy "safe exit" (Esc auto-closes the
  program) is dropped: it needs the not-yet-existing runtime→interpreter trap
  channel and conflicts with Batch 3's clean `Halt` direction.
- **Headless-safe.** With no display, no event sources are registered, so no events
  arrive and every query returns 0 — directly testable in CI.

Tested via `runtime_input.{cb,out}` (headless baseline: every keyboard/mouse query
returns 0, incl. out-of-range scancodes) plus extended `cb-runtime-sys` catalog
assertions (new symbols load with correct arity/types). Interactive behaviour gets a
manual smoke demo (`examples/input_demo.cb`). `cargo test --workspace` green.

## Problem

FD-012 closed the catalog DSL story — adding a runtime function is now a one-line `CB_FN(...)` entry. What we didn't have was the *breadth* of functions a real CoolBasic program expects. At the start of this FD the catalog covered only a handful of stubs:

- System: `print`, `abs(int)`, `abs(float)`
- Math: `sqrt`
- Graphics: `screen`, `drawscreen`, `color`, `line`, `screenwidth`, `screenheight`
- Input: `mousex`, `mousey`
- Test handles: `createtesthandle`, `usetesthandle`

`docs/cb_runtime.md` documents the full legacy surface area: graphics primitives, image handles, pixel ops, text rendering, input (keyboard/mouse/joystick), math library, string functions, file I/O, memblocks, system/window. The legacy implementation lives at `../CBCompiler/Runtime/` (sibling project) and is the reference for behavior, semantics, and edge cases. This FD ports the **core five subsystems** from that surface (see Scope above); the remaining ones are a separate future effort.

## Solution

Port the legacy runtime in batches by subsystem, each batch following the FD-012 pattern: declare the symbol in `cb_runtime.h`, implement in a `.c` / `.cpp` TU (or in `catalog.cpp` for trivial ones), add a `CB_FN(...)` line to `catalog_funcs[]`. New opaque handle types (e.g. `Image`) follow FD-011's pointer-only convention via `type_tag<T*>` specializations and a `CbTypeDesc` entry.

**The five batches delivered by this FD** (details + decisions in Progress above):

1. **Math** ✅ — `runtime/cb_math.cpp`: trig (degrees), `Sqrt`/`Log`/`Log10`, rounding, `Min`/`Max` (int+float), `Distance`/`GetAngle`/`WrapAngle`, seedable `Rnd`/`Rand`/`Randomize`.
2. **String** ✅ — `runtime/cb_string.cpp`: `Upper`/`Lower`/`Trim`/`Left`/`Right`/`StrRemove`/`InStr`/`Chr`/`Hex`, codepoint-aware over the v4 string ABI; string `Len` as a sema intrinsic.
3. **System / Time** ✅ — `runtime/cb_system.cpp`: `Timer`, `Wait`, `MakeError`; `End` as a language statement lowered to an IR `Halt` terminator.
4. **Graphics & Images** ✅ — `runtime/cb_gfx.cpp`: screen management, drawing primitives, pixel ops, and the `Image` opaque handle (`MakeImage`/`LoadImage`/`DrawImage`/…/`DeleteImage`). `Text` excluded (needs fonts).
5. **Input** ✅ — `runtime/cb_input.cpp`: keyboard (`KeyDown`/`KeyUp`/`KeyHit`/`EscapeKey`, ported scancode table + edge-state machine) and a mouse surface (`MouseDown`/`MouseHit`/`MouseUp`/`MouseZ`/`MouseMoveX/Y/Z`); no joystick.

Each batch is backend-agnostic (types live in `cb-ir`/`cb-sema` as catalog-loaded entries, no per-function special-casing), has golden-output fixtures in `crates/cb-driver/tests/fixtures/programs/runtime_*`, and keeps `docs/cb_runtime.md` in sync. The interpreter dispatches all runtime calls through libffi (FD-012); only `cb_rt_print` is intrinsic-overridden for test capture — so new functions route through libffi automatically, with no Rust-side work per function (the one exception was string `Len`, which became a cross-crate intrinsic — see Batch 2).

### Deferred to a future FD

The remaining legacy subsystems are **out of scope** here and will be planned in a separate "extend runtime further" FD:

- **File I/O** — `File` opaque handle (`OpenToRead/Write/Edit`, `ReadLine`/`WriteLine`, `EOF`, filesystem ops, directory search). Wants the runtime trap channel ([FD-015](FD-015_RUNTIME_TRAP_CHANNEL.md)) first so open/read failures raise real errors instead of sentinels.
- **Memblock** — `Memblock` opaque handle (`MakeMemblock`/`DeleteMemblock`, `Peek*`/`Poke*`). Self-contained; the cleanest next batch.
- **Text / fonts** — `Text(x,y,s)` and font loading (the one graphics primitive skipped in Batch 4).
- **`SortArray`** and any other stragglers from `docs/cb_runtime.md`.
- **DLL-plugin catalog extension** (FD-009's `--plugin` loader), which builds on the same ABI seam as FD-015.

## Files to Create/Modify

Delivered (all on branch `fd-013-runtime`):

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime.h` | MODIFY | Prototypes for every new `cb_rt_*` symbol + the `CbImage` opaque handle struct. |
| `runtime/catalog.cpp` | MODIFY | `CB_FN(...)` entries in `catalog_funcs[]`; `CbTypeDesc` + `type_tag<CbImage*>` for the `Image` type. |
| `runtime/cb_math.cpp` | CREATE | Math functions (Batch 1). |
| `runtime/cb_string.cpp` | CREATE | String functions over the v4 string ABI (Batch 2). |
| `runtime/cb_system.cpp` | CREATE | `Timer`/`Wait`/`MakeError` (Batch 3). |
| `runtime/cb_gfx.cpp` | CREATE (from `gfx.c`) | Graphics surface + `Image` opaque handle, on Allegro 5 (Batch 4). |
| `runtime/cb_input.cpp`, `cb_input.h` | CREATE (from `input.c`) | Keyboard + mouse; internal frame/event hooks (Batch 5). |
| `runtime/CMakeLists.txt` | MODIFY | New TUs added to the build. |
| `crates/cb-ir`, `cb-sema`, `cb-backend-interp` | MODIFY | `Halt` terminator + `End` (Batch 3); string `Len` intrinsic → `InstKind::StrLen` (Batch 2). Otherwise no per-function Rust work. |
| `crates/cb-driver/tests/fixtures/programs/runtime_*.{cb,out}` | CREATE | One golden-output fixture per subsystem (`runtime_math`, `runtime_string`, `runtime_system`, `runtime_image`, `runtime_input`). |
| `examples/bounce.cb`, `examples/input_demo.cb` | CREATE | Manual smoke demos for the window/input paths. |
| `docs/cb_runtime.md` | MODIFY | Kept in sync as functions landed; cbcompiler_rs-specific notes recorded. |

## Verification

- `cargo test --workspace` — all existing tests still pass.
- `cargo test -p cb-driver runtime_` — per-subsystem fixture tests prove each new function calls through.
- Compile-time DSL drift checks (FD-012): renaming a `cb_rt_*` symbol without updating its prototype is a link error.
- Manual smoke: a small CoolBasic game (e.g., bouncing-ball demo from the legacy project) runs under `cargo run -p cb-driver -- run examples/<name>.cb`.

## Open questions (resolved)

- **String ABI** — resolved by [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md): strings flow as opaque refcounted `CbString*` (catalog ABI v4). Batch 2 built on it.
- **One umbrella FD vs. per-subsystem** — kept as one umbrella with sub-batches; the umbrella is now capped at five batches and further subsystems get their own FD.
- **Runtime expansion vs. LLVM codegen order** — expansion happened first; interp is the reference per CLAUDE.md, so this is fine. LLVM backend remains empty.
- **Legacy dependencies** — rewritten on top of Allegro 5 (vcpkg) for graphics/input; math/string/system use modern C++ std facilities rather than the legacy Pascal-string runtime / render-target / reference-counter machinery.

No open questions remain for the in-scope work. Cross-cutting follow-ups (the runtime trap channel; File I/O / Memblock / Text / plugins) are tracked in [FD-015](FD-015_RUNTIME_TRAP_CHANNEL.md) and the future extend-runtime FD.

## Related

- [`docs/cb_runtime.md`](../cb_runtime.md) — legacy runtime surface reference.
- `../CBCompiler/Runtime/` — sibling project, original implementation.
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — runtime library architecture (catalog ABI v1).
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque handle types (catalog ABI v2); first real use was the `Image` handle in Batch 4.
- [FD-012](archive/FD-012_CATALOG_CPP_TEMPLATE_DSL.md) — catalog DSL via C++ templates + libffi dispatch (catalog ABI v3).
- [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md) — opaque `CbString*` ABI (v4); Batch 2 built on it.
- [FD-015](FD-015_RUNTIME_TRAP_CHANNEL.md) — runtime trap channel; prerequisite for the deferred File I/O batch.
- **Future "extend runtime further" FD** (not yet created) — File I/O, Memblock, Text/fonts, `SortArray`, and DLL-plugin catalog extension.
