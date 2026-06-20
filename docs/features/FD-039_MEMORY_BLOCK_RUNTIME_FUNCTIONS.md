# FD-039: Memory Block Runtime Functions

**Status:** Pending Verification
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Brings CoolBasic's **memory block** subsystem online — raw byte-level buffers (`MakeMEMBlock`/`Peek*`/`Poke*`/`MemCopy`/`ResizeMEMBlock`). Self-contained, Allegro-free, and a prerequisite for the memblock forms of `Crc32`, `Encrypt`/`Decrypt`, and `CallDLL`.

## Problem

CoolBasic exposes raw, manually-managed memory blocks for byte-level data manipulation (binary file formats, custom serialization, DLL interop buffers). The full surface is already documented in [`docs/cb_runtime.md`](../cb_runtime.md) §"Memory Blocks" but **none of it is implemented** — it sits in the Backlog as the next self-contained runtime subsystem after the FD-036 game-object cluster.

It is also a dependency for already-deferred work: the **memblock form of `Crc32`** (FD-017 left it blocked — "memblock form is blocked on Memblocks"), and the memblock arguments of `Encrypt`/`Decrypt` and `CallDLL`.

The documented surface (14 functions):

| Function | Parameters | Returns | Description |
|----------|-----------|---------|-------------|
| `MakeMEMBlock` | `size: Integer` | `Memblock` | Allocates a zero-filled block of `size` bytes |
| `DeleteMEMBlock` | `mem: Memblock` | — | Frees a block |
| `ResizeMEMBlock` | `mem: Memblock, size: Integer` | — | Resizes, preserving existing bytes and zero-filling growth |
| `MEMBlockSize` | `mem: Memblock` | `Integer` | Size in bytes |
| `MemCopy` | `srcMem, srcOff, dstMem, dstOff, len` (all Integer except memblocks) | — | Copies `len` bytes between blocks |
| `PeekByte` / `PeekShort` / `PeekInt` | `mem: Memblock, offset: Integer` | `Integer` | Reads 8/16/32-bit (byte+short unsigned, int signed) |
| `PeekFloat` | `mem: Memblock, offset: Integer` | `Float` | Reads 32-bit float |
| `PokeByte` / `PokeShort` / `PokeInt` | `mem, offset, value: Integer` | — | Writes 8/16/32-bit |
| `PokeFloat` | `mem, offset, value: Float` | — | Writes 32-bit float |

## Solution

Follow the established runtime-FD recipe (FD-013/FD-017/FD-018/FD-036): a new opaque type registered via the FD-011 machinery + one `CB_FN` catalog row per function; the generic libffi dispatch means **little-to-no per-function Rust work**.

Key properties that make this a clean batch:

- **Functionality subsystem, not a core/ABI type.** `cb_runtime_core` is deliberately the *irreducible plugin SDK*: only the `CbString` type (the catalog ABI requires it to pass String params) + the FD-015 host handshake (`cb_string.cpp` + `cb_host.cpp`). Memblock is **not** ABI-required — a plugin receiving a `Memblock` just gets a raw opaque pointer, and the Peek/Poke ops are ordinary catalog functions, not primitives a plugin links. So Memblock lives in the **`cb_runtime` functionality lib** as a new TU `runtime/cb_memblock.{h,cpp}`, mirroring `cb_gfx.cpp`/`cb_object.cpp` (and the `Image` opaque type, which also has a destructor yet lives in functionality, not core).
- **Still SDK-free testable — via the build list, not via core.** The subsystem is Allegro-free, so add `cb_memblock.cpp` to `build.rs::SDK_FREE_TUS` (the FD-033 path that compiles the Allegro-free TUs with `cc` for headless `cargo test`). SDK-free testability comes from being Allegro-free + listed there, **not** from living in `cb_runtime_core`. Result: the whole subsystem runs in CI with no display.
- **New opaque type `Memblock`.** Register via the FD-011 path (mirrors `Image`/`Font`/`Object`/`Map`). Next type tag = **15**; type table grows 5 → 6. Confirm whether this needs a `CB_CATALOG_VERSION` bump (FD-036/FD-038 added types/functions with **no** version bump — only an ABI/struct-layout change like FD-029's constants table forces one). *Expected: no bump.*
- **Handle model — raw-pointer opaque handle (FD-036 style).** The opaque type *is* the handle ABI; **no numeric id space**, no `int32→block` map. `MakeMEMBlock` returns the buffer pointer as the opaque handle, `= Null` works for free, and `DeleteMEMBlock` frees it. (The `CallDLL`/`Crc32` interop that classic CB threaded through integer ids is out of scope here — see Resolved Decisions #5 — so nothing forces a numeric id.)
- **`Float` is 32-bit on the wire.** `PeekFloat`/`PokeFloat` read/write **32-bit IEEE** floats even though CB `Float` is `f64` (FD-035). The runtime must `f32`-round-trip across the boundary.
- **Endianness — little-endian, pinned explicitly.** Multi-byte Peek/Poke (`Short`/`Int`/`Float`) read/write **little-endian** byte order regardless of host architecture (matches x86 classic CB), so behavior is platform-independent. Pin it in code, not by relying on host layout.
- **`PeekByte`/`PeekShort` return unsigned.** 8-bit → `0..255`, 16-bit → `0..65535`, both as a non-negative `Integer` (i32). `PeekInt` is 32-bit signed.
- **Bounds safety — trap, a deliberate divergence from classic CB.** Classic CB `Peek`/`Poke` blind-cast and can corrupt memory on an out-of-range offset (UB). Instead, **trap via the FD-015 channel** on an out-of-bounds offset (the read/write would cross the block end), on a null/invalid handle, and on a bad `MemCopy` range. Same for `ResizeMEMBlock` with a negative size. Document as an intentional safety divergence.

### Reference

Authoritative behavior: `D:\projects\cbEnchanted\src\meminterface.{h,cpp}` (mine it for exact resize/zero-fill, endianness, and offset semantics). Per [[real-cb-compiler-is-authoritative]], confirm any behavior conflict against the real CoolBasic compiler at `D:\CoolBasic`.

## Resolved Design Decisions (user, 2026-06-20)

1. **Handle model:** raw-pointer opaque handle (FD-036 style) — no numeric id space.
2. **Endianness:** little-endian on the wire, pinned explicitly (platform-independent).
3. **Out-of-bounds / invalid handle / negative resize / bad `MemCopy` range:** **trap** via the FD-015 channel (not silent clamp, not classic UB).
4. **`PeekByte`/`PeekShort` return unsigned** (`0..255` / `0..65535`); `PeekInt` 32-bit signed.
5. **Scope: the 14 core functions only.** The `Crc32(memblock)` / `Encrypt`/`Decrypt`(memblock) / `CallDLL` follow-ons stay in their own FDs.

Still to confirm during implementation (not load-bearing): whether the new type warrants a `CB_CATALOG_VERSION` bump (expected: no — adding a type/functions doesn't change struct layout); exact `ResizeMEMBlock`-to-0 and overlapping-`MemCopy` semantics against the cbEnchanted/real-CB reference.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_memblock.cpp` | CREATE | Memblock buffer type + the 14 `cb_rt_*` entry points (Allegro-free; in the `cb_runtime` functionality lib, **not** core). **No separate `.h`** — prototypes go in `cb_runtime_func.h`, matching `cb_system.cpp`/`cb_math.cpp` (no other TU needs the internals; the deferred `Crc32`/`Encrypt`/`CallDLL` follow-ons will add a cross-TU accessor header if/when they need one) |
| `runtime/catalog.cpp` | MODIFY | `CB_FN` rows for all 14 functions + the `Memblock` type entry (tag 15) |
| `runtime/cb_runtime_func.h` | MODIFY | Prototypes for the new `cb_rt_*` functions (functionality header, alongside gfx/object/etc.) |
| `runtime/CMakeLists.txt` | MODIFY | Add `cb_memblock.cpp` to the `cb_runtime` target's source list (next to `cb_object.cpp`) |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Add `cb_memblock.cpp` to `SDK_FREE_TUS` + `RERUN_SOURCES` so the headless/CI path builds and tests it |
| `crates/cb-runtime-sys/...` | MODIFY | Catalog-content assertions (new type tag 15, function signatures decode) |
| `crates/cb-driver/tests/...` | CREATE | Headless golden fixture `runtime_memblock_fd039` (SDK-free) |
| `runtime/test_*.cpp` (gtest) | CREATE | Native unit tests for alloc/resize/peek-poke/bounds-trap/MemCopy |
| `docs/cb_runtime.md` | MODIFY | Mark the Memblock section implemented; record divergences |
| `docs/features/FEATURE_INDEX.md` | MODIFY | Move from Backlog → Active → Completed on close |

## Verification

- `cargo test --workspace` green on the **SDK-free path** (the whole subsystem should run headless via FD-033) — the `runtime_memblock_fd039` driver fixture must *run*, not graphics-skip.
- `cargo test -p cb-runtime-sys` — catalog-content asserts confirm the `Memblock` type (tag 15) and all 14 signatures decode.
- Native `ctest` cases pinning: zero-fill on alloc, resize preserve+zero-fill-growth, byte/short/int/float peek-poke round-trips (incl. 32-bit float precision), `MemCopy` semantics, and the **out-of-bounds trap** + invalid-handle trap.
- `clippy --workspace --all-targets -D warnings` + `fmt --all --check` clean.
- Cross-check exact resize/offset/endianness behavior against `cbEnchanted/src/meminterface.cpp` and the real CB compiler.
- Deferred (if a real display is ever involved — unlikely here, this is headless): n/a.

## Implementation Notes (2026-06-20)

Implemented per the resolved decisions; all 14 functions landed in a single
Allegro-free TU.

- **`runtime/cb_memblock.cpp`** — `struct CbMemblock { std::vector<uint8_t> bytes; }`
  (global namespace, matching the `cb_runtime_func.h` forward-decl convention).
  `std::vector::resize` gives preserve-existing + zero-fill-growth for free.
  Little-endian load/store are written as explicit byte assembly (host-byte-order
  independent). `PeekFloat`/`PokeFloat` `memcpy`-bitcast a `uint32_t` ↔ `float`
  then widen/narrow to the CB `f64`. A file-local `trap()` helper builds a
  `CbString` message and calls `cb_host()->raise_error` (then releases it — the
  interp copies synchronously); with no host connected (native gtest) it's a
  no-op and the caller falls through to a safe default, never UB.
- **No separate header.** Prototypes live in `cb_runtime_func.h`; the `Memblock`
  type-tag (15), type-table entry, and 14 `CB_FN` rows in `catalog.cpp` all sit
  **outside** the `CB_NO_ALLEGRO` guard, so they ship in the SDK-free catalog.
- **Build wiring:** `cb_memblock.cpp` added to the `cb_runtime` CMake target, to
  `build.rs::SDK_FREE_TUS` + `RERUN_SOURCES`, and to the `cb_runtime_tests` gtest
  target.
- **Catalog version unchanged (v6).** Adding a type + functions doesn't change
  any ABI struct layout — confirmed: the existing layout `static_assert`s and
  the `cb-runtime-sys` decode tests pass in both builds.
- **Tests:** `cb-runtime-sys` asserts Memblock tag 15 (both builds), type counts
  2 (SDK-free) / 6 (full), and the 14 signatures; `runtime/tests/test_memblock.cpp`
  (15 cases) pins round-trips, unsigned reads, LE byte order, resize, overlapping
  `MemCopy`, and OOB-returns-safe-default; `crates/cb-driver` adds the
  `runtime_memblock_fd039` golden (not graphics-gated) and a `cli.rs` OOB-trap
  test (exit 1 + `runtime error: PokeInt … out of bounds`).

**Verified (2026-06-20, Windows + Allegro SDK):** SDK-free `cargo test --workspace`
all green (0 failed) + `clippy --workspace --all-targets -D warnings` clean +
`fmt --all --check` clean; full Allegro `cargo test --workspace` all green
(`cb-runtime-sys` 6-type catalog + every graphics fixture + the memblock fixture
in the real linked build — the ODR/linkage gate); native `ctest` **85/85** (70
prior + **15** new `Memblock.*`). No deferred items — the subsystem is fully
headless-testable.

## Related

- `docs/cb_runtime.md` §"Memory Blocks" — the documented surface (already written)
- `D:\projects\cbEnchanted\src\meminterface.{h,cpp}` — reference implementation
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque-type machinery this reuses
- [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) — the trap channel for bounds/handle errors
- [FD-016](archive/FD-016_RUNTIME_CORE_FUNCTIONALITY_SPLIT.md) — the core/functionality split: core is the ABI-required string type + host handshake only, so Memblock goes in the `cb_runtime` functionality lib
- [FD-033](archive/FD-033_CATALOG_MOCK_FOR_SDK_FREE_TESTS.md) — the SDK-free test path this rides
- [FD-036](archive/FD-036_RUNTIME_GAME_OBJECTS.md) — raw-pointer opaque-handle precedent + trap-not-UB policy
- [FD-017](archive/FD-017_RUNTIME_MODULE_COMPLETENESS.md) — `Crc32` memblock form blocked on this
- Backlog follow-ons unblocked: `Crc32(memblock)`, `Encrypt`/`Decrypt`, `CallDLL`
