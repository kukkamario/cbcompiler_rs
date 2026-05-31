# FD-018: Runtime Text and Font Support

**Status:** Complete
**Completed:** 2026-05-31
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Brings the last major missing graphics subsystem online — on-screen text and TrueType fonts — so CoolBasic programs can render UI, HUDs, and debug overlays.

## Problem

Text rendering was explicitly carved out of FD-013 (Batch 4 graphics) because it
needs font handling: *"`Text` excluded — needs fonts"* and *"`Image` wraps an
Allegro bitmap. `Text` was not yet implemented (fonts batch)."* The entire
**Text & Fonts** surface documented in `docs/cb_runtime.md` (§ Text & Fonts,
lines 259–279) is therefore still unimplemented:

- Immediate text drawing: `Text`, `CenterText`, `VerticalText`
- Queued on-screen text: `Locate`, `AddText`, `ClearText`
- Font management: `LoadFont`, `SetFont`, `DeleteFont` and a new `Font` opaque type
- Metrics: `TextWidth`, `TextHeight`

Without this, programs cannot put any text on the graphics screen — only `Print`/
`Write` to stdout work today. This is the most-requested missing piece for any
real CoolBasic game/UI.

## Solution

Follow the established FD-013/FD-017 runtime recipe: prototype in
`cb_runtime_func.h`, implement in an owning `.cpp`, register one `CB_FN` per CB
function in `catalog.cpp`. The generic libffi dispatch means **no per-function
Rust work** beyond registering the new opaque type — mirroring how `Image`
(`CB_TYPE_IMAGE = 11`) was added.

Key building blocks already in place:

- **Allegro addons are already linked** — `runtime/CMakeLists.txt` links
  `Allegro::allegro_font` and `Allegro::allegro_ttf` (lines 44–45). They just
  need initializing (`al_init_font_addon()` / `al_init_ttf_addon()`) at startup.
- **Opaque-type machinery** from FD-011 — register `Font` as a new custom type
  `CB_TYPE_FONT = 12` in `catalog.cpp` (a `type_tag<CbFont*>` pair + a row in the
  type-descriptor table next to `{ "Image", CB_TYPE_IMAGE }`). Model the `CbFont`
  struct and its lifetime on `CbImage` in `cb_gfx.cpp`.
- **Current draw target / color / DrawToWorld** state already lives in `cb_gfx.cpp`
  — text drawing reuses the same file-static current-buffer and current-color, so
  Text/font code is a natural fit there (or a sibling `cb_text.cpp` that shares
  the gfx state — decide during design).

Open design questions to resolve before/while implementing:

1. **New TU vs. extend `cb_gfx.cpp`?** Text shares so much gfx state (current
   buffer, color, `DrawToWorld`, `Smooth2D`) that a separate TU would need to
   reach into gfx internals. Leaning toward putting it in `cb_gfx.cpp` or
   exposing a small shared header.
2. **Default font.** `Text`/`AddText` with no `SetFont` must still draw. Allegro
   has no built-in TTF; need a bundled fallback font (e.g. a builtin bitmap font
   via `al_create_builtin_font()`) or ship a default TTF. Match cbEnchanted's
   default size/face if known — **consult `docs/cb_syntax.md`/cbEnchanted, don't
   guess**.
3. **`LoadFont` flags.** `bold`/`italic`/`underline` + `size`; map to
   `al_load_ttf_font_stretch`/flags. Family-name vs. file-path resolution (the
   spec says "family name or file path") — likely file-path only initially,
   family-name lookup deferred.
4. **Queued text (`Locate`/`AddText`/`ClearText`) lifecycle.** When is queued
   text flushed and cleared — every `DrawScreen` like the input queues? Confirm
   against cbEnchanted semantics.
5. **Headless behavior.** Like the rest of gfx, metric/draw calls must no-op or
   return sane values when there's no display (so tests run headless). `TextWidth`/
   `TextHeight` need a font loaded even headless — the builtin font should work.
6. **`Smooth2D` interaction** — `LoadFont` "honors `Smooth2D`" per the spec.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_gfx.cpp` (or new `runtime/cb_text.cpp`) | MODIFY/CREATE | `CbFont` struct + impls for the 11 Text & Font functions; init font addons |
| `runtime/cb_runtime_func.h` | MODIFY | `cb_rt_*` prototypes for the new functions; `CbFont` fwd decl |
| `runtime/catalog.cpp` | MODIFY | `CB_TYPE_FONT = 12`, `type_tag<CbFont*>` pair, `{ "Font", CB_TYPE_FONT }` row, one `CB_FN` per function |
| `runtime/CMakeLists.txt` | MODIFY (maybe) | Add new TU if a separate `cb_text.cpp` is created |
| `docs/cb_runtime.md` | MODIFY | Drop the "not yet implemented" caveat once shipped |
| `crates/cb-driver/tests/...` golden fixtures | CREATE | `runtime_text_fd018` golden(s) |

## Verification

- `cargo build` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` all green.
- New golden fixture program exercising `LoadFont`/`SetFont`/`Text`/`CenterText`/
  `VerticalText`/`Locate`/`AddText`/`ClearText`/`TextWidth`/`TextHeight` runs
  headless without crashing and produces stable output (metrics deterministic).
- Manual smoke test with a real display: run an example that draws text on screen
  and visually confirm placement/centering/color/font.
- `Font` opaque handle round-trips: assignable, null-comparable, rejected for
  arithmetic by sema (inherits FD-011 custom-type rules for free).

## Implementation Notes

Delivered as designed. Decisions confirmed with the user: opaque `CbFont*`
(`CB_TYPE_FONT = 12`); **full faithful** `findfont` port; **builtin-font
fallback** for the default font.

- **`runtime/cb_font.cpp` + `cb_font.h`** — `cb_findfont(name, bold, italic)`:
  Windows ports cbEnchanted's 4 family→file tables verbatim under `%WINDIR%\Fonts`
  (guarded against a null `WINDIR`); non-Windows uses fontconfig under
  `FONTCONFIG_FOUND`, else returns `""`.
- **`runtime/cb_gfx.cpp`** — `struct CbFont`; font/ttf addon init + default-font
  load (Courier New 12pt monochrome → `al_create_builtin_font()` fallback) in
  `ensure_init`; the 11 `cb_rt_*` functions; persistent `queued_texts` rendered by
  `render_queued_texts()`, called in `do_draw_screen` just before the flip.
  `VerticalText` iterates UTF-8 **codepoints** (cbEnchanted iterated raw bytes);
  implemented as documented `(x, y, s)`, with the cbEnchanted `(y, x, s)` swap
  noted in a comment.
- **`catalog.cpp` / `cb_runtime_func.h`** — `CbFont` typedef, `CB_TYPE_FONT` tag +
  `type_tag` pair, `"Font"` type row, 11 `CB_FN` rows, 11 prototypes.
- **Rust:** one real fix surfaced by `LoadFont`'s failure path — `ffi.rs` now maps a
  **null opaque return to `Value::Null`** (was `OpaqueHandle(0)`), so the
  documented "0 on failure" / `= Null` comparison works for every opaque-returning
  runtime function (also benefits `LoadImage`). No other Rust changes; `Font`
  registers from the catalog like `Image`.

**Verified:** `cargo build`, `cargo test --workspace` (all green, incl. new
`runtime_text_fd018`), `cargo clippy --workspace --all-targets -D warnings` clean.
`Font` round-trips: declarable, `LoadFont`-returned, `= Null`-comparable, and
arithmetic rejected (E0301). The `runtime_text_fd018` golden runs headless with
font-independent assertions. **Remaining:** the manual real-display smoke test
(placement/centering/color/font face, AddText persistence) is not yet done.

## Related

- `docs/cb_runtime.md` § Text & Fonts (lines 259–279) — authoritative surface
- FD-013 (Extending Runtime Support) — deferred Text/fonts here; established the recipe
- FD-017 (Runtime Module Completeness) — most recent application of the recipe; image hotspot/draw patterns to mirror
- FD-011 (Runtime Custom Types) — `Font` opaque-handle machinery
- FD-012 (Catalog DSL) — `CB_FN` registration pattern
