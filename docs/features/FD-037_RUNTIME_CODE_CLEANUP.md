# FD-037: C++ Runtime Code Cleanup — `extern "C"` Hygiene, Namespaces, Comments

**Status:** Pending Verification
**Priority:** Medium
**Effort:** High (> 4 hours — spans ~24 hand-written runtime files)
**Impact:** Readable, idiomatic C++ runtime with correct linkage hygiene; lowers the cost of every future runtime FD (Sound, Video, Particles, File I/O…) and makes the code self-documenting instead of requiring a second source tree open to follow.

> This is a **pure cleanup / refactor — no behavior change and no ABI change.** The CB-visible catalog surface, the FFI handshake, and the SDK-free build all stay byte-for-byte compatible. Per [[prefer-tests-over-refactor-when-correct]] the code is believed correct; this FD improves its form, not its function.

## Problem

The hand-written C++ runtime in `runtime/` grew organically across FD-009 → FD-036. It works and is well-tested, but the style is inconsistent and harder to read than it should be. Three concrete issues, all confirmed by a survey of the 24 hand-written source files (vcpkg/build artifacts excluded):

### 1. `extern "C"` is applied far more broadly than the ABI requires

`extern "C"` appears **337 times** across the hand-written sources. Only a subset genuinely crosses the FFI boundary and needs C linkage:

- **~279 catalog-registered `cb_rt_*` functions** — dispatched by the interpreter (and later the LLVM backend) via libffi, registered as `CB_FN(...)` rows in `catalog.cpp`. These **must** stay `extern "C"`.
- **~7 string primitives** routed through `CbStringApi` (`cb_rt_string_retain/release/from_literal/len/data/concat`, `cb_runtime_string_api`) — cross the boundary via a function-pointer struct, must stay `extern "C"`.
- **3 handshake entry points** — `cb_runtime_get_catalog`, `cb_runtime_init`, `cb_host` — must stay `extern "C"`.

But a whole population of **internal cross-TU glue** is marked `extern "C"` for no reason — these functions are **never in the catalog** and are only ever called by other C++ TUs inside the runtime itself. Examples:

| File | `extern "C"` functions that don't need it |
|------|-------------------------------------------|
| `cb_object.cpp` | `cb_run_collision_checks`, `cb_object_pick_at`, `cb_objects_update_all`, `cb_objects_render_all` |
| `cb_camera.cpp` | `cb_camera_render_transform`, `cb_camera_world_transform`, `cb_camera_draw_cmd_to_world`, `cb_camera_screen_to_world`, `cb_camera_update_follow`, `cb_camera_zoom`, … |
| `cb_map.cpp` | `cb_map_active`, `cb_map_active_data`, `cb_map_tick_animation`, `cb_map_render_layer` |
| `cb_gfx.cpp` | `cb_gfx_display`, `cb_gfx_event_queue`, `cb_gfx_design_size`, `cb_gfx_window_size`, `cb_gfx_image_bitmap`, `cb_gfx_image_pristine` |
| `cb_input.cpp` | `cb_input_frame_begin`, `cb_input_handle_event` |

~25+ such functions. The `extern "C"` buys nothing (no FFI crossing, no `dlsym`) — it's just being used as a "stable symbol" convention. Worse, several are **redundantly hand-declared `extern "C"` in multiple TUs** instead of via a shared header: `cb_gfx_image_bitmap` is forward-declared in `cb_map.cpp`, `cb_object.cpp`, and `tests/test_masking.cpp` on top of its `cb_gfx.cpp` definition.

### 2. Almost no use of C++ namespaces; one file is an outlier

- Exactly **one named namespace** exists in the entire runtime: `namespace cb_catalog` in `catalog.cpp` (the template DSL).
- Every other module hides its internals in an **anonymous namespace** *or* file-`static`, then exposes cross-TU functions as `extern "C"` free functions in the global namespace.
- **`cb_gfx.cpp` (the largest file, ~1260 lines) uses no namespace at all** — every helper is a bare file-`static`, inconsistent with the anonymous-namespace style every other module uses.

There is no `cb::` namespace organizing the subsystems; the `cb_<subsystem>_` prefix on glue functions is a hand-maintained stand-in for namespacing.

### 3. Comments are verbose and lean on the previous implementation

**~207 comment references** to `cbEnchanted` / `legacy` / `ported` / `port of` across **20 of the 24** files (densest: `cb_gfx.cpp` 43, `cb_object.cpp` 35, `cb_map_data.h` 18, `cb_strfuncs.cpp` 15, `cb_math.cpp` 12). The style frames behavior in terms of a prior implementation rather than explaining the reasoning, and frequently cites external absolute paths that only make sense with a second tree open:

- `cb_string.cpp:3` — `// Port of legacy LString / LStringData (G:\projects\CBCompiler\Runtime\…`
- `cb_gfx.cpp:3` — `// Ported from the legacy ../CBCompiler/Runtime/cb_gfx.cpp + cb_image.cpp + …`
- `cb_math.cpp:4-17` — contrasts "Semantics preserved from the legacy implementation" vs "Improvements over the legacy port"
- `cb_object.cpp:116` — `bool animLooping = false;   // cbEnchanted leaves uninit; default false`
- `cb_font.h:7` — `// Faithful port of cbEnchanted's findfont (../cbEnchanted/src/utilwin.cpp…`

A comment should explain **why** the code is the way it is, not what some other codebase did. (Reference context: the true behavioral reference is the **original, closed-source CoolBasic** compiler/runtime; `../cbEnchanted` is the best available open proxy — see [[cbenchanted-reference-location]] — but the code's own comments shouldn't read like a porting diff.)

### Minor: inconsistent constant naming

Some constants are `k`-camelCase (`kMinZoom`, `kPi` in `cb_camera.cpp`), some are `CB_`-SCREAMING_SNAKE (`CB_OOM`, `CB_EMPTY_STRING_INSTANCE` in `cb_string.cpp`). Unified to `k_snake_case` (see Solution D).

## Solution

Four coordinated workstreams, applied module-by-module. **No catalog row, `cb_rt_*` symbol, or handshake signature changes** — only internal linkage, namespacing, and comments.

### A. `extern "C"` diet

Keep `extern "C"` on exactly three categories: catalog `cb_rt_*` functions, the `CbStringApi` string primitives, and the three handshake entry points. Strip it from every internal cross-TU glue function. Replace the redundant per-TU `extern "C"` forward declarations with a single shared declaration in the owning per-module header (`cb_gfx.h` etc.) that the consumers `#include`. (Note: `cb_gfx.cpp` has no header today — the glue is hand-declared in consumers; this FD adds one.)

### B. Namespaces

Move internal glue and file-local helpers into proper C++ namespaces instead of the `extern "C"` + `cb_<subsystem>_`-prefix convention. Use **per-subsystem sub-namespaces under a top-level `cb`**: `cb::gfx`, `cb::object`, `cb::map`, `cb::camera`, `cb::input`, `cb::font`. Once a function lives in its subsystem namespace the `cb_<subsystem>_` prefix is redundant and **drops** — `cb_gfx_image_bitmap` → `cb::gfx::image_bitmap`, `cb_objects_render_all` → `cb::object::render_all`, etc. File-local helpers and state stay in an anonymous namespace nested inside the subsystem namespace (or remain file-`static`). Standardize `cb_gfx.cpp` onto this so it stops being the lone bare-`static` outlier. The `cb_catalog` template DSL namespace stays as-is.

### C. Comments

Rewrite comments to explain reasoning and design choices on their own terms. Remove external absolute paths (`G:\…`, `../cbEnchanted/…`) and "ported from / legacy / improvements over the port" framing. Where a comment documents a genuine behavioral source-of-truth (a CoolBasic quirk we deliberately replicate), phrase it as the **CoolBasic semantic** being honored — and compare against **original CoolBasic**, not cbEnchanted. Default to removing cbEnchanted pointers; a minimal one may stay **only where it is genuinely critical to explain the reasoning** (a non-obvious behavior whose only available open proxy is cbEnchanted). Keep sparingly.

### D. Naming consistency

Settle internal compile-time constants on **`k_snake_case`** (e.g. `kMinZoom` → `k_min_zoom`, `kPi` → `k_pi`, and the file-local `CB_OOM` / `CB_EMPTY_STRING_INSTANCE` → `k_oom` / `k_empty_string_instance`, converting to `constexpr` where they're currently `#define`s). The public `CB_*` ABI/wire macros in the shared headers (`CB_TYPE_INT`, `CB_CATALOG_VERSION`, `CB_FN`, `CB_NO_ALLEGRO`, …) **stay** SCREAMING_SNAKE — they're load-bearing public macros, not internal constants. Leave `cb_rt_*` and `Cb`-PascalCase type names as-is; they're already consistent and ABI-load-bearing.

### Sequencing

Given ~24 files, do this **phased per subsystem** rather than one mega-diff — **one FD branch, each phase a separate commit**, each phase independently building + testing green. Phases: (1) Allegro-free core (`cb_string`, `cb_strfuncs`, `cb_math`, `cb_system`, `cb_host`, `catalog`) — mostly comments + constants, little/no `extern "C"` glue here; (2) gfx + font; (3) camera + map; (4) object + game loop.

## Decisions

The load-bearing style questions, resolved with the user before implementation:

- **D1 — Namespace scheme.** Per-subsystem sub-namespaces under a top-level `cb` (`cb::gfx`, `cb::object`, `cb::map`, `cb::camera`, `cb::input`, `cb::font`). The now-redundant `cb_<subsystem>_` prefix **drops** once a function is namespaced (`cb::gfx::image_bitmap`).
- **D2 — cbEnchanted references.** Default to removing them. A minimal pointer may stay **only where it is genuinely critical to explain the reasoning**, and even then prefer comparing against **original CoolBasic** rather than cbEnchanted.
- **D3 — Constant naming.** `k_snake_case` for internal compile-time constants; public `CB_*` ABI macros stay SCREAMING_SNAKE.
- **D4 — Phasing.** One FD branch; each phase is a separate commit.
- **D5 — `cb_findfont`.** Folded into the namespace scheme as `cb::font::find()`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_object.{cpp,h}`, `cb_object_data.h` | MODIFY | Drop `extern "C"` from glue (`cb_objects_render_all`, etc.); namespace internals; rewrite comments |
| `runtime/cb_camera.{cpp,h}`, `cb_camera_math.h` | MODIFY | Same; rename `k`-camelCase constants (`kPi`→`k_pi`, `kMinZoom`→`k_min_zoom`) |
| `runtime/cb_map.{cpp,h}`, `cb_map_data.h` | MODIFY | Same (18 comment refs in `cb_map_data.h`) |
| `runtime/cb_gfx.cpp` + new `cb_gfx.h` | MODIFY / CREATE | De-`extern "C"` glue; add `cb_gfx.h` declaring the `cb::gfx` glue currently hand-declared in 3 TUs; move file-`static` helpers into `cb::gfx` (largest file, 43 comment refs) |
| `runtime/cb_input.{cpp,h}` | MODIFY | De-`extern "C"` `cb_input_frame_begin`/`handle_event`; namespace; comments |
| `runtime/cb_font.{cpp,h}` | MODIFY | Comments; move `cb_findfont` → `cb::font::find()` |
| `runtime/cb_string.cpp`, `cb_strfuncs.cpp` | MODIFY | Comments (strip `G:\…` path); confirm string primitives stay `extern "C"` |
| `runtime/cb_math.cpp`, `cb_system.cpp`, `cb_host.cpp` | MODIFY | Comments (strip "ported/legacy" framing) |
| `runtime/catalog.cpp`, `cb_runtime_func.h`, `cb_runtime_core.h` | MODIFY (light) | Comments only; **no `cb_rt_*`/ABI changes** |
| `runtime/tests/test_masking.cpp` (+ other tests) | MODIFY | Replace hand-rolled `extern "C"` glue forward-decls with the new shared header include |
| `docs/cb_runtime.md` | MODIFY (maybe) | Reflect any documented naming/structure conventions |

No Rust crates are expected to change (catalog ABI is untouched), so `cb-runtime-sys`'s catalog-content asserts should pass unchanged — that's a key correctness check, not a modification site.

## Verification

Because this is a no-behavior-change refactor, verification is about proving *nothing moved*:

- **Full Allegro Release build** (vcpkg + Allegro) compiles with **zero errors/warnings** — the `extern "C"`→namespace migration is the kind of change that surfaces ODR/linkage mistakes at link time, so a clean full link is the primary gate.
- **SDK-free `cargo test --workspace`** green (`CB_RUNTIME_FORCE_SDK_FREE=1`) — confirms the Allegro-free core still builds via `cc` and the catalog still decodes (`cb-runtime-sys` ABI/catalog-content asserts unchanged).
- **Native C++ tests** `ctest` green (the `CB_RUNTIME_TESTS` gtest target, incl. `test_masking` which forward-declares the migrated `cb_gfx_image_bitmap` glue).
- **`cargo test --workspace`** full suite green, no golden/snapshot drift (no runtime behavior changed).
- **Grep gates:** after the diet, `extern "C"` appears only on catalog `cb_rt_*` / string primitives / handshake; per the Q2 decision, cbEnchanted/external-path references reduced to the agreed residual.
- **Linux CI** (`.github/workflows/ci.yml`, GCC, SDK-free) green — proves the namespace changes compile cross-compiler.
- `clippy -D warnings` / `cargo fmt --all --check` clean for any (likely zero) Rust touched.

## Implementation

Implemented on branch `fd-037-runtime-code-cleanup`, one commit per phase (D4):

- **Phase 1 — Allegro-free core** (`7efc0f6`): comments + constants in `cb_string.cpp`, `cb_strfuncs.cpp`, `cb_math.cpp`, `cb_system.cpp`, `catalog.cpp`. `CB_EMPTY_STRING_INSTANCE`/`CB_OOM` → `k_empty_string_instance`/`k_oom`. (`cb_host.cpp` needed nothing.)
- **Phase 2 — gfx + font** (`ee6db6b`): new `cb_gfx.h` declaring the `cb::gfx` glue (was hand-declared in 3 TUs); `cb_gfx.cpp` body moved into `cb::gfx`; glue de-`extern "C"`'d + de-prefixed (the redundant `cb_gfx_image_pristine` wrapper dropped, the `image_pristine` helper promoted); state vars `display`/`event_queue` → `g_display`/`g_event_queue` to avoid colliding with the accessors; `cb_findfont` → `cb::font::find`.
- **Phase 3 — camera + map** (`82eb52e`): `cb::camera` / `cb::map` glue; `kPi`/`kMinZoom` → `k_pi`/`k_min_zoom`; DrawToWorld flag vars `g_`-prefixed. The pure, unit-tested header APIs (`cb_camera_math.h`, `cb_map_data.h`) are not `extern "C"` glue and their test consumers are outside the FD's edit set, so they got **comment cleanup only** — their `cb_*`/`CbAffine`/`CbMapData` names stay.
- **Phase 4 — object + game loop + input** (`7317a6d`): `cb::object` / `cb::input` glue; remaining comment pass (incl. `cb_object_data.h`, `cb_collision_data.h`, the sibling `cb_geom.h`, the light `cb_runtime_func.h`, and the `test_camera`/`test_map`/`test_object` comments).

**Decision refinement during implementation:** modules are namespaced by wrapping the whole TU body in `namespace cb::<subsystem> { … }`; the `cb_rt_*` catalog entry points keep `extern "C"` *inside* the namespace (a C-linkage function declared in a namespace refers to the same symbol as its global prototype — [dcl.link]), so the ABI symbol is unchanged while the internals are namespaced. The opaque type structs (`CbImage`/`CbFont`/`CbMap`/`CbObject`) stay in the global namespace to match their `cb_runtime_func.h` forward declarations.

**Verification (all green):** full Allegro **Release** build with zero warnings; `cargo test --workspace` (Allegro) 696 passed; SDK-free `cargo test --workspace` green (`cb-runtime-sys` catalog asserts 20 passed, unchanged); native `ctest` 57/57. Grep gates: `extern "C"` appears only on catalog `cb_rt_*` / the `CbStringApi` string primitives / the three handshake entry points; zero `cbEnchanted`/`legacy`/`ported`/external-path references remain in the hand-written runtime. `docs/cb_runtime.md` needs no change — its cbEnchanted references are legitimate language-surface citations, not source-comment porting framing.

## Related

- [[cbenchanted-reference-location]] — the behavioral reference proxy lives at `../cbEnchanted`
- [[prefer-tests-over-refactor-when-correct]] — code is correct; this is a form-not-function pass, so favor preserving + retesting over rewriting logic
- FD-036 (Game-Object Runtime Cluster) — added the bulk of `cb_object`/`cb_map`/`cb_camera` and most of the cbEnchanted-referencing comments; phased-PR model to mirror
- FD-033 (SDK-free build) — the `CB_NO_ALLEGRO` partition that the core-vs-functionality split must keep intact
- FD-016 (Core/Functionality split) — established `cb_runtime_core.h` (plugin SDK ABI) vs `cb_runtime_func.h` (catalog prototypes); this FD cleans within those seams, doesn't move them
- `docs/cb_runtime.md`, `docs/cb_syntax.md`
