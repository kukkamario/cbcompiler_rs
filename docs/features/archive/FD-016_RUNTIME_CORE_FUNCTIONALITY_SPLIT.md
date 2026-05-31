# FD-016: Runtime Core / Functionality Split

**Status:** Complete
**Completed:** 2026-05-31
**Priority:** Medium
**Effort:** High (> 4 hours) — build-system refactor + header partition + ABI contract definition
**Impact:** Separates the irreducible runtime **core** (string type, catalog ABI structs, FD-015 host/hook types) from the **functionality** library (Math, String funcs, System, Graphics, Input) so that plugins can statically link a tiny, Allegro-free core to handle `String` parameters and register catalog entries — without dragging in the whole functionality library or its Allegro/vcpkg dependency closure. Produces a clean plugin SDK surface (`cb_runtime_core.h` + `cb_runtime_core.lib`) for free.

## Problem

The C++ runtime is currently **one** static library `cb_runtime` (`runtime/CMakeLists.txt`) wrapping six TUs and the entire Allegro dependency closure, surfaced to Rust through the single `cb-runtime-sys` crate. This conflates two very different things:

- **Core** — what the compiler/backends cannot function without and what a **plugin** must reference to accept/return a `String`: the `CbString` type and its primitives, the catalog descriptor structs (`CbTypeTag`/`CbTypeDesc`/`CbParamDesc`/`CbFuncDesc`/`CbCatalog`), `CbStringApi`, and the FD-015 host/hook types (`CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init`). This is Allegro-free.
- **Functionality** — the actual feature subsystems: Math, the String *library* (`Left`/`Upper`/`Instr`/…), System/Time, Graphics, Input — plus the catalog *assembly* that binds them into a `CbCatalog`. Only Graphics and Input pull in Allegro.

Plugins (FD-009 specs a `libloading`-based DLL loader) need to handle `String` parameters, which means referencing `CbString` and the catalog ABI. Today the only artifact that provides those is the monolithic `cb_runtime` lib, which would force every plugin to link Allegro and the full functionality set. The boundary is already a clean DAG in practice (functionality → core; core depends on nothing functional — verified: `catalog.cpp` references `&cb_runtime_string_api`, no core file references a functionality symbol), but it is not expressed as a build/link boundary.

This blocks a clean plugin SDK and entangles FD-015 (whose host/hook *types* belong in core) with a monolithic header.

### Key design decision: plugins handle strings **statically**, not through a vtable

An earlier framing assumed plugins would manipulate strings through a function-pointer table (`CbStringApi`) handed over at init, to avoid two divergent copies of the string implementation. Investigation of the actual code shows this indirection is **unnecessary** and that plugins can link the string implementation statically and call it directly. Every identity the string type relies on is **value-based, not address-based**:

| Property | Mechanism | Evidence |
|----------|-----------|----------|
| Immortality (retain/release no-op) | `refcount < 0` (value) | `cb_string.cpp:61` `is_static()` |
| Emptiness | `len == 0` (value) | `string_handle.rs:58`, `cb_string.cpp:120` |
| String equality (`a$ = b$`) | **byte compare** | `interp.rs:992` `a.as_bytes() == b.as_bytes()` |

The single `CB_EMPTY_STRING_INSTANCE` *address* is used only as an intra-module allocation-avoidance shortcut; no correctness path does pointer-identity against it (the only `== api.empty` comparisons are in tests). So a plugin having its **own** copy of the string code — and therefore its own empty sentinel — is safe: the host releases a plugin-created sentinel by reading its negative refcount (a value) and short-circuiting.

The one hard requirement is a **shared heap**: a `CbString*` allocated in one module and freed in another must hit the same `malloc`/`free`. This is **already guaranteed** by the build's `x64-windows-static-md` triplet (dynamic `/MD` CRT — `build.rs:43-47`), which makes all modules share one ucrt heap. There is existing precedent in-tree: the host's own string *library* already calls `cb_rt_string_from_literal`/`retain`/`len` by **direct symbol**, not through the vtable (`cb_string.cpp:120-356`). A plugin is just one more module doing the same — the only new wrinkle is being a separate DLL, which the shared CRT covers.

## Solution

A principled division falls out of the above:

- **Pure data-structure operations → static link, direct calls.** Strings (and any future pure-data type, e.g. Memblock) are heap operations any module can run on the shared heap. Plugins statically link `cb_runtime_core` and call `cb_rt_string_concat(a, b)` directly — **no indirection on the hot path** (this is the efficiency win that motivates the static approach over a vtable).
- **Host-*policy* operations → `cb_runtime_init` vtable.** FD-015's `request_exit`/`raise_error`/hooks genuinely cannot be statically linked — only the host knows how to unwind the interpreter. They stay behind the handshake, but fire rarely (a trap, a window close), so their indirection cost is irrelevant.

The `CbStringApi` vtable does **not** disappear — it remains how the **Rust host** marshals strings and the seam FD-014 chose for LLVM uniformity — but it is no longer the *required* plugin path, just the host's internal one.

### C++ side

1. **Header partition.** Split `cb_runtime.h` into:
   - `cb_runtime_core.h` — `CbTypeTag` + tag macros, `CbString` fwd-decl, `CbStringApi`, the catalog descriptor structs, `CB_CATALOG_VERSION`, `cb_runtime_get_catalog` decl, the six string-primitive prototypes, and the FD-015 `CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init` declarations. **This is the plugin SDK header.**
   - `cb_runtime_func.h` — Math/System/Graphics/Input prototypes + the `CbImage` typedef (move it out of the core header; only `catalog.cpp` needs `type_tag<CbImage*>`).
2. **Extract the string library.** Move the CB-visible string functions (`Left`/`Upper`/`Trim`/`Instr`/`Chr`/`Hex`/…, `cb_string.cpp:140-357`) into a new `cb_strfuncs.cpp` in the functionality lib. `cb_string.cpp` keeps only the primitives, the sentinel, `cb_runtime_string_api`, and `cb_rt_string_test_refcount`.
3. **Two CMake STATIC targets.**
   - `cb_runtime_core` = `cb_string.cpp` (primitives only) + a new `cb_host.cpp` (FD-015 `cb_runtime_init`, `g_host` file-static + accessor). **No `find_package(Allegro)`, no transitive-closure walk.**
   - `cb_runtime` (functionality) = `catalog.cpp` + `cb_math.cpp` + `cb_system.cpp` + `cb_gfx.cpp` + `cb_input.cpp` + `cb_strfuncs.cpp`; `target_link_libraries(cb_runtime PUBLIC cb_runtime_core ${Allegro…})`. The whole transitive-link-closure machinery (`cb_collect_transitive_links`, the `file(GENERATE …)`, `ALLEGRO_STATICLINK`, the shell32 hack) moves here.
   - `cb_runtime_init` and `g_host` live in **core** (`cb_host.cpp`) so a core-only plugin gets the handshake without dragging Allegro; functionality `cb_rt_*` read `g_host` via a core-exported accessor. The catalog assembly (`catalog.cpp`, `cb_runtime_get_catalog`) stays in **functionality** because `CB_FN` forces link-resolution of every named `cb_rt_*` symbol.

### Rust side

Keep **one** crate (`cb-runtime-sys`) linking **two** static libs. No downstream Rust consumer needs "core without functionality" — `lib.rs` only names `cb_runtime_get_catalog` + the string-test hook; everything else flows through `fn_ptr`. `build.rs` emits two `rustc-link-lib=static=` lines in dependency order (functionality before core for GNU ld; MSVC is order-insensitive). A second crate would only earn its keep if a Rust plugin SDK were published, but FD-009 defines the plugin ABI as the **C header**, so the natural SDK is `cb_runtime_core.h` + the tiny Allegro-free `cb_runtime_core.lib`.

### Plugin ABI contract (document)

The static-string approach trades SQLite's full decoupling for a build-time contract that must be documented in the plugin SDK:

- Plugins **must** be built against the same CRT (dynamic `/MD`) and the same `cb_runtime_core.h` ABI version (`CB_CATALOG_VERSION` guards header-revision drift). A static-CRT plugin breaks cross-module `free`.
- Invariant to preserve everywhere: **emptiness is `len == 0`, immortality is `refcount < 0` — never an address compare.** Nothing in-tree violates this today; keep it that way.
- Each plugin carries its own ~200-line copy of the string code (negligible size cost).

### Coordination with FD-015

**Resolved: FD-016 lands first, then FD-015.** This FD performs the header partition and stands up `cb_runtime_core.h`; FD-015 then defines `CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init` directly in that core header (and implements `cb_runtime_init` in the core `cb_host.cpp`) rather than touching a monolithic header. This simplifies FD-015's file-change list and gives the trap-channel types their permanent home from the start. (Note: with static string handling decided here, FD-015 does **not** need to extend `CbHostApi` to carry `CbStringApi` to plugins — that earlier idea is dropped.) FD-016 stubs the FD-015 declarations in `cb_runtime_core.h` only insofar as the partition requires; the trap-channel semantics remain FD-015's scope.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime_core.h` | CREATE | Plugin SDK header: type tags, `CbString` fwd-decl, `CbStringApi`, catalog descriptor structs, `CB_CATALOG_VERSION`, `cb_runtime_get_catalog` decl, string-primitive prototypes, FD-015 `CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init`. |
| `runtime/cb_runtime_func.h` | CREATE | Functionality prototypes: Math/System/Graphics/Input + `CbImage` typedef. |
| `runtime/cb_runtime.h` | MODIFY | **Resolved: umbrella now, remove later.** Reduce to a back-compat umbrella that `#include`s both `cb_runtime_core.h` and `cb_runtime_func.h`, so existing TU includes keep working during the transition. Migrate each TU to the narrower header, then delete this umbrella in a follow-up once nothing includes it. |
| `runtime/cb_string.cpp` | MODIFY | Keep only primitives, sentinel, `cb_runtime_string_api`, `cb_rt_string_test_refcount`. Include `cb_runtime_core.h`. |
| `runtime/cb_strfuncs.cpp` | CREATE | The moved CB-visible string library (`Left`/`Upper`/`Instr`/…). Functionality lib. |
| `runtime/cb_host.cpp` | CREATE | FD-015 `cb_runtime_init` + `g_host` file-static + accessor. Core lib. |
| `runtime/cb_math.cpp`, `cb_system.cpp`, `cb_gfx.cpp`, `cb_input.cpp`, `catalog.cpp` | MODIFY | Switch includes to the narrower headers; no logic change. `catalog.cpp` stays in functionality. |
| `runtime/CMakeLists.txt` | MODIFY | Two STATIC targets (`cb_runtime_core` Allegro-free, `cb_runtime` functionality linking core + Allegro). Move transitive-closure machinery to the functionality target. |
| `crates/cb-runtime-sys/build.rs` | MODIFY | Emit two `rustc-link-lib=static=` lines (functionality before core); update `rerun-if-changed` file list for the new/split sources. |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY (minimal) | No symbol-set change expected; verify the ABI-version assert and `#[repr(C)]` mirrors still match the partitioned header. |
| `docs/cb_runtime.md` | MODIFY | Document the core/functionality split and the plugin ABI contract (CRT + version + value-based-identity invariant). |
| `docs/features/archive/FD-009_RUNTIME_LIBRARY.md` | DONE | Superseding callouts added: the `char*`/`cb_string_new` string model (→ FD-014 opaque `CbString*`), per-call `dlsym` dispatch (→ FD-012 `fn_ptr`), and the "back-link the host exe" plugin model (→ FD-016 static core link) are marked superseded. |

## Verification

- `cargo build --workspace` and `cargo test --workspace` green — the split is purely structural; no behavior change.
- `cb-runtime-sys` catalog test still asserts the ABI version and that `cb_runtime_get_catalog` round-trips; string refcount/sentinel tests (`string_handle.rs`, `lib.rs`) still pass.
- Inspect the built artifacts: `cb_runtime_core.lib` has **no** Allegro/vcpkg symbols (confirm the core target links none of the closure); `cb_runtime.lib` carries the functionality + Allegro closure.
- Link-order sanity on a GNU-ld target (if/when Linux is built): functionality-before-core resolves cleanly.
- (Forward-looking, no code yet) A throwaway out-of-tree plugin DLL that links **only** `cb_runtime_core`, returns a catalog with a `String`-typed function, and is dispatched by the interpreter via `fn_ptr` — strings created in the plugin are released by the host without crashing (validates the shared-heap + value-based-sentinel model). Defer to the FD that actually implements the loader.

## Related

- [FD-015](FD-015_RUNTIME_TRAP_CHANNEL.md) — its `CbHostApi`/`CbRuntimeHooks`/`cb_runtime_init` types belong in the new core header; host-policy callbacks are the one thing that stays behind a vtable rather than being statically linked.
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — specs the plugin DLL loader (`--plugin`, `libloading`, `cb_runtime_get_catalog`, catalog merge). Its `dlsym`/`GetProcAddress` per-call symbol-resolution model and exported `cb_string_new()` predate FD-012 (`fn_ptr` dispatch) and FD-014 (`CbStringApi`); **reconciled as part of this FD** via superseding callouts in the archived doc (per-call dispatch is `fn_ptr`, not symbol; plugins static-link `cb_runtime_core` rather than back-linking the host exe).
- [FD-014](archive/FD-014_RUNTIME_STRING_ABI.md) — defines `CbString`, the refcount/negative-sentinel design, and `CbStringApi`. The value-based identity (negative refcount = immortal, `len==0` = empty) is what makes per-module static string handling safe.
- [FD-012](archive/FD-012_CATALOG_CPP_TEMPLATE_DSL.md) — the `CB_FN` DSL and `fn_ptr` libffi dispatch; the reason the catalog assembly must live in the functionality lib (it link-resolves every `cb_rt_*`).
- `docs/cb_runtime.md` — runtime function catalog and termination semantics.
