# FD-036: Game-Object Runtime Cluster — Multi-frame Images, Camera, Tile Maps, Objects & Game Loop

**Status:** In Progress (Phase 4 of 5 complete)
**Priority:** Medium
**Effort:** High (> 4 hours; multi-PR — landed phase by phase)
**Impact:** Brings CoolBasic's central game-programming abstractions online — multi-frame sprites, the camera, tile maps, sprite **Objects**, collision, and the `UpdateGame`/`DrawGame` loop — unlocking the bulk of idiomatic CoolBasic game code.

## Problem

The largest unimplemented region of the runtime is the **game-object cluster**: 2D sprite **Objects** plus the subsystems they sit on top of — multi-frame sprite **Images**, the **Camera** (world↔screen transform), and **Tile Maps**. The full surface is already specified in [`docs/cb_runtime.md`](../cb_runtime.md) (§Objects, §Camera, §Tile Maps, and the `UpdateGame`/`DrawGame` game-loop functions in §Graphics).

These subsystems are **tightly entangled** (see the dependency analysis below), so they are grouped into this single phased FD rather than split across independent FDs. The independent subsystems — Sound, Video, Particles, File I/O, Memory Blocks, DATA, Encryption, `CallDLL`, and the plumbing System funcs — depend on none of this and are tracked as separate follow-on FDs (see Related → Roadmap).

Without this cluster, almost no idiomatic CoolBasic game program can run on either backend.

### Dependency analysis (why this order)

```
Images (single-frame) ── done (FD-013/017)
   └─► Multi-frame sprite sheets (LoadAnimImage + frame params)  ← MISSING, blocks object animation
Camera (transform core) ── world↔screen, DrawToWorld, MouseWX/WY ← no Object dependency
   ├─► Tile Maps (tileset = Image; renders via camera)
   └─► Objects (sprite = Image; world render needs Camera)
          ├─ collision type 4 / ObjectSight ──► needs Tile Maps
          ├─ ScreenPositionObject / ObjectPick / CameraPick ──► needs Camera
          └─ animation (PlayObject/LoopObject/ObjectFrame) ──► needs multi-frame Images
UpdateGame / DrawGame (game loop) ── orchestrates Objects + Camera + Map
```

Three hard constraints fall out:

1. **Multi-frame Images must precede animated Objects.** `Image` is single-frame today (`LoadAnimImage` + `frame` params deferred); `LoadAnimObject`/`PlayObject`/`ObjectFrame` are unimplementable without it.
2. **Camera transform core must precede Objects** — objects render in world space and convert screen↔world (`ScreenPositionObject`, `ObjectPick`).
3. **Camera ↔ Object is circular.** Camera's transform core has no Object dependency, but its convenience funcs (`PointCamera`, `CameraFollow`, `CloneCameraPosition/Orientation`, `CameraPick`) reference Objects. Break the cycle: build the transform core first, wire the object-aware camera funcs *after* Objects exist (Phase 5).

## Solution

> **Decided design points (this session):**
> - **Handle representation = opaque types.** `Object` and `Map` are new **opaque type tags** — `Object` = tag **13**, `Map` = tag **14** — mirroring `Image` (FD-011, tag 11) and `Font` (FD-018, tag 12). Although `cb_runtime.md` describes them as integer handles (faithful to cbEnchanted ids), exposing them as opaque types gives a distinct sema/IR type, `= Null` null-safety (per the FD-018 null-opaque-return fix), and rejection of arithmetic/ordering/field-access. The opaque handle's bit pattern is a **raw pointer** (`CbObject*`/`CbMap*`), mirroring `Image`/`Font` — the C++ side keeps **no internal integer id** (resolved 2026-06-20; see *Registry — no numeric ids*).
> - **Objects are a graphics subsystem — tests are graphics-gated.** Sprites *are* Allegro bitmaps; it makes no sense to require them to work without graphics. Object/Camera/Map fixtures route through the FD-033 `HAS_GRAPHICS` gate (the existing pattern for graphics/input fixtures). Only pure math helpers (AABB/circle overlap, `GetAngle2`/`Distance2`, collision geometry) stay headless-assertable.

Follow the established **FD-013/FD-017 runtime recipe** per function: prototype in `cb_runtime_func.h`, implement in the owning `.cpp`, register one `CB_FN` row (and any new opaque type) in `catalog.cpp` under the `#ifndef CB_NO_ALLEGRO` guard, and lean on the generic libffi dispatch so per-function Rust work is near-zero beyond mirroring the new type tags in `cb-runtime-sys`/`cb-ir`/`cb-sema`. Use `D:\projects\cbEnchanted` (now a working directory) as the authoritative behavioral reference; `cb_runtime.md` already distills the semantics, defaults, and quirks (e.g. `rotQuality` accepted-but-ignored, `CloneObject` resetting position/angle, 1-based `GetCollision`, `EditMap`'s popped-but-ignored `map` arg, `PixelPick` being a registered no-op stub).

Each phase is **one logical PR**:

### Phase 1 — Multi-frame sprite sheets (extend `Image`) ✅ DONE (commit `0874022`)
Slicing + `frame`-param plumbing on the existing `Image` type (`runtime/cb_gfx.cpp`): `LoadAnimImage`, `MakeImage(w,h,frameCount)`, and the deferred `frame` arguments on `DrawImage`/`DrawGhostImage`/`DrawImageBox`/`ImagesCollide`/`SaveImage`. No new opaque type. Foundation for object animation.

**Landed:** `CbImage` gained `frame_w/frame_h/anim_begin/anim_length`; a slice helper, `cb_rt_load_anim_image` (with MakeImage's headless memory-bitmap fallback), `cb_rt_make_image_frames` (frameCount popped+ignored), and `frame` overloads for `DrawImage`/`DrawGhostImage`/`DrawImageBox` (one `CB_FN` per arity). `HotSpot(-1,-1)` frame-centers; `CloneImage` copies frame metadata. Graphics-gated golden fixture `runtime_image_fd036` (build sheet → `SaveImage` → `LoadAnimImage` → `DrawImage(frame)` → `GetPixel`, run in an isolated temp cwd) + `cb-runtime-sys` overload-arity assertions. `cb_runtime.md` updated.

**Decisions resolved during impl:**
- **Slice bugs #1/#2 → fixed** (not replicated): `row = frame/framesX`, `top = row*frameHeight` (cbEnchanted used `/framesY`, `*frameWidth`). The fixture's multi-row non-square layout pins the fix (verified it fails under the buggy math).
- **`useMask` on `DrawImage`/`DrawImageBox` → accepted but ignored**, with a `// TODO(FD-036)` to revisit — this port's masking is destructive (single bitmap, no unmasked copy to select). `SaveImage`/`ImagesCollide` `frame` args stay inert (matches cbEnchanted).

**Follow-ups noted (not blocking):**
- Pre-existing `DrawImageBox` arg-order discrepancy: this port's signature is `(srcX, srcY, w, h, dstX, dstY)` vs the documented/cbEnchanted `(dstX, dstY, srcX, srcY, w, h)`. The new `frame` overloads were kept consistent with the existing port order rather than reordering an FD-017 function mid-phase.
- `cb_rt_load_image` (single-frame `LoadImage`) still lacks the headless memory-bitmap fallback that `LoadAnimImage` now has.

### Phase 2 — Camera transform core ✅ DONE
New `runtime/cb_camera.{h,cpp}`: `PositionCamera`, `MoveCamera`, `TranslateCamera`, `RotateCamera`, `TurnCamera`, `CameraX`, `CameraY`, `CameraAngle`, the Allegro world transform (world Y inverted **in the screen↔world wrapper functions, not the matrix** — see *Camera & transform* under Reference-verified behavior), `DrawToWorld` (three independent user-draw flags, **orthogonal** to the always-on object/map world pass), and `MouseWX`/`MouseWY`. **No Object dependency.** The object-referencing camera funcs are deferred to Phase 5.

**Landed:** New Allegro-free `cb_camera_math.h` (a 2D affine reproducing Allegro's `al_*_transform` composition exactly, so it is the single source of truth for both screen↔world conversion and the render `ALLEGRO_TRANSFORM`); `cb_camera.{h,cpp}` holding the live state and the 11 `cb_rt_*` entry points; a new `design_w/h` (400×300 default, set by `Screen`) in `cb_gfx.cpp` that the transform centers on; `DrawToWorld` wired into every user draw command (`Line`/`Box`/`Circle`/`Ellipse`/`Dot`/`DrawImage*`/`DrawGhostImage*`/`DrawImageBox*`/`Text`) via a set-world-then-restore-identity helper gated by `!drawing_on_image`. Headless gtest `runtime/tests/test_camera.cpp` (affine round-trips, design-resolution centering, render==world→screen), `cb-runtime-sys` arity asserts, and graphics-gated golden fixture `runtime_camera_fd036`. `cb_runtime.md` updated.

**Decisions resolved during impl:**
- **Dual-angle model kept faithful (user decision):** `camera_angle` (degrees — `CameraAngle()` + `MoveCamera` heading) and `camera_rad_angle` (radians — world matrix) are independent fields; `RotateCamera(logical, render)`/`TurnCamera(dLogical, dRender)` take **two** angle args (cbEnchanted's LIFO "dummy" pop dropped — a real-CoolBasic-bytecode artifact this compiler doesn't need). `cb_runtime.md` corrected from its prior single-arg form.
- **`DrawToWorld` fully wired (user decision):** the Y-flip is folded into a dedicated *render transform* (world matrix with the c/d column negated) so raw world coords map straight to screen — identical to cbEnchanted for line/dot/circle/image/text, differing only in Box/Ellipse *extent* orientation (cbEnchanted flips the anchor only; documented, visual-only divergence).
- **Headless testability:** the transform math is Allegro-free (`cb_camera_math.h`, like `cb_geom.h`) so it unit-tests via the `CB_RUNTIME_TESTS` gtest path with no display; the catalog rows stay behind `#ifndef CB_NO_ALLEGRO` (SDK-free build compiles them out cleanly).

**Follow-ups noted (not blocking):**
- `MouseWX`/`MouseWY` exact values aren't golden-asserted (live mouse position is non-deterministic); their math is pinned by `test_camera.cpp` and the catalog/libffi path is smoke-tested in the fixture.
- The object-aware camera funcs (`PointCamera`, `CameraFollow`, `CloneCameraPosition`/`Orientation`, `CameraPick`, `ScreenPositionObject`) and `CameraFollow`'s use of the physical window size remain deferred to Phase 5 per the dependency analysis.

### Phase 3 — Tile Maps ✅ DONE
New `runtime/cb_map.{h,cpp}` + register the `Map` opaque type (tag 14): `LoadMap`, `MakeMap`, `MapWidth`, `MapHeight`, `GetMap`, `GetMap2`, `EditMap`, `SetMap`, `SetTile`. Layer model (0=background, 1=foreground, 2=collision always-active, 3=data), 1-based tile ids (0=empty), single active map, rendered via the Phase 2 camera.

**Landed:** New Allegro-free `cb_map_data.h` (the `CbMapData` struct, the `.til` binary parser `cb_map_parse`, and the pure grid/centering/slice helpers — headless-unit-tested in `runtime/tests/test_map.cpp`); `cb_map.{h,cpp}` holding the singleton `CbMap` (parsed data + tileset bitmap), the 10 `cb_rt_*` entry points (`SetTile` is a 2-/3-arity overload, slowness default 1), and the `cb_map_render_active` pass; a new `cb_camera_world_transform()` accessor (the plain world transform, no folded Y-flip) so tiles render like cbEnchanted's `useWorldCoords` + per-anchor `convertCoords` Y-flip; the render pass wired into `do_draw_screen` (background layer 0 then foreground layer 1, on top of user draws, beneath the AddText overlay). `Map` = opaque tag **14** in `catalog.cpp` (tag 13 reserved for `Object`). Graphics-gated golden fixture `runtime_map_fd036` (`MakeMap`/`EditMap`/`GetMap2` round-trips + `LoadMap` of the real CoolBasic `testmap.til`+`tileset.bmp`, staged into an isolated cwd) + `cb-runtime-sys` type-tag/arity assertions. `cb_runtime.md` updated.

**Key finding (simplifies the FD's plan):** opaque types are **catalog-driven by name** — `cb-ir`/`cb-sema`/`cb-backend-interp` handle `Map` generically via `IrType::RuntimeType(String)`, so **no Rust frontend changes** were needed beyond the catalog-content test assertions. The "Files to Create/Modify" table over-lists `cb-ir`/`cb-sema` for this phase.

**Decisions resolved during impl:**
- **`.til` extension + format byte-verified (user-provided real asset).** CoolBasic tile maps use `.til`, not `.map`; `D:\CoolBasic\Media\testmap.til` confirmed every documented offset/magic (version 1.3, mask@520, tileCount@820=256, 32px tiles, 20×15, on-disk layer order 0/2/1/3). The doc's `.map` references are renamed to `.til`; the format note is upgraded from "code-derived, not asset-confirmed" → **asset-confirmed**.
- **Anim off-by-one replicated.** The file stores `tileCount` per-tile anim entries (256 = 2048 bytes) but cbEnchanted reads only `tileCount-1` (255); the trailing 8 bytes are ignored. The port replicates the read for byte-compatibility (harmless here — all entries are 0; matters only for animated maps, driven by the Phase 5 update tick).
- **Render now via a DrawScreen hook (user decision).** With no object subsystem yet, the map draws from `do_draw_screen`; Phase 5 relocates the call into `drawObjects`. Pixels can't be golden-asserted (DrawScreen flips+clears — same limit as the Phase 2 camera fixture), so the render math is unit-tested headlessly and the blit is a visual/manual smoke; the fixture asserts deterministic state.
- **Map handle = pointer to the singleton `CbMap`** (mirrors `Image`/`Font`); the funcs never deref a passed handle (`EditMap`'s `map` arg is ignored). **No shared id space is introduced** — Phase 4 adopts the no-id registry design (see *Registry — no numeric ids* below), and because `Map` is its own opaque type the active map is **not** enumerated by `InitObjectList`/`NextObject`.

**Bug-ledger / defensive decisions:**
- **`setTile` realloc bug → fixed.** cbEnchanted writes the slowness default into the *old* freed array, leaving new slots uninitialised; the port grows the vectors and initialises new slots to 1.
- **Null-deref → guarded.** cbEnchanted null-derefs `tileMap` when no map is loaded; every port func returns 0 / no-op instead.
- **Out-of-range layer → guarded.** cbEnchanted indexes `layers[]` with an unchecked `uint8_t` layer (UB for layer ≥ 4); the port bounds-checks and returns 0 / no-op.

**Follow-ups noted (not blocking):**
- The viewport-cull loop is replaced by a full-grid draw (pixel-identical; Allegro clips off-screen tiles) — a deferred optimisation. `usePixelPreciseWorldCoords` rounding is not reproduced (visual-only).
- Odd-dimension sub-tile centring may differ from cbEnchanted by half a tile (visual-only; `GetMap` world-coord centring and the render anchor use the same `*0.5` math so they agree).
- Tile animation advancement, collision use of layer 2, and `ObjectSight` land with the Phase 5 game loop / object subsystem.

### Phase 4 — Objects core ✅ DONE
New `runtime/cb_object.{h,cpp}` + register the `Object` opaque type (tag 13): the object registry and creation/destruction (`LoadObject`/`LoadAnimObject`/`MakeObject`/`MakeObjectFloor`/`CloneObject`/`DeleteObject`/`ClearObjects`), position/movement, rotation/angle (incl. `GetAngle2`/`Distance2`), appearance (mask/ghost/mirror/visibility/order/size), animation (Phase 1), custom data slots (`ObjectInteger`/`Float`/`String`), `ObjectLife`, and enumeration (`InitObjectList`/`NextObject`). Floor objects draw before regular objects.

**`Object` is a raw-pointer opaque handle with no numeric ids** (decided 2026-06-20) — the opaque type *is* the handle ABI, so cbEnchanted's `nextObjectId()` counter, the `int32→CBObject*` map, and the shared Object+Map+particle id space are all dropped. The registry is plain creation-order + per-draw-chain `std::vector<CbObject*>`s. See *Registry — no numeric ids* under Reference-verified behavior.

**Landed:** New Allegro-free `cb_object_data.h` (the pure math — `GetAngle2`/`PointObject` angle formula, `Distance2`, `MoveObject` heading, the corrected frame slice, rotated `ObjectSizeX/Y` bbox, `TurnObject` wrap, animation advance + `ObjectLife` decrement — headless-unit-tested in `runtime/tests/test_object.cpp`); `cb_object.{h,cpp}` holding the `CbObject` struct, the `CbTexture` shared holder, the creation-order + draw-chain registries, all **57 `cb_rt_*` entry points**, and the render orchestrator `cb_objects_render_all` (the `drawObjects` analogue). The Phase 3 standalone `cb_map_render_active` is **retired**: `cb_map` now exposes `cb_map_render_layer(slot)` + `cb_map_active()`, and the orchestrator brackets one world transform over **map background (layer 0) → floor objects → regular objects → map foreground (layer 1)**, wired into `do_draw_screen`. New `cb_gfx_image_bitmap` glue (for `PaintObject(Object/Map, Image)`) and `cb_camera_zoom`/`cb_camera_draw_area` accessors (for floor tiling). `Object` = opaque tag **13** in `catalog.cpp`. Graphics-gated golden fixture `runtime_object_fd036` (state round-trips: position/angle/turn-wrap/slots/life, `GetAngle2`/`Distance2`, `CloneObject` pos+angle reset, a built sprite-sheet object → `PlayObject`/`StopObject`/`LoopObject`/`ObjectFrame`/`ObjectSizeX/Y`, `PaintObject`, and `InitObjectList`/`NextObject` enumeration) + `cb-runtime-sys` type-tag/arity/overload assertions. `cb_runtime.md` §Objects updated.

**Decisions resolved during impl:**
- **Textures shared & reference-counted (user decision).** Each object holds `std::shared_ptr<CbTexture>`; `CloneObject` shares the holder (refcount++, frees the bitmap only when the last owner drops it — safer than cbEnchanted's raw `copied` flag). `PaintObject`/`MaskObject` mutate `tex->bmp` **in place** (all clones see it); `MirrorObject` repoints the one object to a **fresh private** holder (clones unaffected). `CloneObject` resets pos+angle to 0 and **forces `visible=true`** (faithful to cbEnchanted; doc reconciled from "visibility copied").
- **Render interleaved faithfully now (user decision).** Pulled the FD's "Phase 5 relocation" forward: the unified object pass reproduces cbEnchanted's `drawObjects` order under one world-transform bracket, replacing the known-wrong intermediate standalone map pass.
- **`PaintObject` = three handle-typed overloads (user decision):** `(Object, Image)`, `(Object, Object)`, `(Map, Image)` — arity-2, disambiguated by param type; the `(Map, Image)` form lives in `cb_map.cpp` and repaints the active tileset. Replaces the doc's `source: Integer`.
- **Dropped cbEnchanted's phantom pops (Phase 2 `RotateCamera` precedent):** `RotateObject`/`TurnObject` are 2-arg (the two "Random shit" pops are real-CoolBasic-bytecode artifacts this compiler doesn't emit). Documented-but-ignored `z`/`dz`/`rotQuality` on `PositionObject`/`MoveObject`/`TranslateObject`/`LoadObject`/`LoadAnimObject` are exposed as **per-arity overloads** (the catalog has no default-arg mechanism for runtime funcs).
- **No Rust frontend changes** (Phase 3 key finding): opaque types are catalog-driven by name (`IrType::RuntimeType`), so `cb-ir`/`cb-sema`/`cb-backend-interp` handle `Object` generically — only `cb-runtime-sys` catalog-content asserts changed.
- **`alphaBlend` quirk fixed (Bug #7):** kept on the documented 0–1 scale; cbEnchanted's `=255` load write is **not** replicated (render blends only when `< 1.0`, so a loaded object would otherwise never blend). `GhostObject` takes 0–100 and scales/clamps to 0–1.
- **`ClearObjects` decoupled (resolved 2026-06-20):** objects-only — leaves the active map alone (the map is an independent singleton owned by `LoadMap`/`MakeMap`). Intentional divergence from cbEnchanted, which frees the tilemap because there the map is a floor object.

**Follow-ups noted (not blocking):**
- **Floor-object tiling** is display-coupled and untestable headless — the math stays in `cb_object_data.h` where it can, but the blit (and the new `cb_camera_draw_area` accessor it needs) is a visual smoke only.
- **`PaintObject(Object, Image)`** replaces the texture bitmap only; the object's existing frame/anim params persist (sizeX/Y track the new image, faithful to cbEnchanted) — revisit if a real program repaints across sheet geometries.
- **Use-after-`DeleteObject`** is a dangling `CbObject*` (matches `Image`/`Font`); the shared-texture refcount makes the *bitmap* safe but not the handle. Delete-safety stays a cross-cutting opaque-type decision, not Phase 4's.
- **`ScreenPositionObject`** stays Phase 5 (needs object-aware screen→world).

### Phase 5 — Collision, picking, object-aware Camera, game loop
Collision (`SetupCollision` — a **persistent** registration re-evaluated every update tick, with report/stop/slide handling, `ObjectRange`, `ResetObjectCollision`, `ClearCollisions`, `CountCollisions`, `GetCollision`, `CollisionX/Y/Angle`, `ObjectsOverlap`); picking & line of sight (`ObjectPickable`, `ObjectPick`, `PixelPick` stub, `PickedObject/X/Y/Angle`, `ObjectSight` — needs Phase 3 map walls); the object-referencing Camera funcs deferred from Phase 2 (`PointCamera`, `CameraFollow`, `CloneCameraPosition/Orientation`, `CameraPick`, `ScreenPositionObject`); and the game loop `UpdateGame`/`DrawGame` (built-in update/render only — **no user callbacks**; `gameUpdated`/`gameDrawn` dedup flags) plus per-update-tick animation + `ObjectLife` advancement. See *Game loop & registry* and *Collision & picking* under Reference-verified behavior for the exact lifecycle, valid type/handling matrix, and angle/contact formulas.

### Reference-verified behavior (cbEnchanted, mined & fact-checked 2026-06-19)

A 4-agent deep pass over `D:\projects\cbEnchanted\src` (every claim below verified by reading the cited source). This **resolves all three prior open design questions** (game-loop/registry, map singleton, multi-frame Image storage) and pins the behavioral details the per-function recipe needs. The Rust port may choose its own in-memory representations; only the *observable* behavior and the on-disk `.map` format must match.

#### Game loop & registry (Phase 4–5)

- **`UpdateGame`/`DrawGame` run built-in update/render only — there are no user CB callbacks.** The C function-pointer hooks (`addUpdateGameCallback`/`addDrawGameCallback`/`addDrawScreenCallback`, `gfxinterface.h:84-88`) are **defined but never registered anywhere** (zero call sites). Drop any callback machinery from the port; it is out of scope.
- **Frame-boundary dedup** (`gfxinterface.cpp:306-328, 619-634`): `commandUpdateGame` = `updateObjects(); gameUpdated=true`. `commandDrawGame` = update-if-not-already + `drawObjects(); gameDrawn=true`. `commandDrawScreen` = if `!gameUpdated` update, update camera-follow, if `!gameDrawn` draw, then **reset both flags**, pump input, flip. So an explicit `UpdateGame`/`DrawGame` suppresses `DrawScreen`'s implicit pass for that frame.
- **`update_objects` tick order** (`objectinterface.cpp:972-1002`, `cbobject.cpp:246-279`): per object → advance animation **(frame-step: `currentFrame += animSpeed`, `timestep` ignored)** → if `usingLife` `--life` and **auto-delete at `life<=0`** → `eraseCollisions()`. Then update particles, run **all** registered collision checks, then reset `checkCollisions=true` on every object. `TurnObject` is **not** in this loop: `cbobject.cpp:328` `turnObject(speed)` is just `angle += speed` — a one-shot *relative* rotate applied at call time (verified 2026-06-19), **not** a per-update spin. (`cb_runtime.md` §Objects has been corrected to match; the `speed` param name is a cbEnchanted misnomer kept for catalog parity.)
- **`draw_objects` order** (`objectinterface.cpp:862-896`), all under one `useWorldCoords(true)` pass: floor objects (the tilemap is a floor object, so its **background layer 0 draws here**) → regular objects → **tilemap foreground layer 1 drawn last** over everything.
- **Registry & ids (cbEnchanted):** lookup is `std::map<int32, CBObject*> objectMap`; draw order is **not** a list but four head/tail pointers (`firstObject/lastObject`, `firstFloorObject/lastFloorObject`) threaded through each object's intrusive `afterObj/beforeObj`. Ids come from `nextObjectId()` — a `static` counter, **monotonic from 1, never reused, shared across Objects + Maps + particles** (maps go in the same `objectMap` via `addMap`). These integer ids exist **only because legacy CoolBasic typed objects as `Integer`** — they are the CB-visible handle ABI, nothing more.
- **Registry — no numeric ids (port design, decided 2026-06-20):** `Object` is a **raw-pointer opaque handle**, exactly like `Image`/`Font` (`new CbImage{...}` → `CbImage*`, "a bit pattern the runtime owns", `cb_gfx.cpp:14`; freed by a bare `delete`, `cb_gfx.cpp:705`) and the Phase 3 `Map` singleton. The opaque type (FD-018) *replaces* the integer-handle ABI, so `nextObjectId()`, the `int32→CBObject*` map, and the **shared Object+Map+particle id space are all dropped**. Everything the id did is covered without it:
  - **Lookup** — the handle *is* the `CbObject*`; no map indirection.
  - **Draw order** — one `std::vector<CbObject*>` per chain (floor, regular); `ObjectOrder(±1)` = move-to-back/front; creation appends. (Replaces the intrusive `afterObj/beforeObj` head/tail pointers.)
  - **Enumeration order** — one `std::vector<CbObject*>` of live objects in **creation order** (identical to the old monotonic-id order).
  - `DeleteObject` linear-scans the object out of those vectors and `delete`s it (CB object counts are tiny — no perf concern). **Trade-off:** use-after-`DeleteObject` is a dangling pointer, not a safe no-op — but that already matches `Image`/`Font`, so `Object` stays consistent rather than re-introducing an id map just for itself. (Delete-safety, if ever wanted, is a cross-cutting decision for *all* opaque types, not an Object-specific reason to keep ids.)
  - Even Phase 5's "returns an id" funcs (`GetCollision`, `PickedObject`) return an `Object` **handle** (or `Null`), not an integer — so no numeric id is needed anywhere downstream either. (`GetCollision`'s "1-based" is the *index argument*, unrelated to object ids.)
- **Enumeration** (`InitObjectList`/`NextObject`): cbEnchanted uses a **single shared, stateful iterator** over `objectMap` (`NextObject` returns `iter->first` then `++iter`, `0` at end), walking **id order** and **also surfacing map ids**; non-reentrant. **Port:** same single shared stateful index, but over the creation-order `std::vector<CbObject*>` — `NextObject` returns the next `Object` handle and **`Null`** at end (FD-018 null-opaque sentinel, replacing the `0`). **Intentional divergence:** map ids are **not** surfaced — `Map` is a separate opaque type (tag 14), and yielding one through an `Object`-typed return would be a type violation. This is also a correctness improvement: CB code doing `o = NextObject()` can no longer accidentally receive a map.
- **`ObjectOrder`**: `1` → move to tail (front), `-1` → move to head (back); no-op with one object.

#### Objects — lifecycle (Phase 4)

- `CloneObject` (`cbobject.cpp:469-487`) copies image/mask/renderTarget/frames/anim/visibility/range into a **fresh** object → **position and angle reset to 0** (constructor defaults, not copied); shares the texture (won't double-free). Map objects can't be cloned.
- `DeleteObject` removes from pickables + collision checks + draw order, then frees (in the port: scans the object out of the live-objects + draw-chain `std::vector<CbObject*>`s, then `delete`s — no map erase). `ClearObjects` deletes all objects and clears the draw-chain vectors. **Decoupling decision (resolved 2026-06-20): objects-only — leave the active map alone.** cbEnchanted's `ClearObjects` also frees the tilemap *because the map is a floor object in `objectMap`*; with `Map` an independent singleton (`LoadMap`/`MakeMap` already replace it), `ClearObjects` does **not** touch the map. Intentional divergence from cbEnchanted.
- Custom slots (`ObjectInteger/Float/String`) are plain get/set; `ObjectLife` set marks `usingLife=true`. **Catalog arg-count quirks** (LIFO pops): `TurnObject` pops 4 (2 ignored "Random shit" + amount + id), `RotateObject` pops 4 (2 ignored + angle + id), `PositionObject` pops a 4th ignored `z`.
- **`alphaBlend` quirk**: stored 0–255 internally; `load()` sets it to 255, but `render()` only blends when `alphaBlend < 1.0f`, so a freshly loaded object never alpha-blends despite the "0.0–1.0" doc. Decide whether the port "fixes" this before relying on it.

#### Camera & transform (Phase 2 + object-aware funcs in Phase 5)

- **State**: `cameraX, cameraY` (world), `cameraAngle` (**degrees**, what `CameraAngle()` reports), `cameraRadAngle` (**radians**, what enters the matrix), `cameraZoom` (default 1). The two angle fields are set **independently** by `RotateCamera`/`TurnCamera` (each pops a dummy + two separate angle args) and are **intentionally desyncable** — do not collapse them to one field.
- **World transform** (`camerainterface.cpp:193-204`, real `al_*` ops, post-multiply order): `identity → translate(-cameraX, +cameraY) → rotate(cameraRadAngle) → scale(zoom, zoom) → translate(defaultWidth/2, defaultHeight/2)`. Centering uses the **logical design resolution** `getDefaultWidth/Height` (400×300), **not** the window size.
- **Y-inversion is NOT in the matrix.** It lives in the wrappers: `screenCoordToWorld` = `invert-transform` then `y=-y`; `worldCoordToScreen` = `y=-y` then `transform` (`camerainterface.cpp:131-139`); plus `convertCoords` flips Y per world-space draw. Folding `-Y` into the matrix will be wrong.
- **Screen→world** (`MouseWX/WY`, `CameraPick`, `ScreenPositionObject`) all funnel through `screenCoordToWorld` (cached inverse transform). Reproduce the exact construction — these are **headless-testable** as pure affine round-trips.
- **`CameraFollow`** (`camerainterface.cpp:141-173`, run once per `DrawScreen` when following): style 1 (smooth) `cam += (target - cam) / setting` (larger setting = slower); style 2 (deadzone) uses **actual `screenWidth/Height` ± setting** edge tests; style 3 (orbit) `cam = target + (cos, sin)(target.angle°) * setting`.
- **Zoom semantics differ per command** (all clamp to `MIN_CAMERA_ZOOM = 0.00001`): `PositionCamera` sets zoom absolutely *only if `> MIN`*; `MoveCamera`/`TranslateCamera` add `dzoom`. `MoveCamera` direction combines **both** angle fields. `wrapAngle` = while `>360 -=360`, while `<0 +=360`.
- **`DrawToWorld`** sets three independent flags (`drawTextToWorld`/`drawImageToWorld`/`drawDrawCommandToWorld`) gating *user* draw commands — separate from the always-on object/map world pass.

#### Collision & picking (Phase 5)

- **`SetupCollision` is a persistent registration** (`objectinterface.cpp:456-483`), pushed onto a `collisionChecks` vector and re-tested **every update tick** — *not* one-shot. Cleared only by `ClearCollisions` or object delete. Per-object recorded lists are wiped each tick (`eraseCollisions()`). Pops 5 LIFO: `handling, typeB, typeA, objB, objA`.
- **Valid type/handling matrix is sparse and asymmetric** (`collisioncheck.cpp:101-170`):
  - Types: 1=box, 2=circle, 4=map (B only). Handling: 0=report, 1=stop, 2=slide.
  - **`Stop` (1) is circle-only** — box/map with handling 1 are **rejected at setup** (the check is nulled).
  - **Mode 0 "report" computes the corrected position but does NOT apply it** (only records the collision); modes 1/2 apply via `positionObject(safeX, safeY)`.
  - **`Stop` vs `Slide` differ only in the CircleCircle contact-angle source** (Stop = from last safe position → straight push-back; Slide = from new position → tangential slide). Box/map have no Stop/Slide distinction.
  - **`CircleRect` and `RectCircle` object tests are empty no-ops** (`collisioncheck.cpp:163-170`, just `DrawCollisionBoundaries()`) — **box↔circle object pairs never record a collision.** (The static circle-rect helper is used only by CircleMap and `ObjectsOverlap`.)
- **Angle/contact formulas (load-bearing — programs read these):** `CollisionAngle` = `((rad + π) / π) * 180` (note `/π`, **not** `/2π`). **Map normals are hardcoded cardinals**: top=270°, right=180°, bottom=90°, left=0°. Contact points are pushed-back edge/corner points.
- **Map collision (type 4)**: walls = tilemap **layer index 2**, `getHit(x,y)=layers[2][y*w+x]`, **any nonzero = solid**, 0 outside bounds (`cbmap.cpp:434-441`). `CircleMap` is ~285 lines of corner/edge disambiguation — **the single hardest function to port and the most likely interp↔LLVM miscompile site**; budget Phase 5 accordingly.
- **Indexing/counts**: `GetCollision` is **1-based**, returns the collided object's id (0 if none); `CountCollisions` = list size; `ResetObjectCollision` both clears this frame's list **and** skips the object for the current tick. **Default `ObjectRange` is 0×0** — `LoadObject`/`MakeObjectFloor` set it to image size, but **`MakeObject` leaves it 0**, so a made object's collisions are inert until `ObjectRange` is called. `ObjectRange(obj, r1, r2)` with `r2 < 0.001` → `r2 = r1`.
- **Picking**: `ObjectPick` raycasts from picker along facing, keeps nearest pickable by squared distance; `rayCast` → box (slope-intercept) or circle (quadratic, ray length 1e7); `PixelPick` raycast returns false. `PixelPick` command = pure `STUB;` (confirmed no-op). `ObjectsOverlap` type 1=box, 2=circle, 3=pixel (errors → 0).
- **`ObjectSight`** = DDA grid walk (`mapRayCast`) over the tilemap; returns 1 if no wall between. **Requires a tilemap to exist** — define behavior (and avoid the null-deref) when no map is loaded.

#### Multi-frame Images (Phase 1)

- **Storage = one bitmap, sliced on the fly** (`cbimage.cpp:55-92`) — *not* pre-cut sub-bitmaps (the Rust port may pre-cut; only the resulting pixels must match). `LoadAnimImage` loads then `setAnimParams(frameW, frameH, startF, animLen)`; no slicing at load.
- **Slice math**: `if (animLength==0)` draw whole image; else `framesX = w/frameW`, `framesY = h/frameH`, `copyX = frame % framesX`, `copyY = (frame-copyX)/framesY` *(bug)*, `left = copyX*frameW`, `top = copyY*frameW` *(bug)* — see Bug ledger. `frame` is **0-based**, taken `% framesX`, **not** clamped to `animLength`. `animBegin`/`startFrame` is **stored but never read** in any draw path.
- **Mask is whole-sheet** (`al_convert_mask_to_alpha` over the full bitmap) — no per-frame mask. **Hotspot is per-image**, subtracted every frame; `HotSpot(-1,-1)` centers on a **single frame** (`frameW/2, frameH/2`) when frame size is set, else whole-image center.
- **`MakeImage(w,h,frameCount)`'s `frameCount` is popped and ignored** (produces a single-frame image). **`ImagesCollide`'s `startFrame` args are inert** (whole-sheet pixel collision; `// TODO: Check different frames`).

#### Tile Maps & the `.map` format (Phase 3)

- **Singleton confirmed**: one `CBMap *tileMap` (`mapinterface.cpp:13`). `LoadMap`/`MakeMap` delete+replace it and return an id via `addMap` (so it draws), but that id is **never consulted** by the map funcs. `EditMap` pops `map` last and **discards it** (1-based `x,y` → `-1`). `GetMap` = world coords; `GetMap2` = 1-based grid. `SetMap(back, over)` toggles `layerShowing[0/1]`. `SetTile(tile, length, slowness)` has **3 mandatory pops** — the runtime catalog must inject `slowness=1` when omitted.
- **Tile animation is time-based** (`cbmap.cpp:366-378`, unlike object anim): `currentFrame[i] += timestep / (animSlowness[i] * animSpeed)`, wraps at `animLength[i]`; rendering draws `tileId + (int)currentFrame[tileId]` (animated tiles advance through **consecutive tileset ids**).
- **`.map` binary format** — see the dedicated table below; it is a **compatibility-frozen** on-disk surface and must be reproduced byte-exact.

### `.map` binary format (compatibility-frozen — `cbmap.cpp:58-199`)

Little-endian. **Note the two absolute seeks** (skip editor metadata) and the **on-disk layer order 0, 2, 1, 3** with *descending* magic bytes. (cbEnchanted's own inline FIXME comments mislabel the layers; the array indices below are authoritative.)

| Offset | Bytes | Field |
|--------|-------|-------|
| 0 | 4 | magic `{40, 192, 13, 139}` (else fail) |
| 4 | 4 | `float version`, require `1.0 ≤ v ≤ 2.0` |
| **seek 520** | 1,1,1,1 | `maskR`, `maskG`, `maskB`, 1 pad byte |
| **seek 820** | 4 | `int32 tileCount` (sizes the anim arrays; **not** the tileset tile count) |
| 824 | 4,4 | `int32 tileWidth`, `int32 tileHeight` |
| 832 | 4,4 | `int32 mapWidth`, `int32 mapHeight` (tiles) |
| 840 | 4 | magic `{254, 45, 12, 166}` |
| 844 | `w*h*4` | **layer 0** (background), `int32` row-major |
| — | 4 | magic `{253, 44, 11, 165}` |
| — | `w*h*4` | **layer 2** (collision; nonzero = solid) |
| — | 4 | magic `{252, 43, 10, 164}` |
| — | `w*h*4` | **layer 1** (foreground) |
| — | 4 | magic `{251, 42, 9, 163}` |
| — | `w*h*4` | **layer 3** (data) |
| — | 4 | magic `{250, 41, 8, 162}` |
| — | `(tileCount-1)*8` | per tile `i = 1..tileCount-1`: `int32 animLength[i]`, `int32 animSlowness[i]` (index 0 unused = empty) |

Tile cell values are stored as-is; `drawTile` does `tile--` before slicing the tileset, so stored values are effectively **1-based** (0 = empty). **No sample `.map` asset exists in either tree** — byte-verify against one real CoolBasic map before shipping `LoadMap` (offsets/endianness are code-derived, not asset-confirmed).

### cbEnchanted bug ledger (per-bug port decisions)

cbEnchanted is itself a CoolBasic reimplementation and carries genuine bugs. Policy (user decision 2026-06-19): **decide per bug** — replicate-with-documenting-test where real CoolBasic likely depends on it or it's harmless; fix where it's clearly cbEnchanted-only. Resolve each before/while implementing the owning phase.

| # | Bug (location) | Effect | Proposed decision |
|---|----------------|--------|-------------------|
| 1 | Frame-slice row index `copyY=(frame-copyX)/framesY` should be `/framesX` (`cbimage.cpp:64`) | Wrong frame on multi-row sheets | **Cross-check real CB first.** Only correct for single-row/square sheets; real CB games use multi-row sheets, so cbEnchanted is likely the deviant. Lean **fix** (correct slicing) + test, pending the cross-check. |
| 2 | Frame-slice vertical offset `top=copyY*frameWidth` should be `*frameHeight` (`cbimage.cpp:67`) | Wrong row pixels for non-square frames | Same as #1 — lean **fix**. |
| 3 | `PointCamera` uses `obj->getY()` for both `atan2` args (`camerainterface.cpp`) | Camera aims wrong | **Fix** — clearly a cbEnchanted typo; `commandPointObject` (the object analogue) does it correctly. |
| 4 | `CloneCameraOrientation` sets `cameraAngle` but not `cameraRadAngle` | Reported angle ≠ matrix rotation | **Fix** (set both) — the desync is an artifact of the dual-field design, not intended behavior. |
| 5 | `PickedAngle` uses stale loop-end coords, returned in **radians** (others are degrees) (`objectinterface.cpp`) | Wrong pick angle, wrong unit | **Fix** — compute from the picked hit point, return degrees; document the divergence from cbEnchanted. |
| 6 | `CircleRect`/`RectCircle` object collision tests are empty no-ops (`collisioncheck.cpp:163-170`) | Box↔circle object pairs never collide | **Replicate + test** initially (observable behavior real games may assume); revisit if a game needs it. The static helper still serves CircleMap/`ObjectsOverlap`. |
| 7 | `alphaBlend` loaded as 255 but blended only when `< 1.0` | Freshly-loaded objects never alpha-blend | **Replicate + test** — settle the 0–255 vs 0–100 scale (`GhostObject` takes 0–100) when implementing appearance; document. |

### Remaining open questions

- **Runtime threading model — RESOLVED: single-threaded** (verified 2026-06-19). The interpreter's `Value` model is built on `Rc`/`RefCell` (`crates/cb-backend-interp/src/{value,heap,string_handle}.rs`) with no `Arc`/`Mutex`/`thread::spawn`/`rayon` anywhere, so the VM is `!Send` and executes on one thread; runtime catalog/FFI calls happen on that same thread. The single shared `InitObjectList` iterator and the singleton tilemap as plain process-global state are therefore safe. (cbEnchanted's function-local `static` id counter is **dropped** under the no-id registry design — see *Registry — no numeric ids*; only the global vectors remain.) (Revisit only if a future backend parallelizes VM execution.)
- **`.map` asset byte-verification** (see note above) and **`alphaBlend`/`GhostObject` scale reconciliation** (Bug #7).

## Files to Create/Modify

| File | Phase | Action | Purpose |
|------|-------|--------|---------|
| `runtime/cb_gfx.cpp`, `runtime/cb_runtime_func.h` | 1 | MODIFY | Multi-frame `Image` slicing + `frame` params |
| `runtime/cb_camera.{h,cpp}` | 2 | CREATE | Camera transform core + world↔screen + `MouseWX/WY` |
| `runtime/cb_map.{h,cpp}` | 3 | CREATE | Tile-map subsystem; `Map` opaque type |
| `runtime/cb_object.{h,cpp}` | 4–5 | CREATE | Object registry, lifecycle, transform, animation, collision, picking, game loop |
| `runtime/cb_runtime_func.h` | 2–5 | MODIFY | Prototypes for the new `cb_rt_*` functions |
| `runtime/catalog.cpp` | 1–5 | MODIFY | Register `Object`(13)/`Map`(14) opaque types + one `CB_FN` row per command (`#ifndef CB_NO_ALLEGRO`) |
| `runtime/CMakeLists.txt`, `crates/cb-runtime-sys/build.rs` | 2–4 | MODIFY | Add new TUs to the build / rebuild watch |
| `crates/cb-runtime-sys` | 3–4 | MODIFY | Mirror the new `Object`/`Map` type tags in catalog decode |
| `crates/cb-ir`, `crates/cb-sema` | 3–4 | MODIFY | Wire `Object`/`Map` as opaque types (FD-011/FD-018 `RuntimeType`/`OpaqueHandle` machinery) |
| `crates/cb-driver/tests/` | 1–5 | MODIFY | Graphics-gated golden fixtures (+ headless tests for pure collision/angle math) |
| `docs/cb_runtime.md` | 1–5 | MODIFY | Mark implemented funcs / reconcile cbEnchanted discrepancies found during impl |

## Verification

- `cargo build` (full Allegro path) + SDK-free (`CB_RUNTIME_FORCE_SDK_FREE=1`) both succeed each phase.
- `cargo test --workspace` green across feature combos; new golden fixtures graphics-gated via `HAS_GRAPHICS` (FD-033). Pure helpers — AABB/circle overlap, `GetAngle2`/`Distance2`, collision geometry — get headless unit tests.
- **Headless-testable beyond the graphics gate** (no display needed): the camera world↔screen transform as pure affine round-trips (e.g. camera at origin, zoom 1, angle 0 → world `(0,0)` maps to screen `(defaultW/2, defaultH/2)`; `screen→world→screen` identity); the `.map` binary parser against a crafted/real fixture; the `CollisionAngle` `((rad+π)/π)*180` formula and cardinal map normals.
- `cargo clippy --workspace --all-targets -D warnings` and `cargo fmt --all --check` clean.
- Cross-check behavior against `D:\projects\cbEnchanted` for the documented quirks (`CloneObject` position/angle reset, 1-based `GetCollision`, `ObjectLife` decremented once per **update tick** — the implicit `DrawScreen` one *or* an explicit `UpdateGame`/`DrawGame`, `EditMap` ignored-`map`, `Stop` collision circle-only, dead `CircleRect`/`RectCircle` object pairs). Each cbEnchanted bug from the ledger gets a test pinning the chosen replicate-or-fix behavior.
- **Deferred (needs a real display):** visual smoke test of sprite rendering, animation, rotation/mirroring, camera follow/zoom, tile-map drawing, and collision response — can't run headless.

## Related

- `docs/cb_runtime.md` §Objects, §Camera, §Tile Maps, §Images, §Graphics (`UpdateGame`/`DrawGame`) — authoritative behavioral spec
- `D:\projects\cbEnchanted` — authoritative C++ reference implementation
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque-handle (`CB_TYPE_*`) machinery (`Object`/`Map` follow this)
- [FD-018](archive/FD-018_RUNTIME_TEXT_AND_FONT_SUPPORT.md) — `Font` opaque-type + null-opaque-return precedent
- [FD-013](archive/FD-013_EXTENDING_RUNTIME_SUPPORT.md) / [FD-017](archive/FD-017_RUNTIME_MODULE_COMPLETENESS.md) — runtime-port recipe & completeness precedent
- [FD-033](archive/FD-033_CATALOG_MOCK_FOR_SDK_FREE_TESTS.md) — SDK-free build & `HAS_GRAPHICS` gating

**Roadmap — follow-on FDs (game-cluster-first, then these in order):**
1. **Sound** → **Video** (video mixes audio through the sound interface) → **Particles** (needs Images + the game-loop update cadence)
2. **File I/O**, **Memory Blocks**, **DATA / `Read` / `Restore`**, **`Encrypt` / `Decrypt`**, plumbing System funcs (`Crc32`/`SetWindow`/`FrameLimit`/`Errors`), **`CallDLL`**
