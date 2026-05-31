# FD-017: Runtime Module Completeness Pass

**Status:** Pending Verification
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Brings the runtime modules we already ship — Math, String, System/Time, Graphics, Images, Input — up to full parity with the cbEnchanted surface documented in `docs/cb_runtime.md`, without yet introducing any new subsystem.

## Problem

`docs/cb_runtime.md` was rewritten (against the cbEnchanted reference) to describe
the **complete** CoolBasic runtime surface (~345 commands/functions). Our runtime
currently implements only a subset of that surface, and the modules we *do* have
are partial: each implemented module is missing functions that cbEnchanted
provides, and a few of the functions we expose diverge from the spec in signature
or behavior.

This FD is a **completeness pass over the already-implemented modules only**. We do
**not** add new modules here (no Sound, Objects, Camera, Tile Maps, File I/O,
Memblocks, Particles, Video, Fonts/Text, DATA, Encryption, CallDLL). The goal is
narrow: for the six modules that already have a `.cpp` in `runtime/`, fill in the
missing catalog entries and reconcile the documented divergences so each module
matches its section of `cb_runtime.md`.

Implemented modules (source of truth = what `runtime/catalog.cpp` registers today):

| Module | File | State |
|--------|------|-------|
| Math | `runtime/cb_math.cpp` | ~24 funcs, 3 missing |
| String | `runtime/cb_strfuncs.cpp` | 10 funcs, ~12 missing |
| System/Time | `runtime/cb_system.cpp` | 3 funcs, ~8 missing |
| Graphics | `runtime/cb_gfx.cpp` | screen/draw/pixel ops, several missing + signature gaps |
| Images | `runtime/cb_gfx.cpp` | basic image ops, multi-frame + many ops missing |
| Input | `runtime/cb_input.cpp` | keyboard + mouse core, several missing |

## Gap analysis

Legend: ✅ implemented · ❌ missing · ⚠️ implemented but diverges from spec ·
🚧 blocked on an unimplemented subsystem (out of scope for this FD).

### Math (`cb_math.cpp`) — 3 missing

| Function | Status | Note |
|----------|--------|------|
| `CurveValue` | ❌ | `current + (target - current) / smoothness` |
| `CurveAngle` | ❌ | like CurveValue, wraps at 360, shortest path |
| `BoxOverlap` | ❌ | AABB overlap test → Integer |
| `Int` / `Float` | ⚠️ verify | handled as sema intrinsics — confirm `Int(Float)` rounds half-up (`+0.5` then truncate) and `Int(String)` parses, per spec §Math |
| `Rnd`/`Rand` | ⚠️ verify | confirm the `high < low` special-cases match the spec (not a swap) |

### String (`cb_strfuncs.cpp`) — ~12 missing

| Function | Status | Note |
|----------|--------|------|
| `Mid` | ❌ | `len` chars from 1-based `pos` |
| `Replace` | ❌ | replace all; empty `find` → unchanged |
| `LSet` | ❌ | left-align in width `len`, pad right |
| `RSet` | ❌ | right-align in width `len`, truncate to rightmost |
| `Asc` | ❌ | byte/codepoint value of first char |
| `Bin` | ❌ | 32-bit binary string |
| `String` | ❌ | repeat `s` `count` times |
| `Flip` | ❌ | reversed string |
| `StrInsert` | ❌ | insert at 1-based `pos` |
| `StrMove` | ❌ | cut `len` at `pos`, reinsert at `pos+offset` |
| `CountWords` | ❌ | count `sep`-separated words |
| `GetWord` | ❌ | n-th `sep`-separated word |
| `Str` / `Len` | ✅ | sema intrinsics (already covered) |

### System/Time (`cb_system.cpp`) — ~8 missing

| Function | Status | Note |
|----------|--------|------|
| `Date` | ❌ | formatted current date |
| `Time` | ❌ | `HH:MM:SS` |
| `CommandLine` | ❌ | program command-line args |
| `GetEXEName` | ❌ | absolute path of running exe |
| `FrameLimit` | ❌ | caps frame rate (ties into `DrawScreen`) |
| `Errors` | ❌ | enable/disable error-message display |
| `SetWindow` | ⚠️/🚧 | needs window/title control via the gfx display |
| `Crc32` | ⚠️/🚧 | file-path form is in scope; memblock form is blocked on Memblocks |
| `Timer`/`Wait`/`MakeError`/`End` | ✅ | already implemented |

### Graphics (`cb_gfx.cpp`) — missing + signature reconciliations

| Function | Status | Note |
|----------|--------|------|
| `DrawScreen` | ⚠️ | spec is `DrawScreen(cls, vsync)`; we register a 0-arg form. Add the documented args (or overloads). |
| `Screen` (function form) | ❌ | no-arg form returns the screen render-target buffer id |
| `Screen` (depth/mode) | ⚠️ | spec is `Screen(w,h,depth,mode)`; we have 2-arg + 3-arg, no `depth` |
| `ScreenDepth` | ❌ | color depth in bits |
| `GFXModeExists` | ❌ | mode availability test |
| `ScreenGamma` | ❌ | whole-screen gamma |
| `ScreenShot` | ❌ | save screen buffer to file |
| `GetRGB` | ❌ | component of current draw color |
| `PickColor` | ❌ | read screen pixel → draw color |
| `Smooth2D` | ❌ | 2D antialiasing toggle |
| `Ellipse` | ❌ | filled/outline ellipse |
| `PutPixel`/`GetPixel` | ⚠️ | reconcile packed format (we use 32-bit ARGB; spec uses `0xRRGGBB`) and the `buffer` arg; `GetPixel` spec sig is `(x,y,buffer)` vs our `(img,x,y)`; add `PutPixel2`/`GetPixel2` aliases |
| `CopyBox` | ❌ | blit region between render targets |
| `DrawToWorld` / `UpdateGame` / `DrawGame` | 🚧 | depend on camera/object systems — defer |

### Images (`cb_gfx.cpp`) — missing + multi-frame support

| Function | Status | Note |
|----------|--------|------|
| `LoadAnimImage` | ❌ | sprite-sheet slicing — needs multi-frame `CbImage` |
| `MakeImage` (frameCount) | ⚠️ | current form is `(w,h)`; spec adds `frameCount` |
| `DrawImage` (frame, useMask) | ⚠️ | current form is `(img,x,y)`; spec adds `frame`, `useMask` |
| `CloneImage` | ❌ | copy image + properties |
| `DrawGhostImage` | ❌ | alpha 0–100 |
| `DrawImageBox` | ❌ | src sub-rect → dst size |
| `DefaultMask` | ❌ | default mask color for future images |
| `HotSpot` | ❌ | rotation/scale origin |
| `ResizeImage` | ❌ | resize bitmap |
| `RotateImage` | ❌ | rotate bitmap |
| `PickImageColor` | ❌ | read image pixel → draw color (`PickImageColor2` alias) |
| `SaveImage` | ❌ | write image/frame to disk |
| `ImagesOverlap` | ❌ | AABB test between placed images |
| `ImagesCollide` | ❌ | pixel-precise collision |

> Multi-frame sprite-sheet support is the structural prerequisite for
> `LoadAnimImage` and the `frame` parameter on `DrawImage`/`MakeImage`. Decide
> whether to take it on in this FD (see Open Questions).

### Input (`cb_input.cpp`) — several missing

| Function | Status | Note |
|----------|--------|------|
| `GetKey` | ❌ | next queued char code |
| `WaitKey` | ❌ | block until key; function form returns scancode |
| `ClearKeys` | ❌ | clear key states |
| `LeftKey`/`RightKey`/`UpKey`/`DownKey` | ❌ | arrow-key level queries |
| `GetMouse` | ❌ | next queued button-down event |
| `WaitMouse` | ❌ | block until button |
| `PositionMouse` | ❌ | move cursor |
| `ShowMouse` | ❌ | hide/show/image cursor |
| `ClearMouse` | ❌ | clear button states |
| `MouseWX` / `MouseWY` | 🚧 | world-space mouse — needs camera; defer |
| `Input` / `CloseInput` / `SafeExit` | 🚧 | interactive on-screen text entry — defer (sub-feature, not core input) |

## Solution

Per-module, the mechanical recipe is the FD-013 pattern: declare the prototype in
`runtime/cb_runtime_func.h`, implement it in the owning `.cpp`, add one `CB_FN(...)`
line in `runtime/catalog.cpp`. The generic libffi dispatch in `cb-backend-interp`
handles everything that flows through the catalog — no per-function Rust work
unless a function needs intrinsic-level treatment (as `Len`/`Str` did).

Suggested batching (mirrors FD-013's per-subsystem batches; keeps reviews small and
lets golden fixtures land per module):

1. **String** — pure, no Allegro, highest count of cheap wins. Decide the byte-vs-codepoint question (Open Q1) first since it affects every new index-based function.
2. **Math** — `CurveValue`/`CurveAngle`/`BoxOverlap`; verify `Int`/`Float`/`Rnd`/`Rand` semantics.
3. **System/Time** — `Date`/`Time`/`CommandLine`/`GetEXEName`/`FrameLimit`/`Errors`; `SetWindow` + file-path `Crc32` if in scope.
4. **Graphics** — reconcile `DrawScreen`/`Screen`/`PutPixel`/`GetPixel` signatures (Open Q2), then add `ScreenDepth`/`GFXModeExists`/`ScreenGamma`/`ScreenShot`/`GetRGB`/`PickColor`/`Smooth2D`/`Ellipse`/`CopyBox`.
5. **Images** — multi-frame support (Open Q3) gates `LoadAnimImage` + frame params; then the remaining image ops.
6. **Input** — key/mouse queue + arrow keys + cursor control.

Signature changes to existing functions (`DrawScreen`, `MakeImage`, `DrawImage`,
`PutPixel`/`GetPixel`) will touch existing golden fixtures — update them in the same
batch.

## Resolved Decisions

The open questions were resolved with the user (2026-05-31):

- **Q1 — String semantics: keep UTF-8/codepoint.** All new index-based string
  functions are added on the existing UTF-8/codepoint basis (matching
  `Left`/`Right`/`StrRemove`). Byte-semantics convergence is left to a separate FD.
- **Q2 — Pixel format: keep 32-bit ARGB, document divergence.** Retain the current
  32-bit ARGB packing, add `PutPixel2`/`GetPixel2` aliases, and document the
  divergence from the spec's `0xRRGGBB`. No rewrite of existing pixel behavior.
- **Q3 — Multi-frame images: defer to its own FD.** FD-017 is scoped to
  single-frame images. `LoadAnimImage` and the `frame` params on
  `DrawImage`/`MakeImage` are **deferred** (a new FD will add `CbImage` frames).
- **Q4 — Borderline System funcs: defer the plumbing-heavy ones.** Implement the
  pure functions (`Date`, `Time`, `CommandLine`, `GetEXEName`). `SetWindow`,
  `FrameLimit`, `Errors`, and file-path `Crc32` are **deferred** — they need
  window/loop/error-display plumbing that is thin today.
- **Q5 — Confirmed.** The 🚧 items (`DrawToWorld`, `UpdateGame`, `DrawGame`,
  `MouseWX`/`MouseWY`, `Input`/`CloseInput`/`SafeExit`) remain deferred as blocked
  on unimplemented subsystems (camera/objects/interactive input).

### Revised in-scope surface after these decisions

- **Math:** `CurveValue`, `CurveAngle`, `BoxOverlap`; verify `Int`/`Float`/`Rnd`/`Rand`.
- **String:** `Mid`, `Replace`, `LSet`, `RSet`, `Asc`, `Bin`, `String`, `Flip`,
  `StrInsert`, `StrMove`, `CountWords`, `GetWord`.
- **System/Time:** `Date`, `Time`, `CommandLine`, `GetEXEName`.
  (`FrameLimit`/`Errors`/`SetWindow`/`Crc32` deferred.)
- **Graphics:** `ScreenDepth`, `GFXModeExists`, `ScreenGamma`, `ScreenShot`,
  `GetRGB`, `PickColor`, `Smooth2D`, `Ellipse`, `CopyBox`, `Screen()` no-arg form;
  `PutPixel2`/`GetPixel2` aliases (ARGB retained). `DrawScreen(cls,vsync)` args.
- **Images (single-frame only):** `CloneImage`, `DrawGhostImage`, `DrawImageBox`,
  `DefaultMask`, `HotSpot`, `ResizeImage`, `RotateImage`, `PickImageColor`,
  `SaveImage`, `ImagesOverlap`, `ImagesCollide`.
  (`LoadAnimImage`, `MakeImage(frameCount)`, `DrawImage(frame,useMask)` deferred.)
- **Input:** `GetKey`, `WaitKey`, `ClearKeys`, `LeftKey`/`RightKey`/`UpKey`/`DownKey`,
  `GetMouse`, `WaitMouse`, `PositionMouse`, `ShowMouse`, `ClearMouse`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime_func.h` | MODIFY | Add prototypes for all new functions |
| `runtime/cb_strfuncs.cpp` | MODIFY | Mid, Replace, LSet, RSet, Asc, Bin, String, Flip, StrInsert, StrMove, CountWords, GetWord |
| `runtime/cb_math.cpp` | MODIFY | CurveValue, CurveAngle, BoxOverlap |
| `runtime/cb_system.cpp` | MODIFY | Date, Time, CommandLine, GetEXEName, FrameLimit, Errors, (SetWindow, Crc32) |
| `runtime/cb_gfx.cpp` | MODIFY | Graphics + Images additions and signature reconciliations |
| `runtime/cb_input.cpp` | MODIFY | GetKey, WaitKey, ClearKeys, arrow keys, GetMouse, WaitMouse, PositionMouse, ShowMouse, ClearMouse |
| `runtime/catalog.cpp` | MODIFY | One `CB_FN(...)` line per new function/overload |
| `crates/cb-backend-interp/tests/*` | MODIFY | Golden fixtures per module; update fixtures affected by signature changes |
| `crates/cb-driver/tests/cli.rs` | MODIFY | Exit-code / output assertions where relevant |
| `docs/cb_runtime.md` | MODIFY | Update the "Implementation status" section as each module reaches parity |

## Implementation summary (all six batches complete)

Implemented across `runtime/` (catalog + the owning `.cpp`), plus two Rust-side
changes for the String module and the `Int` intrinsic. `cargo test --workspace`
and `cargo clippy --workspace --all-targets -- -D warnings` are green.

- **String** — `Mid`, `Replace`, `LSet`, `RSet`, `Asc`, `Bin`, `String`, `Flip`,
  `StrInsert`, `StrMove`, `CountWords`, `GetWord` (codepoint/1-based). Frontend:
  `String(...)` now parses as a call (`parser.rs` keyword-callee arm +
  `is_expr_start`). **`InStr` not-found reconciled `-1` → `0`** (spec).
- **Math** — `CurveValue`, `CurveAngle`, `BoxOverlap`. **`Rnd`/`Rand` `high<low`
  special case** fixed (no longer `return min`); **`Rand` is now inclusive
  `[low,high]`** (cbEnchanted parity, user-chosen). **`Int(Float)` rounds half-up
  and `Int(String)` trims+parses a leading int** — in `convert_value`, so explicit
  `Int()` and implicit Float→Int coercion agree (`interp.rs`).
- **System/Time** — `Date`, `Time`, `CommandLine`, `GetEXEName`
  (`FrameLimit`/`Errors`/`SetWindow`/`Crc32` deferred).
- **Graphics** — `Screen(w,h,depth,mode)` + no-arg `Screen()`, `ScreenDepth`,
  `GFXModeExists`, `DrawScreen(cls,vsync)`, `GetRGB`, `PickColor`, `Smooth2D`,
  `Ellipse`, `CopyBox`, `PutPixel2`/`GetPixel2` (ARGB). `ScreenGamma`/`ScreenShot`
  best-effort (no portable display gamma / no-op headless).
- **Images (single-frame)** — `CloneImage`, `ResizeImage`, `RotateImage`,
  `PickImageColor`(`2`), `SaveImage`, `DrawGhostImage`, `DrawImageBox`,
  `DefaultMask`, `HotSpot` (per-image form), `ImagesOverlap`, `ImagesCollide`
  (pixel-precise via alpha). `CbImage` gained a hotspot; `MakeImage` clears to
  opaque black.
- **Input** — `GetKey`, `WaitKey`, `ClearKeys`, `LeftKey`/`RightKey`/`UpKey`/
  `DownKey`, `GetMouse`, `WaitMouse`, `PositionMouse`, `ShowMouse`, `ClearMouse`.
  `WaitKey`/`WaitMouse` return 0 immediately with no window (headless-safe).

New/updated golden fixtures (`crates/cb-driver/tests/fixtures/programs/`):
`runtime_string_fd017`, `runtime_gfx_fd017`, `runtime_image_fd017` (new);
`runtime_math`, `runtime_system`, `runtime_input`, `runtime_string`,
`mixed_arithmetic` (updated).

## Verification

- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Per-module golden fixtures (interp output) for each newly added function, following the FD-013 fixture style.
- For functions with documented edge cases (`Replace` empty-find, `Rnd`/`Rand` `high<low`, `Int` half-up rounding, `InStr` not-found = 0, 1-based indexing), add targeted fixtures asserting the spec behavior.
- Cross-check each completed module's catalog entries against its section in `docs/cb_runtime.md` — every non-🚧 row should be ✅.
- Where a signature changed (`DrawScreen`, `MakeImage`, `DrawImage`, `PutPixel`/`GetPixel`), confirm updated fixtures and that sema overload resolution still picks the right symbol.

## Related

- `docs/cb_runtime.md` — the complete cbEnchanted surface (implementation target) and the "Implementation status in cbcompiler_rs" divergence list.
- [FD-013](archive/FD-013_EXTENDING_RUNTIME_SUPPORT.md) — ported the first cut of Math/String/System/Graphics/Input; this FD completes those modules.
- [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md) — the `CbString*` ABI the new string functions build on.
- [FD-016](archive/FD-016_RUNTIME_CORE_FUNCTIONALITY_SPLIT.md) — core/functionality split; new functions live in `cb_runtime` (functionality), not core.
- `../cbEnchanted` — authoritative runtime reference (see memory: cbenchanted-runtime-reference).
