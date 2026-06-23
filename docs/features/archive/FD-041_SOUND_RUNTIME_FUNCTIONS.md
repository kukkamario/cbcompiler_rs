# FD-041: Sound Runtime Functions and Types

**Status:** Open
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Brings CoolBasic's audio subsystem online — loading samples and playing/streaming sounds with live volume/pan/pitch/loop control — unlocking sound effects and music in CoolBasic programs. Also the **prerequisite for Video** (FD backlog: video mixes its audio through the sound interface).

## Problem

CoolBasic programs play audio through a small sample-based sound surface that the runtime does not yet implement. `docs/cb_runtime.md` §"Sound" documents 6 commands (`LoadSound`/`PlaySound`/`SoundPlaying`/`SetSound`/`StopSound`/`DeleteSound`) and two opaque types (`Sound`, `SoundChannel`), none of which exist in the runtime catalog — they return "not yet implemented" today (`cb_runtime.md` §"Not yet implemented" lists sound).

Sound is the next item on the runtime backlog and the **gateway to Video** (the following backlog item — video routes its audio through this same interface). It is otherwise an independent subsystem.

The behavior below was mined from the two authoritative sources (per the FD-036/038/039/040 practice): the **cbEnchanted** reference impl (`G:\projects\cbEnchanted\src\soundinterface.{cpp,h}`, `cbsound.{cpp,h}`, `cbchannel.{cpp,h}`) and the **official CoolBasic Help** (`G:\tools\CoolBasic\Help\commands\*.html`). Both sources agree exactly on the command set.

### Authoritative command set — exactly 6 commands, all sample-based

Both sources confirm classic CoolBasic's audio model is **entirely sample-based**: there is **no `Music`/`PlayMusic`, no master-volume, and no 3D/positional API**. Large-file "music" is folded into `PlaySound` itself (streaming, see below). The official `playsound.html` Help also describes a **CD-track** form (an integer 0–99 track number), but cbEnchanted implements no CD path and CD audio is dead on modern targets — **out of scope** (same call as classic CB minus CD). The Help directory contains one extra file, `lstsound.html`, but cbEnchanted has **no handler** for it — it is an IDE/debug listing pseudo-command (like `LstMem`), **out of scope** for the runtime.

## Solution

This is **not** the FD-039/040 playbook. Sound depends on the **Allegro audio addons** (`allegro_audio` + `allegro_acodec`), so — unlike the Allegro-free Memblock and File subsystems — it follows the **FD-018 (Text/Font) / FD-036 (Game Objects) pattern**: an Allegro-dependent TU registered **inside** the `CB_NO_ALLEGRO` guard, audio-gated tests, and a deferred audible smoke test. Generic libffi dispatch still means **zero Rust frontend work** beyond `cb-runtime-sys` catalog-content asserts (modulo the `PlaySound`/`SetSound` overloads, which are just additional `CB_FN` rows).

### `Sound` and `SoundChannel` opaque types

Two distinct opaque types — the command set cannot collapse them (see the naming trap below):

- **`Sound`** — tag **17** (Image 11, Font 12, Object 13, Map 14, Memblock 15, File 16 → **Sound 17**). A loaded `ALLEGRO_SAMPLE`. Produced by `LoadSound`; consumed by `DeleteSound` and the `Sound`-handle form of `PlaySound`. `Null` by default / `Null` on load failure.
- **`SoundChannel`** — tag **18** (internally `struct CbChannel`, `CB_TYPE_CHANNEL`). A playing instance (`ALLEGRO_SAMPLE_INSTANCE` **or** `ALLEGRO_AUDIO_STREAM`). Produced by `PlaySound`; consumed by `SetSound`/`StopSound`/`SoundPlaying`.

> **Naming decision (resolved):** the CB-facing type is **`SoundChannel`**, not the bare `Channel`. The original CB Help calls the handle *"kanavamuuttuja"* (channel variable), so `Channel` would be faithful — but opaque type names are **reserved case-insensitively** (`E0303`), and unlike the other reserved nouns (`Sound`/`Image`/`File`/`Map`), "channel" is **polysemous** in a game/graphics lib (color channel, network channel). `SoundChannel` is self-documenting, groups with `Sound`, and leaves the common identifier `channel` free for user code. The internal C++ struct stays `CbChannel` (mirroring cbEnchanted's `CBChannel`) and the tag stays `CB_TYPE_CHANNEL` — only the registered catalog string is `"SoundChannel"`.

Both registered **inside** the `CB_NO_ALLEGRO` guard (they need the audio addon). Type table grows **7 → 9** in the full build; the SDK-free build is **unchanged** (stays at 3 — File/Memblock are still the only headless opaque types). **No `CB_CATALOG_VERSION` bump** (adding types + functions changes no ABI struct layout; catalog stays v6 — same reasoning as FD-039/040).

> **Naming trap (load-bearing for the type checker):** `StopSound`, `SetSound`, and `SoundPlaying` all say "Sound" but take a **`SoundChannel`** handle. Only `LoadSound`/`DeleteSound` (and the preloaded form of `PlaySound`) take a `Sound`. Wiring these to the wrong type mistypes every program — pin it with sema tests.

Following all prior opaque types (FD-011/036/040), `Sound`/`SoundChannel` are **strict opaque handles**: assignment + `= Null` + identity comparison, but sema rejects arithmetic / ordering / field access. This **diverges from classic CB**, where both handles are plain `int32` ids that programs can store in `Int` variables and do arithmetic on — we drop the numeric-id space exactly as FD-036/040 did for Object/Map/File. *(Resolved — see Open Question #3.)*

### New Allegro-dependent TU

- `runtime/cb_sound.cpp` — the `cb_rt_*` entry points; `struct CbSound` (wraps `ALLEGRO_SAMPLE*`, a plain raw-pointer handle like every other type — `new`/`delete`); the **`SoundChannel` pool** — a generation-tagged slab of `CbChannel` slots (sample-vs-stream union + `playType` tag, mirroring cbEnchanted `cbchannel.h`) ported from the interpreter's `heap.rs` `Slab` (see Open Question #1); the best-effort audio init (`al_install_audio` + `al_init_acodec_addon` + `al_reserve_samples(512)`) folded into an `ensure_audio_init()` with graceful audio-less degradation (Open Question #2); and the per-frame reaper that returns finished slots to the free-list and bumps their generation. Registered **inside** the `CB_NO_ALLEGRO` guard; **not** added to the SDK-free `cc` build.
- `runtime/cb_sound.h` — pure, headless-testable helpers: the **parameter-mapping math** (CB volume/balance 0–100 → Allegro gain/pan 0..1 / −1..1; CB target-frequency-Hz → Allegro speed ratio `freq / native_freq`), extracted so gtest can pin the conversions **without an audio device** (mirrors `cb_camera_math.h` / `cb_map_data.h` / `cb_particle.h`). The generation-pool liveness logic (alloc / reap-bump / stale-reject) is likewise pure and headless-testable. This is the only part testable in CI-like conditions.

### Command surface (6 — exact set from `docs/cb_runtime.md` §"Sound")

| Command | Signature | Returns | Semantics |
|---------|-----------|---------|-----------|
| `LoadSound` | `path: String` | `Sound` | Load a file fully into memory as a sample. `Null` on failure (FD-018 null-opaque→`Null` mapping makes the documented "0 on failure" / `= Null` work for free). |
| `PlaySound` | `source: Sound\|String, volume: Float, balance: Float, frequency: Integer` | `SoundChannel` | Play a sound; return a SoundChannel. **Polymorphic first arg** — see below. Used as both a function (keep the SoundChannel) and a statement (discard it). Trailing args optional (arity overloads — see below). |
| `SetSound` | `channel: SoundChannel, looping: Integer, volume: Float, balance: Float, frequency: Integer` | — | Mutate a *playing channel* live: loop on/off (`ALLEGRO_PLAYMODE_LOOP`/`ONCE`), volume, pan, frequency. Trailing args optional. |
| `StopSound` | `channel: SoundChannel` | — | Stop a playing channel. |
| `SoundPlaying` | `channel: SoundChannel` | `Integer` | 1 if the channel is still playing, else 0 (incl. a finished/reaped channel — see lifecycle). |
| `DeleteSound` | `sound: Sound` | — | Free a loaded sound and drop it from the table. |

Defaults (cbEnchanted `cbchannel.h`; the Help marks `SetSound`'s volume/balance/freq *"valinnainen"* = optional): `volume = 100`, `balance = 0`, `frequency = -1`. *(Resolved — Open Question #4: modeled as **arity overloads**, not catalog default args, because the catalog has no default-arg mechanism.)*

### `PlaySound` polymorphic first arg — two source forms

`PlaySound`'s first arg is `Any` in cbEnchanted: an **Int** → a preloaded **`Sound`** handle played as a one-shot `ALLEGRO_SAMPLE_INSTANCE`; a **String** → a **filename** loaded on the fly as a streaming `ALLEGRO_AUDIO_STREAM` (3 buffers × 8192 samples). This is effectively the "music" path (stream a file from disk).

Our strict by-name sema has no `Any`, but it **already overloads** by both parameter type and arity (`abs` by type, `ParticleMovement` by arity) — so register the two source-type forms as distinct `CB_FN` rows, each returning `SoundChannel`:

- `cb_rt_play_sound(Sound, Float, Float, Int) -> SoundChannel`
- `cb_rt_play_sound_file(String, Float, Float, Int) -> SoundChannel`

Because the catalog has **no default-arg mechanism** (Open Question #4, resolved), the documented optional args (`volume`/`balance`/`frequency`) become **arity overloads**: each source form fans out into 1/2/3/4-arg rows backed by thin C wrappers that supply the defaults, exactly as `drawimage`/`moveobject`/`playobject` already do (`cb-runtime-sys` notes "the catalog has no default-arg mechanism"). Full faithful coverage is up to 4 arities × 2 source types; the exact row set is a row-count trade-off to settle at implementation (see Open Question #4).

The `CbChannel` carries a `playType` (`Sample` vs `Stream`) discriminator + a union of the two Allegro handles; `SetSound`/`StopSound`/`SoundPlaying` branch on it (gain/pan/speed setters differ between `ALLEGRO_SAMPLE_INSTANCE` and `ALLEGRO_AUDIO_STREAM`).

**Function-and-statement form (resolved — Open Question #5):** classic CB exposes `PlaySound` both as a function (returns `SoundChannel`) and a fire-and-forget statement — the official Help even classifies it as *"hybridit"* (hybrid command+function), while `SetSound`/`StopSound` are *"komennot"* (commands, void) and `SoundPlaying` is a *"funktio"* (function). Verified against our sema/lowering: a paren-less command-call statement does **not** require a void return — sema accepts a value-returning call in statement position (`check.rs` `check_stmt` → `ExprStmt`) and lowering discards the unread result register (`lower.rs` → `InstKind::Call`; interpreter stores into the result reg only if read). So the `SoundChannel`-returning overloads cover **both** forms. **A separate void overload would be actively wrong:** overload resolution ignores the return type and keys only on arity + param types (`resolve_overload`), so a void `PlaySound(Sound,Float,Float,Int)` alongside the value-returning one with identical params would be **unselectable / ambiguous**. (cbEnchanted's own `commandPlaySound` is byte-identical to `functionPlaySound` minus the final `pushValue` — exactly what statement-discard gives us for free.)

### SoundChannel lifecycle — generation-tagged pool (resolved; Open Question #1)

cbEnchanted's `updateAudio()` runs **every frame** and **deletes any channel whose `isPlaying()` is false**, erasing it from the `channels` map. Consequence: a finished `SoundChannel` handle **silently becomes invalid**, and `SetSound`/`StopSound`/`SoundPlaying` on it are **silent no-ops** (return 0/false) — `getChannel` returns null for a missing id **without raising an error** (deliberate, `soundinterface.cpp:175`). This is **asymmetric** with `Sound`: a bad `Sound` id *does* raise a "Sound access violation" (`getSound`). We should replicate both: **stale channel = silent no-op; invalid sound = trap.**

This collides with the **raw-pointer opaque-handle pattern** of every prior opaque type (Object/Map/Memblock/File, all plain `new`/`delete` pointers that only null-check — no liveness validation, use-after-free is accepted UB): if we reap-and-`free` a `CbChannel`, a CB program holding the stale handle has a dangling pointer, and `malloc` reuse could alias it to a *new* channel — a silent use-after-free / wrong-channel mutation. Crucially, auto-reaping makes a stale handle the **normal** case (the sound just finished on its own), not a program bug, so we cannot wave it away as "the user's bug" the way `DeleteObject`/`CloseFile` do. **Resolved (Open Question #1): a generation-tagged `SoundChannel` pool** mirroring the interpreter's `heap.rs` `Slab` — the reap path bumps the slot's generation so a stale handle fails the liveness check → silent no-op, with no pointer-reuse hazard. See Open Question #1 for the handle encoding.

### Parameter mapping (pure, headless-testable in `cb_sound.h`)

- **Volume / balance are 0–100 scale**, divided by 100 before Allegro (`cbchannel.cpp:35,41`): gain `= volume/100`, pan `= clamp(balance/100, -1, 1)` (balance −100..100 → pan −1..1; clamp so an out-of-range balance degrades gracefully instead of Allegro rejecting the pan — cbEnchanted `createError`ed on rejection).
- **`frequency` is an absolute target in Hz**, converted to a **speed ratio** `freq / sample_native_freq` (`cbchannel.cpp:48-49`); `freq <= 0` → leave native (ratio 1.0).
- **Resolved (Open Question #6): unify to `gain = volume/100` for both sample and stream paths.** cbEnchanted's stream initial-play multiplies by `streamGain = al_get_audio_stream_gain(stream)` read right after load (`cbchannel.cpp:141,144`) — but Allegro inits a fresh stream's gain to **1.0**, so the product *always* equals `volume/100`; and both `SetSound` paths already use `vol/100` directly (`cbchannel.cpp:73,104`). The multiply is dead cruft inconsistent with the other three gain sites. Unifying changes nothing observable and lets `cb_sound.h` expose a single `gain(volume)=volume/100` helper used by sample+stream and PlaySound+SetSound alike — which is what makes it cleanly headless-unit-testable.

### Init & headless / audio-less behavior

`ensure_audio_init()` does best-effort `al_install_audio()` + `al_init_acodec_addon()` + `al_reserve_samples(512)` (the 512 cap is cbEnchanted's; failure is *fatal* there — `initializeSounds` returns false and the engine aborts). **Resolved (Open Question #2): degrade gracefully, never abort** — exactly the FD-018 / `cb_rt_screen` headless pattern (`if (!g_display) return;`, then every op checks a global and no-ops). On any init failure we set a static `g_audio_ok = false` and return; thereafter:

| Audio-unavailable behavior | |
|---|---|
| `LoadSound` | returns `Null` (it's a load failure) |
| `PlaySound` | returns `Null` (a non-playing channel) |
| `SoundPlaying` | `0` |
| `SetSound` / `StopSound` | no-op (the stale/invalid-channel path is *always* a silent no-op, regardless of audio state) |
| `DeleteSound` / `PlaySound(Sound,…)` on the forced `Null` | **no-op, trap suppressed** — so a `Null`-ignoring program runs silently instead of exit-1-ing on a silent CI box |

The audio-available path keeps the normal trap-on-`Null`-`Sound` rule (below). The audio-gated driver tests already skip on a silent box, and gtest's `cb_host()` is null so traps no-op there too — the suppression is the backstop for real user programs on audio-less hosts.

### Error handling

- **Null `Sound` handle** (load-failure `Null`, or never-assigned) on `DeleteSound`/`PlaySound(Sound,…)` → **trap** via the FD-015 channel (`cb_host()->raise_error(...)` → exit 1), matching cbEnchanted's "Sound access violation" and the Memblock/File null-trap convention. *Use-after-`DeleteSound`* (a non-null but freed `Sound`) is **UB — the user's bug**, exactly as `CloseFile`/`DeleteMemblock` document for their raw-pointer handles. We deliberately do **not** give `Sound` a generation pool: it isn't auto-reaped, so a stale `Sound` is a genuine program bug, and matching the established raw-pointer convention keeps `Sound` consistent with every other deletable handle. Trap suppressed while audio is unavailable (see above).
- **Stale/finished `SoundChannel`** on `SetSound`/`StopSound`/`SoundPlaying` → **silent no-op** (return 0 for `SoundPlaying`), matching cbEnchanted's deliberate non-erroring `getChannel`. This is *safe* (not UB): the generation-tagged pool (#1) detects the stale handle via the generation check instead of dereferencing a dangling pointer. This is the one place we are *more permissive* than our usual trap-on-bad-handle rule — intentional, because finished channels legitimately go away on their own.

### Forward-looking: Video (next backlog item)

Video "mixes audio through the sound interface." **Decision (Open Question #7, resolved):** do *not* build a shared mixer now — video isn't needed for a while. Follow cbEnchanted's shortcut and attach channels directly to `al_get_default_mixer()` (cbEnchanted's own `cbMixer` field is unused). Keep the `ALLEGRO_AUDIO_STREAM` stream path factored enough that the Video FD can introduce a real shared mixer later without reworking Sound.

## Authoritative reference semantics (confirmed)

From cbEnchanted (`soundinterface`/`cbsound`/`cbchannel`) + the official Help command list; both agree:

| Aspect | Confirmed behavior |
|--------|--------------------|
| Command set | Exactly 6 (`LoadSound`/`PlaySound`/`SetSound`/`StopSound`/`SoundPlaying`/`DeleteSound`); `LstSound` is IDE-only, no runtime handler |
| Audio library | **Allegro 5 audio addon** (`allegro_audio` + `allegro_acodec`) — matches our target |
| Handles (cbEnchanted) | `Sound` and the channel handle (our `SoundChannel`) are separate `int32` ids in two maps, monotonic from 1, **never reused** |
| `PlaySound` source | Int → preloaded sample (one-shot instance); String → filename → streamed (3×8192) |
| `PlaySound` classification | Help: *"hybridit"* — both a command (statement) and a function (returns the channel) |
| volume / balance | 0–100 floats → /100 → Allegro gain / pan (−1..1); Help marks them optional |
| frequency | absolute Hz → speed ratio `freq / native`; `<= 0` = native |
| loop | channel-only, via `SetSound` (`PlaySound` always starts once) |
| SoundChannel reaping | `updateAudio()` every frame frees channels that stopped playing |
| Bad-id asymmetry | bad channel id = silent (no error); bad `Sound` id = raises an error |
| Init | `al_install_audio` + `al_init_acodec_addon` + `al_reserve_samples(512)` |
| No-API surface | no Music / master-volume / 3D-positional commands; CD-track form exists in Help but is unimplemented/out of scope |

**Help bodies now read** (the Finnish HTML was extractable after all): they confirm the *"hybridit"*/*"komennot"*/*"funktiot"* classification, the optional `SetSound` params, and the CD-track form. All signatures/semantics still cross-checked against the fully-readable cbEnchanted C++.

## Deliberate divergences from cbEnchanted / classic CB

1. **Strict opaque `Sound`/`SoundChannel` types** (tags 17/18, `Null` default) instead of plain `int32` ids — consistent with Object/Map/Memblock/File; gives null-safety + type-distinct rejection. Drops classic CB's int-handle arithmetic. *(See Open Question #3.)*
2. **Generation-tagged `SoundChannel` pool** (Open Question #1, resolved) — a finished channel must not alias a new one; cbEnchanted gets this from never-reused ids, our raw-pointer pattern does not, so `SoundChannel` (uniquely) uses a generation-tagged slab mirroring `heap.rs` instead of a raw pointer. `Sound` stays a raw pointer like every other type.
3. **Graceful audio-less degradation** vs cbEnchanted's fatal init failure (so headless/CI doesn't abort — Open Question #2, resolved).
4. **Unify the sample/stream gain handling** (Open Question #6, resolved) — drop cbEnchanted's stream-path `* streamGain` multiply (always ×1.0 in practice; a latent inconsistency). Behaviorally identical to cbEnchanted, internally uniform.
5. **No CD-track form** — the Help's integer-track CD path is dropped (cbEnchanted never implemented it; CD audio is dead on modern targets).

## Open Design Questions

All design questions are now **RESOLVED**; the decisions and their rationale are kept inline below.

1. **SoundChannel handle representation & reaping (load-bearing). — RESOLVED: generation-tagged pool (option a).** A finished/reaped channel must be a *safe* silent no-op, not a use-after-free — and because the reaper makes stale handles routine, the existing raw-pointer "trust the pointer" pattern (every C++ type today only null-checks; use-after-free is UB) is unacceptable here.
   - **Decision: a generation-tagged `CbChannel` pool in `cb_sound.cpp`**, mirroring the interpreter's `heap.rs` `Slab` (the one tested generation-slab already in the repo): parallel `entries` / `generations` / `free_list` vectors. `alloc` pops a free slot (or grows); the per-frame reaper sets the slot empty, **bumps its generation**, and pushes it to the free-list; lookups verify `generations[index] == handle.generation` → a stale/reaped handle fails → silent no-op. Bounded memory (slots reused, no per-play heap alloc), no pointer-reuse hazard.
   - **Handle encoding:** pack `{u32 index, u32 generation}` into the pointer-sized opaque slot — the FFI carries handles as a `u64` (`Value::OpaqueHandle(u64)`) and `cb_runtime_core.h:55-57` explicitly permits a non-pointer bit pattern. Reserve all-zero for `Null`: encode the low half as `index + 1` (so a live handle is never `0`); `Null`/`0` decodes to a non-existent slot → no-op. Generations may start at 0 like `heap.rs` (the `+1` on the *index*, not the generation, is what keeps the encoding non-zero).
   - **Why not (b) monotonic id-map** (faithful to cbEnchanted's `map<int32, CBChannel*>`): also safe, but it introduces a per-play map+heap allocation and an ever-growing id counter, and there's no in-repo precedent to mirror — whereas the generation slab is already written and tested in `heap.rs` (`freed_handle_is_rejected`, `slot_reused_with_bumped_generation`). **Why not (c) never-free records:** leaks one record per played sound (unbounded).
2. **Init-failure / audio-less behavior. — RESOLVED: degrade gracefully, never abort.** Best-effort `ensure_audio_init()`; on failure set `g_audio_ok = false` and return (the FD-018 / `cb_rt_screen` `if (!g_display) return;` pattern), *not* cbEnchanted's fatal `initializeSounds` → abort. In audio-unavailable mode: `LoadSound`→`Null`, `PlaySound`→`Null` (non-playing channel), `SoundPlaying`→0, `SetSound`/`StopSound`→no-op, and the `Sound`-handle trap is suppressed (a `Null`-ignoring program runs silently instead of exit-1-ing on a silent CI box). See the "Init & headless" section for the full table.
3. **Drop the int-handle model? — RESOLVED.** Yes — `Sound`/`SoundChannel` are strict opaque types, consistent with every prior runtime handle (Object/Map/Memblock/File) and implied by the `SoundChannel` naming decision above. We accept dropping classic CB's int-handle arithmetic; no real CoolBasic program is known to depend on storing/arithmetic-ing audio handles as `Int` (they were just ids passed back to the sound commands). Pin the strictness (reject arithmetic/ordering/field access) with sema tests.
4. **Default args. — RESOLVED.** The catalog has **no default-arg mechanism**: `CbFuncDesc`/`CbParamDesc` encode no default, sema requires exact arity (`check.rs` `resolve_overload` filters by `params.len() == arg_types.len()`), and lowering passes args 1:1. (User-defined-function defaults are parsed and sema-accepted via a min/max arity range, but *not* filled by lowering — a separate latent gap, not this FD's concern.) The established pattern is **arity overloads**: one `CB_FN` row per arity, each backed by a thin C wrapper supplying the documented defaults (precedent: `drawimage` 3/4/5-arg, `moveobject` 2/3/4-arg, `playobject` 1/3/4/5-arg). **Decision:** honor the documented optional args (the Help marks `SetSound`'s volume/balance/freq *"valinnainen"*, and `PlaySound snd` with no volume is idiomatic) via arity overloads. Row-count trade-off to settle at implementation: full coverage is `PlaySound` 1/2/3/4-arg × {Sound, String} (up to 8 rows) + `SetSound` 2/3/4/5-arg (4 rows); a leaner subset (e.g. only the 1-arg and full-arg forms) is acceptable if the intermediate arities prove unused — but should be a conscious choice, not an oversight.
5. **Statement vs function `PlaySound`. — RESOLVED.** A single `SoundChannel`-returning overload pair (× the arity overloads from #4) covers the fire-and-forget statement form. Confirmed against the code: paren-less command-call statements do **not** require a void return — sema accepts a value-returning call as an `ExprStmt` and lowering/interpreter discard the unread result (`check.rs` `check_stmt`, `lower.rs` `InstKind::Call`, `interp.rs` call dispatch). The annotation's worry — that we might need a separate void overload — is the opposite of correct: overload resolution keys only on arity + param types and **ignores the return type**, so a void `PlaySound(...)` with the same params as the value-returning one would be **ambiguous and unselectable**. No void overload; do not add one.
6. **Sample/stream gain inconsistency. — RESOLVED: unify to `volume/100`.** cbEnchanted's stream-path `* streamGain` (`cbchannel.cpp:144`) reads a freshly-loaded stream's gain, which Allegro always inits to 1.0, so it's a behavioral no-op and inconsistent with the sample path and both `SetSound` paths (all `vol/100`). Drop it; `cb_sound.h` exposes one uniform `gain(volume)=volume/100`. Zero observable change vs cbEnchanted.
7. **Mixer. — RESOLVED.** Per the annotation ("we don't need video support for a while"): follow cbEnchanted's `al_get_default_mixer()` shortcut now — attach channels directly to the default mixer, do **not** build a shared mixer abstraction for Sound. The shared-mixer plumbing is deferred to the Video FD when it lands; keep the `ALLEGRO_AUDIO_STREAM` path factored enough that Video can introduce a real mixer then without reworking Sound.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_sound.cpp` | CREATE | The `cb_rt_*` entry points (6 commands + `PlaySound`/`SetSound` arity & source-type overloads); `struct CbSound` (raw-pointer handle); the **generation-tagged `CbChannel` pool** (slab + free-list, Open Question #1) + per-frame reaper; best-effort `ensure_audio_init()` with graceful degradation (Open Question #2); default-mixer attach |
| `runtime/cb_sound.h` | CREATE | Pure, headless-testable param-mapping math — unified `gain(volume)=volume/100`, `pan(balance)=clamp(balance/100,-1,1)`, `speed(freq,native)` — mirrors `cb_camera_math.h` |
| `runtime/cb_runtime_func.h` | MODIFY | Forward-declare `CbSound`/`CbChannel`; declare the `cb_rt_*` sound prototypes |
| `runtime/catalog.cpp` | MODIFY | Register `Sound` (tag 17) + `SoundChannel` (tag 18, string `"SoundChannel"` → `CB_TYPE_CHANNEL`) + the `CB_FN` rows, **inside** the `CB_NO_ALLEGRO` guard |
| `runtime/CMakeLists.txt` | MODIFY | Add `cb_sound.cpp` to the **full** (Allegro) `cb_runtime` lib + the `CB_RUNTIME_TESTS` gtest target; link `allegro_audio` + `allegro_acodec`; ensure the per-frame reaper is called from the existing frame hook (`DrawScreen`/`UpdateGame`) |
| `runtime/tests/test_sound.cpp` | CREATE | Native gtest for the **pure** `cb_sound.h` math (unified gain/pan/speed, edge cases: freq≤0, balance clamp) **and** the generation-pool liveness logic (stale-handle rejection, slot reuse) — the audio-device-free parts |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Catalog-content asserts: `Sound` tag 17 / `SoundChannel` tag 18, type counts (full = 9, SDK-free unchanged = 3), the sound signatures decode |
| `crates/cb-driver/tests/programs.rs` | MODIFY | An **audio-gated** driver fixture (load → play → `SoundPlaying` → stop), gated like the FD-033 `HAS_GRAPHICS` graphics fixtures (skips when audio/Allegro absent) |
| `crates/cb-driver/tests/fixtures/programs/` | CREATE | `.cb` source + `.out` golden for the audio fixture (assertions must be device-independent) |
| `crates/cb-driver/tests/cli.rs` | MODIFY | Trap fixture — an op on a null/deleted `Sound` handle, exit 1 with stderr (audio-gated) |
| `docs/cb_runtime.md` | MODIFY | Mark §"Sound" implemented; rename the documented type `Channel` → `SoundChannel`; record divergences (opaque types replace int ids, stale-channel silent no-op, graceful audio-less degradation, no CD form); note `PlaySound` overloads + stream path |
| `docs/cb_syntax.md` | MODIFY (maybe) | Only if the opaque-handle/overload story needs a §-level note beyond what FD-011/036 already cover |
| `docs/features/FEATURE_INDEX.md` | MODIFY | Promote Sound Backlog → Active now; move Active → Completed on close |

## Verification

- `cargo build --workspace` + `cargo test --workspace` green in **both** modes. SDK-free: the sound TU is Allegro-gated, so the audio fixtures **graphics/audio-skip** (must skip cleanly, like the FD-036 fixtures), and `cb-runtime-sys` asserts the SDK-free type count is **unchanged (3)**. Full-Allegro (`CB_RUNTIME_REQUIRE_ALLEGRO=1`): the catalog links the audio TU, asserts **9 types**, and the sound signatures decode.
- `cargo test -p cb-runtime-sys` — `Sound` tag 17, `SoundChannel` tag 18, type counts, signature decode (incl. both `PlaySound` source overloads and their arity variants).
- Native `ctest` — new `Sound.*` gtest cases pinning (1) the **pure** param mapping in `cb_sound.h`: unified `gain = volume/100`, `pan = clamp(balance/100,-1,1)` incl. out-of-range balance, `frequency ≤ 0` → native speed; and (2) the **generation pool** liveness logic (alloc → reap bumps generation → stale handle rejected → slot reused), mirroring `heap.rs`'s slab tests — both run **without an audio device**. (The actual play/stop/reap of real Allegro instances needs a device → audible smoke deferred.)
- `cargo clippy --workspace --all-targets -D warnings` + `cargo fmt --all --check` clean.
- Sema tests pinning the **naming trap**: `StopSound`/`SetSound`/`SoundPlaying` accept a `SoundChannel` and reject a `Sound`; `DeleteSound` accepts a `Sound` and rejects a `SoundChannel`; `PlaySound(LoadSound("x"),…)` and `PlaySound("x.ogg",…)` both type as `SoundChannel`. Also pin that `PlaySound snd` (statement form, value discarded) type-checks and that the opaque types reject arithmetic/ordering.
- **Deferred (needs a real audio device — can't run headless):** the audible smoke (load a sample, play it, hear it; loop via `SetSound`; pan/volume/pitch sweep; stream a long file via the String form of `PlaySound`).

## Related

- [`docs/cb_runtime.md`](../cb_runtime.md) §"Sound" — the documented 6-command surface + `Sound`/`SoundChannel` types; §"Video Playback" — the dependent subsystem.
- **FD-018** (Runtime Text & Font Support) — closest precedent: an Allegro-dependent TU inside the `CB_NO_ALLEGRO` guard, a new opaque type, headless graceful-degradation (default-font fallback ↔ audio-less fallback), and the null-opaque→`Value::Null` mapping that makes "Null on load failure" work with zero frontend change.
- **FD-036** (Game-Object Runtime Cluster) — precedent for dropping the numeric-id space in favor of opaque handles, graphics-gated tests (FD-033 `HAS_GRAPHICS`), a frame-hook update tick (the channel reaper attaches there), and **arity overloads for optional args** (`drawimage`/`moveobject`/`playobject`).
- **FD-039 / FD-040** (Memblock / File I/O) — the *contrast* case: those are Allegro-free and headless-testable; Sound is **not** (needs the audio addon), so it follows FD-018, not FD-039/040.
- **FD-011** (Runtime Custom Types) — the opaque-type machinery `Sound`/`SoundChannel` register through.
- **FD-015** (Runtime Trap Channel) — the channel by which an invalid `Sound` handle raises a clean runtime error.
- **FD-009** (Runtime Library) — the overload-resolution machinery that handles the `PlaySound`/`SetSound` rows (keys on arity + param types, **not** return type).
- `[[cbenchanted-runtime-reference]]` — authoritative semantics mined from `G:\projects\cbEnchanted\src\soundinterface.{cpp,h}` / `cbsound.{cpp,h}` / `cbchannel.{cpp,h}`; opcodes 450–455 at `cbenchanted.cpp:723-726,942-944`. Help command list cross-checked against `G:\tools\CoolBasic\Help\commands\{loadsound,playsound,setsound,stopsound,soundplaying,deletesound,lstsound}.html`.
- `[[coolbasic-help-manual]]` — the official documented command behavior (Finnish); confirmed the *"hybridit"* classification of `PlaySound`, the optional `SetSound` params, and the CD-track form.
- **Unblocks Video Playback** (next backlog item) — video mixes audio through this interface.
