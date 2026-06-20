# FD-038: Particle System Runtime Functions

**Status:** Pending Verification
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Brings CoolBasic's "Effects" subsystem online — particle emitters for smoke, sparks, fire, and exhaust trails — the last of the core game-programming subsystems before the independent runtime modules (Sound, File I/O, etc.).

## Problem

CoolBasic ships a built-in particle system ("Efektit") so games don't have to manage thousands of tiny short-lived objects by hand. It is **not yet implemented** in the runtime. The full public API is small — one function and three commands:

| Entry point | Form | Purpose |
|-------------|------|---------|
| `MakeEmitter(image, lifetime)` | **function** → emitter | Create an emitter at (0,0). `lifetime` = how many `DrawScreen` frames each emitted *particle* lives (not the emitter). Returns a handle. |
| `ParticleMovement emitter, speed, gravity, [acceleration]` | command | Particle launch `speed` (px, float), `gravity` (float, ~0.02–0.4, may be negative), optional per-frame `acceleration` (float, default `1.0`; `<1` decelerates, `>1` accelerates). |
| `ParticleEmission emitter, density, count, spread` | command | `density` = frames between emissions (smaller = denser), `count` = particles spawned per emission, `spread` = emission sector in degrees `0–180` (default 180 = all directions; 0 = a tight stream in the emitter's facing direction). |
| `ParticleAnimation emitter, frames` | command | Treat the emitter image (loaded via `LoadAnimImage`) as a `frames`-long animation, played **once** over each particle's lifetime. |

Authoritative behavior notes (from CoolBasic Help + cbEnchanted):

- **An emitter IS an object.** `CBParticleEmitter : public CBObject` in cbEnchanted. It is created, moved, rotated, and destroyed with the ordinary **object commands** (`PositionObject` / `RotateObject` / `MoveObject` / `TurnObject` / `DeleteObject`) and can be given a life. Particles are emitted in the direction the emitter object faces, within the `spread` sector. (The Help docs also claim collision and picking work on emitters, but real-CB testing disproved both — see resolved OQ2.)
- **Particles are internal**, not user-visible `Object`s — the runtime owns the `vector<Particle>`, updates and renders them. The user only ever holds the emitter handle.
- **Per-frame cadence is load-bearing.** Lifetime, emission density, gravity, acceleration, and the once-through animation are all measured in *frames* (i.e. `DrawScreen` / the FD-036 update tick). This subsystem depends on the FD-036 game loop and multi-frame images already being in place.

## Solution

Mirror the FD-036 object-runtime recipe. New C++ runtime module `runtime/cb_particle.{h,cpp}` implementing an emitter that plugs into the existing object render/update pipeline, plus `CB_FN` catalog rows for the four entry points. **No Rust frontend work** — opaque types are catalog-driven by name (`IrType::RuntimeType`), so this is C++ runtime + catalog + `cb-runtime-sys` content asserts, exactly as FD-036's Object/Map landed.

### Resolved (deep analysis, 2026-06-20): reuse `Object`; no distinct `Emitter` catalog type

Runtime-type compatibility in `cb-sema` is **strict by-name with no subtyping** — `find_implicit_conversion` (`crates/cb-sema/src/convert.rs:58-83`) has exactly one opaque-type rule, `Null → RuntimeType`; there is no `RuntimeType → RuntimeType` coercion. A distinct `Emitter` type would therefore **fail overload resolution against every `CbObject*`-typed object command** (~50–70 catalog rows: transforms, paint/show/ghost, play/loop, data slots, life, the collision family, picking, object-aware camera) — emitting E0317 / E_NO_MATCHING_OVERLOAD. Making a distinct type usable would require either a load-bearing `Emitter <: Object` type-system extension or ~50 duplicate command overloads — both rejected.

Decision:

- **`MakeEmitter(Image, Int) → Object`** (catalog return tag 13). The handle lives in the existing object registry, draw chain, and update loop — so the transform/lifecycle commands `PositionObject` / `RotateObject` / `MoveObject` / `TurnObject` / `DeleteObject` / `ObjectLife` work on it **with zero frontend changes**, and `NextObject` (return-typed `Object`) enumerates it for free. **Neither collision nor picking apply to emitters** (resolved OQ2 — real-CB verified): the emitter is excluded from both the pick set and the collision set. This matches cbEnchanted's class shape (the emitter *is* a `CBObject`) but **not** cbEnchanted's behavior, which leaves emitters in both registries (a divergence from real CB — bug-ledger entry).
- **Emitter-specific behavior is keyed on an internal `CbObject` kind field, invisible to the catalog.** Promote the existing `isFloor` flag (`runtime/cb_object.cpp:103/124`) to a small `kind` discriminator (or add `bool is_emitter` + a `unique_ptr<EmitterState>` payload, null on regular objects). This is the same kind-dispatch precedent `render_object`/`render_all` already use for floor objects — cbEnchanted expresses the same thing as `enum Type { Object, Map, ParticleEmitter }` + virtual `type()`.
- **The three particle commands take `Object` and runtime-guard the kind by trapping.** Classic CB blind-`static_cast`s the handle (`effectinterface.cpp:24/31/40`) with no check — UB on a non-emitter. cbcompiler_rs instead checks the internal kind and **traps via the FD-015 channel** (`cb_host()->raise_error`) with a clear message (e.g. `ParticleMovement: object is not a particle emitter`), making it **strictly safer than classic CB**. This mirrors cbEnchanted's own hard error for `MirrorObject` on a non-Object (`objectinterface.cpp:310`). The reuse "hazard" (passing a plain object to `ParticleMovement` type-checks) is therefore caught at runtime, not silently swallowed (resolved OQ3).
- **Emitter-kind dispatch via the internal kind:** `DeleteObject` branches to deferred-free for emitters (move to a rogue list so live particles finish their lifetimes, per `effectinterface.cpp:56-72`); `MirrorObject` rejects emitters; and emitters are **excluded from both `pick_at`/`pickable_objects` and the collision set (`SetupCollision`/`collision_checks`/`ObjectsOverlap`)** (resolved OQ2 — real CB does neither). These exclusion commands silently no-op on an emitter (matching real CB's "does nothing"); pixel-pick on an emitter must be inert, **not** the crash classic CB exhibits. Note this is *stricter* than cbEnchanted, whose collision and raycast-pick paths have no type guard.
- **Catalog cost:** four `CB_FN` rows + four `by_symbol` asserts in `cb-runtime-sys`. **No type-count bump** (`catalog.types.len()` stays 5, `crates/cb-runtime-sys/src/lib.rs:459`) and **no `CB_CATALOG_VERSION` bump** (v6 gates struct layout, not row counts).

### Particle simulation sketch

- **Particle update** runs in the FD-036 per-tick path (folded into `cb::object::update_all`, gated on the emitter kind): advance `particleSpawnCounter` by the emission density; on each emission spawn `count` particles with launch velocity from `speed`, direction = `emitter_angle + uniform_random(-spread, +spread)` degrees (resolved OQ4); each frame apply `gravity`, scale velocity by `acceleration`, advance per-particle life; cull at `lifetime`.
- **Particle render** runs inside the FD-036 world-transform bracket (a kind branch in `render_object`, mirroring the `isFloor` branch), in render order. With `ParticleAnimation`, pick the frame as `min(floor(age / lifetime * frameCount), frameCount - 1)` — **clamped** to the last frame so an over-running index can't read past the strip (resolved OQ5; animation plays once over a particle's life). May need an additive blend set/restored mid-pass (like `mirror_object` does) — a local concern in `render_object`, not structural.

## Open Questions (resolve before implementing — candidate for `/fd-deep`)

1. ~~**Distinct `Emitter` opaque type, or reuse `Object`?**~~ **Resolved (deep analysis, 2026-06-20): reuse `Object`.** See the Solution's "Resolved" subsection — strict by-name runtime typing makes a distinct type fail against ~50–70 object commands; the emitter reuses `Object` with an internal kind field + runtime guard.
2. ~~**Emitter pickability / collision**~~ **Resolved (real-CB test, 2026-06-20): emitters do NOT pick or collide.** The user verified against the actual CoolBasic compiler: pixel pickability crashes, box/circle pickability has no effect, and emitters do not interact with collisions either. So the Help docs sentence "collision and pick commands work with sources" is **wrong for real CB on both counts**. cbcompiler_rs excludes emitter-kind objects from `pick_at`/`pickable_objects` **and** the collision set (`SetupCollision`/`collision_checks`); those commands silently no-op on an emitter, and pixel-pick must be **inert, not a crash**. Record the docs-vs-real-CB discrepancy in `docs/cb_runtime.md`.
3. ~~**Runtime kind-guard behavior**~~ **Resolved (2026-06-20): trap.** When an emitter-only command (`ParticleMovement`/`ParticleEmission`/`ParticleAnimation`) receives a plain (non-emitter) `Object`, the runtime traps via the FD-015 channel with a clear message — not a silent no-op (which would swallow a handle mix-up). Mirrors cbEnchanted's hard error for `MirrorObject` on a wrong type and the debuggability-first reference interpreter.
4. ~~**Random direction within `spread`**~~ **Resolved (2026-06-20): uniform distribution.** Each particle's launch direction is `emitter_angle + uniform_random(-spread, +spread)` degrees (spread 0–180; 180 = full circle / all directions, 90 = half circle, 0 = a tight stream along the facing direction — per the Help docs). Uniform over the sector, no weighting.
5. ~~**Over-running animation frames**~~ **Resolved (2026-06-20): clamp.** The animation frame index is clamped to `frameCount - 1` so it can never read past the strip. Classic CB crashes when `ParticleAnimation frames` exceeds the loaded strip length; cbcompiler_rs clamps instead (no crash) — another safer-than-classic-CB behavior for the ledger.
6. ~~**`StopEmitting`?**~~ **Resolved (real-CB check, 2026-06-20): not a CoolBasic function.** `stopEmitting()` is a cbEnchanted-internal extension method, used only to drive its deferred-free-on-delete (`rogueEmitters`). Real CB exposes **no** stop-emitting command — emitter lifecycle is `DeleteObject` + `ObjectLife` only. We do not add a command; we keep the equivalent internal "stop + let particles finish" step inside the emitter's `DeleteObject` deferred-free path. Public CB particle surface is exactly the **four** documented entry points.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_particle.h` | CREATE | Emitter struct + glue declarations (per FD-037 `cb::object`/`cb::gfx` namespace convention). |
| `runtime/cb_particle.cpp` | CREATE | Emitter create/update/render + the 4 `cb_rt_*` entry points; integrates with `cb::object` render/update pipeline. |
| `runtime/catalog.cpp` | MODIFY | Four `CB_FN` rows (`MakeEmitter`/`ParticleMovement`/`ParticleEmission`/`ParticleAnimation`), all taking/returning the existing `Object` tag. **No new type entry.** |
| `runtime/cb_object.{h,cpp}` | MODIFY | Internal emitter kind on `CbObject` (promote `isFloor` to a `kind`, or add `is_emitter` + payload); emitter branch in `update_all`/`render_object`; deferred-free in `delete_object`; reject emitters in `mirror_object`; **exclude emitters from picking (`pick_at`/`pickable_objects`, pixel-pick inert/no crash) and from collision (`SetupCollision`/`collision_checks`/`ObjectsOverlap`)** — both silently no-op on an emitter. |
| `runtime/CMakeLists.txt` + `build.rs` | MODIFY | Add the new TU to the full build + the rebuild watch list (graphics-gated; SDK-free build excludes it like the rest of the Allegro layer). |
| `crates/cb-runtime-sys/...` | MODIFY | Four `by_symbol` content asserts for the new functions. No type-count or `CB_CATALOG_VERSION` bump. |
| `docs/cb_runtime.md` | MODIFY | Document the particle subsystem surface (the **four** entry points; no `StopEmitting`) + cbEnchanted parity / bug ledger, incl. the **docs-vs-real-CB discrepancy**: Help claims "collision and pick commands work with sources," but real CB does **neither** (pixel-pick crashes, box/circle inert, no collision). cbEnchanted leaves emitters in both registries — also a divergence. |
| `docs/cb_syntax.md` | MODIFY | If any frontend-visible behavior is needed (likely none). |

## Verification

- `cargo test -p cb-runtime-sys` — catalog decodes with the four new functions (type table unchanged at 5) — the SDK-free gate.
- Full Allegro build + `ctest` for the headless-testable particle math under the `CB_RUNTIME_TESTS` gtest path, mirroring FD-036's camera/collision unit tests: **uniform** spawn direction within `[-spread, +spread]`, gravity/acceleration integration, once-through animation frame selection, and the **frame-index clamp at `frameCount - 1`** (over-running index does not read past the strip).
- Headless driver fixture using `MakeEmitter` + the three commands (asset-independent assertions), graphics-gated via `HAS_GRAPHICS`.
- `cargo build`/`test --workspace`, `clippy -D warnings`, `fmt --check` green in both SDK-free and full-Allegro modes.
- **Negative tests (real-CB-verified):** `ObjectPickable`/`SetupCollision`/`ObjectsOverlap` on an emitter no-op (emitter never picked or collided); pixel-pick on an emitter does not crash; an emitter-only command on a plain `Object` traps with a clear message.
- **Deferred (needs a real display):** visual smoke of smoke/spark/fire/exhaust emitters, spread/gravity/acceleration tuning, animated particles, emitter-as-object move/rotate.

## Implementation Notes (2026-06-20)

Implemented as designed: emitter reuses `Object`, internal kind, runtime trap, no new catalog type/version bump. Net change is C++ runtime + 5 catalog rows + asserts — **zero Rust frontend changes**.

- **`runtime/cb_particle.h` (new, header-only, Allegro-free):** `CbParticle` / `CbEmitterState` + the pure simulation — `particle_launch_rad` (uniform ±spread), `integrate_and_cull` (x+=v; v*=accel; vy-=gravity; cull), `spawn_due` (density schedule, non-positive-density guard), `particle_frame` (forward, clamped). Mirrors the cb_*_data.h headless-test pattern.
- **`runtime/cb_object.cpp`:** `CbObject` gained a `std::unique_ptr<CbEmitterState> emitter` (the kind discriminator) + a `rogue_emitters` drain list. New entry points `cb_rt_make_emitter` / `cb_rt_particle_movement` (+`_acc`) / `cb_rt_particle_emission` / `cb_rt_particle_animation`; `require_emitter` traps non-emitters via `cb_host()->raise_error`. `render_particles` (in the world-transform bracket), `update_emitter` + `update_rogue_emitters` (in `update_all`), deferred-free in `delete_object`, rogue cleanup in `clear_objects`. Emitters excluded in `register_collision` / `objects_overlap_impl` / `object_pickable` / `mirror_object`. Spawn RNG draws from the shared `cb_rt_rnd_max` (so `Randomize` applies).
- **`runtime/cb_gfx.{h,cpp}`:** new `cb::gfx::image_frame_info` accessor (frame cell size + strip length) so MakeEmitter can size/clamp animated particles.
- **`runtime/cb_runtime_func.h` + `catalog.cpp`:** 5 prototypes + 5 `CB_FN` rows (MakeEmitter, 2× ParticleMovement overloads, ParticleEmission, ParticleAnimation), all `Object`-typed. No new type row, no `CB_CATALOG_VERSION` bump.
- **Tests:** `runtime/tests/test_particle.cpp` (13 gtest cases, the pure math); `cb-runtime-sys` catalog asserts for the 5 functions; driver fixture `runtime_emitter_fd038` (emitter-as-object + enumeration + collision/pick exclusion through `UpdateGame`); cli.rs `particle_command_on_non_emitter_traps` (the trap, exit 1).

**Verified (2026-06-20, Windows + Allegro SDK):** `cargo test --workspace` all green (0 failed); `cargo clippy --workspace --all-targets -D warnings` clean; `cargo fmt --all --check` clean; `cb-runtime-sys` catalog asserts 20/20 (full Allegro build, emitter signatures decode); native `ctest` **70/70** (57 prior + 13 new `Particle.*`); emitter driver fixture + trap cli test pass.

**Deferred (needs a real display — can't run headless):** visual smoke of smoke/spark/fire/exhaust emitters, spread/gravity/acceleration tuning, animated particles, emitter-as-object move/rotate. One detail worth a real-CB check: the animation *direction* (we play forward per the Help; cbEnchanted played reverse).

## Related

- **Backlog:** "Particle Effects — After the FD-036 game loop — needs multi-frame Images + a per-frame update cadence."
- **Depends on [FD-036](archive/FD-036_RUNTIME_GAME_OBJECTS.md):** Object opaque type (tag 13), multi-frame `LoadAnimImage`, the `UpdateGame`/`DrawGame` per-tick loop with `ObjectLife` advancement, world-transform render bracket.
- **Follows [FD-037](archive/FD-037_RUNTIME_CODE_CLEANUP.md) conventions:** per-subsystem `cb::<name>` namespace, `extern "C"` only on catalog `cb_rt_*` entry points, `k_snake_case` internal constants.
- **Authoritative sources:** CoolBasic Help `commands/{makeemitter,particlemovement,particleemission,particleanimation}.html`; cbEnchanted `src/cbparticleemitter.{h,cpp}`, `src/effectinterface.{h,cpp}`, `src/particle.h`, `src/cbobject.{h,cpp}` (the `enum Type`/`type()` model), `src/objectinterface.cpp` (the `type()`-keyed delete/mirror/pick branches).
- **Type-model decision evidence:** `crates/cb-sema/src/convert.rs:58-83` (no opaque subtyping), `runtime/cb_object.cpp:103/124` (`isFloor` kind precedent), `crates/cb-runtime-sys/src/lib.rs:459` (type-count assert), `runtime/catalog.cpp:192-204` (type table).
- `docs/cb_runtime.md`, `docs/cb_syntax.md`
