# FD-022: C++ Runtime Allegro-Layer Fixes

**Status:** Open
**Priority:** High
**Effort:** Medium-High (3-6 hours)
**Impact:** Fixes a use-after-free and a collision-detection coordinate bug in the C++ runtime's Allegro layer, makes Linux font resolution actually work, and stands up the first native C++ test target so this 3.7k-LOC memory-sensitive layer stops being tested only indirectly.

## Problem

The post-FD-018 review found the C++ runtime clean overall (the refcounted `CbString` ABI and string library are strong) but flagged real defects in the Allegro layer, all of which are currently exercised only as "does not crash" through CB-program golden fixtures — there is **no native C++ test target**.

1. **Use-after-free: `DeleteFont` leaves a dangling `ALLEGRO_FONT*` in the text queue.** `cb_rt_add_text` snapshots `t.font = current_font` as a raw pointer into a persistent `QueuedText` (`cb_gfx.cpp:987`), re-rendered every `DrawScreen` by `render_queued_texts` (`:912-920`). `cb_rt_delete_font` calls `al_destroy_font(f->font)` (`:1043`) and only repairs the `current_font` global (`:1039-1041`) — it never scans `queued_texts`. The sequence `AddText(font) → DeleteFont(font) → DrawScreen` dereferences freed memory inside `al_draw_text` (the `if (t.font)` guard doesn't help: the pointer is non-null but dangling). The single-frame headless fixture misses it because it `ClearText`s before `DeleteFont`.

2. **`ImagesCollide` mixes world-space and screen-space Y conventions.** The broad-phase AABB runs in cbEnchanted world-space with Y negated (`rect_overlap(x1, -y1, w1, h1, …)`, `cb_gfx.cpp:817`, where `rect_overlap` defines `top=y-h, bottom=y`), but the per-pixel narrow phase computes its scan box in raw screen-space top-left coords (`:819-822`) and samples `al_get_pixel(bmp, x - x1, y - y1)` (`:826-827`). The two phases use inconsistent coordinate systems, so collisions are computed against a region that doesn't match what the AABB gated. `cb_rt_images_overlap` (`:799-805`) uses the world-space convention consistently, making `collide` the outlier.

3. **Fontconfig font resolution is dead code on Linux.** `cb_findfont` has Windows-table / fontconfig (`#ifdef FONTCONFIG_FOUND`) / stub branches, but `FONTCONFIG_FOUND` is **never defined** by `runtime/CMakeLists.txt` (no `find_package(Fontconfig)`). So on every non-Windows build the fontconfig branch (`cb_font.cpp:214-241`) is compiled out and `cb_findfont` always returns `""`; the default-font lookup in `ensure_init` (`cb_gfx.cpp:140-146`) always fails the Courier New resolution and falls back to Allegro's 8×8 builtin. The carefully-ported fontconfig code is unreachable and silently untested.

Lower-severity items folded in:

- **`ResizeImage`/`RotateImage` leave the Allegro draw target on the resized bitmap** (`cb_gfx.cpp:688`/`:726`) instead of restoring the caller's previous target the way `MakeImage` does (`:566-569`) — an avoidable global-state side effect.
- **`GetWord` with `n <= 0` returns the first word instead of empty** (`cb_strfuncs.cpp:494`/`:501`-`509`), inconsistent with the clamp-to-empty discipline of the other FD-017 string functions (e.g. `Mid` returns `""` for `pos<=0`). Not memory-unsafe.
- **`alloc_with_data` calls `std::abort()` on malloc failure** (`cb_string.cpp:83`), bypassing the FD-015 trap channel (`cb_host()->raise_error`) that exists precisely to surface fatal runtime conditions cleanly.

## Solution

In `runtime/`:

- **Font UAF:** on `DeleteFont`, also drop or rebind any `queued_texts` entries whose `font == f->font` (remove them, or repoint to `default_font`); or store an owning/refcounted font reference in `QueuedText` instead of a borrowed raw pointer.
- **`ImagesCollide`:** pick one convention. Since the pixel loop is screen-space top-left, run the AABB in the same screen-space (drop the Y negation for `collide`); or reconcile both phases to world-space. Document the choice.
- **Fontconfig:** add `find_package(Fontconfig)` to `CMakeLists.txt`, define `FONTCONFIG_FOUND` and link the lib when found; otherwise explicitly document that Linux uses the builtin fallback. Either way, end the misleading ported-but-uncompiled state.
- **Image target restore:** capture `al_get_target_bitmap()` at entry and restore at exit, updating `current_target` only when the resized/rotated image *was* the current target.
- **`GetWord`:** guard `n <= 0` to return `make_empty()` up front.
- **OOM:** route `alloc_with_data` failure through `cb_host()->raise_error` with a message before terminating, or document why `abort()` is preferred.
- **Native test target:** add a small gtest/doctest target for the Allegro-free logic — drive `cb_input_handle_event`/`frame_begin` with synthetic `ALLEGRO_EVENT`s and assert `KeyHit`/`KeyUp`/`MouseMove` edges; unit-test the UTF-8 helpers (`cp_len`, `byte_offset_of_cp`, `encode_utf8`, `utf8_chars`) and `rect_overlap`. These need no display and lock in the input/collision semantics the review found undriven.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_gfx.cpp` | MODIFY | Scan/rebind text queue on `DeleteFont`; reconcile `ImagesCollide` coordinates; restore draw target in `ResizeImage`/`RotateImage` |
| `runtime/cb_strfuncs.cpp` | MODIFY | `GetWord(n<=0)` returns empty |
| `runtime/cb_string.cpp` | MODIFY | Route OOM through `cb_host()->raise_error` (FD-015 channel) |
| `runtime/CMakeLists.txt` | MODIFY | `find_package(Fontconfig)` + `FONTCONFIG_FOUND`/link; add C++ test target |
| `runtime/tests/` (new) | CREATE | gtest/doctest for input state machine, UTF-8 helpers, `rect_overlap` |
| `crates/cb-runtime-sys/build.rs` | MODIFY (if needed) | Ensure the test target is excluded from the linked static libs |
| `crates/cb-driver/tests/programs.rs` | MODIFY | Golden fixture asserting a known overlapping vs non-overlapping `ImagesCollide` pair |

## Verification

- New native C++ tests pass headlessly (`ctest` or the chosen runner): keyboard edge transitions, UTF-8 helpers, `rect_overlap`.
- New CB golden fixture distinguishes a true vs false `ImagesCollide` result.
- Manual (real display): `AddText(LoadFont(...)) → DeleteFont → DrawScreen` no longer crashes; on Linux a named font (`LoadFont("Arial")`) resolves via fontconfig instead of silently falling back.
- `cargo test --workspace` + `clippy -- -D warnings` green; runtime still builds via vcpkg/Allegro.

## Related

- Surfaced by the post-FD-018 codebase review (C++ runtime area).
- [FD-018](archive/FD-018_RUNTIME_TEXT_AND_FONT_SUPPORT.md) — the text/font queue and `cb_findfont` introduced here.
- [FD-017](archive/FD-017_RUNTIME_MODULE_COMPLETENESS.md) — `ImagesCollide`/`GetWord`/image transforms introduced here.
- [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) — `cb_host()->raise_error` channel for the OOM path.
- `docs/cb_runtime.md` — collision and string-function semantics.
