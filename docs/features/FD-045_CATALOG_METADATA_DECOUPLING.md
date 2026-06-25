# FD-045: Catalog Metadata Decoupling

**Status:** Pending Verification
**Created:** 2026-06-24
**Designed:** 2026-06-25 (4-angle `/fd-deep` pass; claims verified against the code)
**Implemented:** 2026-06-25 (Phases A–C; Phase D remains design-only — plugin loader deferred to FD-009)
**Priority:** Medium (prerequisite for a runtime-free native compiler; not blocking interp work)
**Effort:** High (build-system + ABI surface change; design pass complete)
**Impact:** Lets semantic analysis and a future native/AOT (LLVM) compiler obtain the runtime function/type/constant **catalog as metadata** (symbol name + signature) **without linking the executable C++ runtime**. Today every binary that type-checks must link the full C++/Allegro runtime just to read the catalog — this is the real coupling behind "a native compiler that doesn't need the runtime to run," not the executable-topology question.

> Surfaced by FD-044's `/fd-deep` executable-topology analysis (2026-06-24). The deep analysis concluded that splitting the compiler into separate native/interp executables would **not** shed the runtime dependency, because the catalog (a sema input) is only obtainable from the linked runtime. This FD captures the decoupling that actually unblocks a lightweight native compiler. Filed **Planned** — it is a load-bearing build/ABI decision that needs its own design pass (CLAUDE.md: ask before load-bearing structural choices).

## Problem

Semantic analysis requires the **runtime catalog** — the list of runtime-provided functions, opaque types, and constants with their signatures — so it can resolve builtin calls, check argument types, and seed runtime constants. `cb_sema::analyze(... runtime_catalog: &RuntimeCatalog)` takes it as input (`crates/cb-sema/src/lib.rs`), and the driver loads it on every non-`--dump-ast` path (`crates/cb-driver/src/main.rs:245,261`).

The **only** way to obtain a real catalog today is to link the C++ runtime and call into it at runtime:

- `cb-runtime-sys` declares `extern "C" fn cb_runtime_get_catalog() -> *const CbCatalog` (`crates/cb-runtime-sys/src/lib.rs:168-169`); `load_catalog()` → `fetch_catalog()` calls it and decodes the result (`lib.rs:190-196,266-268`). The comment is explicit: *"The catalog is the only entry point Rust needs from the runtime library"* (`lib.rs:165-167`).
- The catalog is assembled in C++ as static arrays in `runtime/catalog.cpp` (`catalog_funcs[]`, `catalog_types[]`, `catalog_consts[]`), via the `CB_FN(cb_name, fn)` template DSL (`catalog.cpp:206-216`, FD-012).
- Critically, each `CB_FN` entry stores **`reinterpret_cast<void(*)(void)>(fn)`** — a live function pointer (`catalog.cpp:209`, mirrored as `CbFuncDesc::fn_ptr` in `crates/cb-runtime-sys/src/lib.rs:33-37` and `cb_ir::FuncDesc::fn_ptr`). Taking that address **ODR-uses** the function, so producing the catalog forces the entire runtime (and its Allegro transitive closure) to be linked.

Consequence: **any** executable that runs sema links the full C++ runtime — even a pure type-check, a `--dump-ir`, or a future native compiler that never executes a single runtime function. But a native/AOT backend needs only **metadata**: the CB name, the linker **symbol** string (`CbFuncDesc::symbol`, the `#fn` stringification at `catalog.cpp:208`), and the signature (param/return `IrType`s) — enough to emit LLVM `declare`/`call`. The IR already encodes this split: `FuncKind::Runtime { symbol, fn_ptr }` with the note that *"the interpreter dispatches through [`fn_ptr`]; the LLVM backend uses `symbol` for declare/call"* (`crates/cb-ir/src/lib.rs:42-50`). Only the interpreter — which executes runtime functions in-process via libffi — actually needs `fn_ptr`.

The signature data does **not** force linking: `FuncTraits<fn>` / `cb_anon_params<fn>` derive param/return tags from the function **type** (`decltype`), not its address. Only the `fn_ptr` field does. So a metadata-only catalog is mechanically feasible.

## Goal

Provide the catalog **metadata** (functions: name + symbol + param/return types; opaque types: name + tag; constants: name + type + value) to the compiler **without** linking the executable runtime, while keeping the **C++ catalog DSL as the single source of truth** (no hand-maintained Rust duplicate — that is exactly what FD-012's DSL exists to avoid). The interpreter keeps using the full catalog (with `fn_ptr`) by linking the runtime, as today.

Concretely, after this FD:

- `cb-sema` / a native compiler can type-check and emit `declare`/`call` from metadata alone — no C++/Allegro toolchain required for that path.
- `cb-backend-interp` still resolves `fn_ptr` by linking the runtime (it must, to execute in-process).
- The C++ `CB_FN(...)` line stays the one place a runtime function is registered.

## Solution (designed — `/fd-deep` 2026-06-25)

**Decision (user-confirmed):** the core runtime stays **statically linked** — single-exe deployment is preserved (the deliberate `x64-windows-static-md` choice at `crates/cb-runtime-sys/build.rs:196-203`). The compiler obtains the core catalog as **metadata only**; the full C++/Allegro runtime is linked **only on the interpreter path** to supply `fn_ptr`. The catalog metadata schema is designed to generalize to dlopen'd plugins, but **the plugin loader is out of scope for this FD** (it reuses FD-009's spec).

Rationale for this shape over the alternatives considered in the design pass:
- *Dynamically loading the core runtime ("plugin zero")* would literally satisfy "the compiler links no runtime," but it ends single-exe deployment, adds cross-module CRT-shared-heap management, and complicates the SDK-free static-archive story. Rejected for the core; reserved for plugins.
- *A language-neutral `.def` rewrite (old approach #3)* has no historical CoolBasic precedent (signatures lived inside the compiler binary) and would discard the C++ `FuncTraits` `decltype`-deduction that makes signatures impossible to mis-declare. The durable lesson from CoolBasic/cbEnchanted is **the metadata/execution split**, not a file format — so keep the `CB_FN` DSL as the single source of truth.
- *Generate-to-Rust static data* remains a clean **later upgrade** for a pure-Rust / cross-compiled compiler distribution; it is not required now and adds a build-time emitter (compile+link+run a helper) that is currently unprecedented in this repo's `build.rs`.

### Phase A — Data-model split (independent; do first)

Split `cb_ir::FuncDesc` (and the `CbFuncDesc` mirror) so the **metadata** (name, c_symbol, params, return_ty) is separable from the **execution binding** (`fn_ptr`). Verified during the design pass: **sema never dereferences `fn_ptr`** — it is threaded loader→IR and pattern-ignored at `crates/cb-sema/src/check.rs:1117`; the **LLVM backend references neither `fn_ptr` nor `symbol`** (it is a stub); **only the interpreter** consumes it (`crates/cb-backend-interp/src/interp.rs:461` → libffi `ffi.rs:145`). So the split is nearly free for the frontend.

- `cb-ir`: `FuncKind::Runtime { symbol, fn_ptr }` → `FuncKind::Runtime { symbol }` (`crates/cb-ir/src/lib.rs:45-49`); drop `FuncDesc::fn_ptr` (`lib.rs:65`).
- `cb-runtime-sys`: relax `decode_catalog` so a null `fn_ptr` is **valid metadata** (today it errors at `crates/cb-runtime-sys/src/lib.rs:347-349`); add an interp-only `resolve_bindings() -> HashMap<symbol, fn_ptr>`.
- `cb-sema`: delete the now-dead `fn_ptr` field (`scope.rs:66,102`, `check.rs:394,410`, `lower.rs:397,417`).
- `cb-backend-interp`: at startup build the `symbol → fn_ptr` table; look up by `symbol` at `interp.rs:461`. `RuntimeCatalog` becomes constructible from metadata alone.

### Phase B — Metadata-only compilation (the chosen decoupling)

Compile `catalog.cpp` under a new `-DCB_METADATA_ONLY` define: `CB_FN` sets `fn_ptr = nullptr` (drop the lone `reinterpret_cast<void(*)(void)>(fn)` at `runtime/catalog.cpp:209` — the **only** address-take, which is what currently ODR-uses every runtime function and drags in Allegro), and guard the `cb_runtime_string_api` pointer (`catalog.cpp:728`). Removing the cast also makes `catalog_funcs[]` `constexpr` (the comment at `catalog.cpp:280-282` confirms the cast is the sole reason it isn't). This yields a **tiny, Allegro-free object** exposing `cb_runtime_get_catalog_meta()` that references zero function addresses and links with only `catalog.cpp` + headers.

The compiler links **only this metadata object**; the full-runtime link (for `fn_ptr`) moves behind the interpreter path. The metadata object is compiled with the **same `-DCB_NO_ALLEGRO`** switch as the linked runtime, so the two catalogs match by construction (no separate mock needed — both emit the real catalog under the matching define). *(Lean variant first: link the tiny object and call it at runtime. Generate-to-Rust is the later upgrade.)*

### Phase C — Drift guard

The compile-time `#fn` symbol↔pointer tie (`catalog.cpp:204-205`) no longer holds once metadata and bindings come from independently-built artifacts. Replace it with a **fatal interp-startup assert** (matching the existing fatal-by-panic init at `string_api()`, `crates/cb-runtime-sys/src/lib.rs:220-228`, and cbEnchanted's `link()` assert): every metadata `symbol` must resolve to a non-null binding and vice-versa. Strengthen with a **catalog content hash** over the sorted `(name, symbol, param_tags, return_tag)` tuples, compared between the metadata object and the linked runtime, to catch signature drift the symbol-set check alone would miss.

### Phase D — Plugin-ready schema (design only; loader deferred to FD-009)

Shape the metadata schema so a future plugin DLL is just another catalog source:
- Carry a `magic` + `plugin-name` + `semver` in the catalog struct (behind a `CB_CATALOG_VERSION` bump); reuse the existing `CB_HOST_ABI_VERSION` handshake (each DLL already exports `cb_runtime_init`).
- **Cross-module identity is the CB name (namespaced), not the linker symbol** — cbEnchanted keys dispatch on stable IDs / `(groupId, funcId)`, not symbols a third party can't reproduce. `symbol` stays the *binding* key for the statically-linked core only.
- **Type tags stay catalog-local** (a compact index into each catalog's own param arrays) with host-assigned remapping at merge time. `IrType::RuntimeType(String)` is already name-based (`crates/cb-runtime-sys/src/lib.rs:452-453`), so the IR/type-system is already insulated from the hand-assigned tags 10–18.
- `CallDLL` (backlog) is explicitly **not** this mechanism — it is a runtime escape hatch (memblock-marshalled, fixed C ABI, type-checked only at the call statement). Keep them separate.

## Open questions

Resolved in the design pass:

- ~~**Target-vs-host catalog content.**~~ **Resolved:** the metadata object is compiled with the **same `-DCB_NO_ALLEGRO`** switch as the linked runtime, so host and metadata catalogs match by construction. Cross-compilation targets (metadata for a runtime other than the host's) remain future work, layered on the plugin schema (Phase D).
- ~~**`fn_ptr` reconciliation for interp.**~~ **Resolved:** Phase C — fatal interp-startup assert (every metadata `symbol` resolves to a non-null binding and vice-versa) + a catalog content hash to catch signature drift.
- ~~**Where does generated metadata live / how is it tested?**~~ **Resolved (lean variant):** the compiler links the tiny metadata-only object — no generated artifact, no separate mock. Both the metadata object and the linked runtime emit the real catalog under the matching `-DCB_NO_ALLEGRO` define, so FD-033's SDK-free path is unaffected. (If the generate-to-Rust upgrade is later taken, the emitted data lives in `OUT_DIR`, regenerated each build keyed off the existing `RERUN_SOURCES`.)
- ~~**Scope vs. FD-044.**~~ **Resolved:** FD-044 has shipped. Phase A (the data-model split) is independent and can land anytime; Phases B–C deliver the decoupling when the runtime-link weight bites.

Still open:

- **Identity key for the plugin schema.** Confirm name-vs-symbol: recommended is **CB name (namespaced) as cross-module identity** for plugins, with the linker `symbol` retained as the binding key for the statically-linked core only (Phase D). The exact namespacing scheme (`plugin::Type`, group IDs) is a Phase-D detail to settle when the loader (FD-009) is built.
- **Build experiment (gates Phase B).** The "`-DCB_METADATA_ONLY` severs the Allegro link" claim is verified by **code reading only** (`catalog.cpp:209` is the sole address-take). It needs an actual compile to confirm the metadata object links with zero Allegro/subsystem symbols **before** committing to Phase B — see Verification.

## Files to Create/Modify

| File | Action | Phase | Purpose |
|------|--------|-------|---------|
| `crates/cb-ir/src/lib.rs` | MODIFY | A | `FuncKind::Runtime { symbol }` (drop `fn_ptr`); drop `FuncDesc::fn_ptr`; `RuntimeCatalog` constructible from metadata alone |
| `crates/cb-sema/src/{scope,check,lower}.rs` | MODIFY | A | Delete the now-dead `fn_ptr` field threaded through `DeclKind::RuntimeFn`/`OverloadVariant` (verified never dereferenced) |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | A,B | Relax `decode_catalog`'s null-`fn_ptr` error (`:347-349`); add interp-only `resolve_bindings() -> HashMap<symbol, fn_ptr>`; expose `cb_runtime_get_catalog_meta()` |
| `crates/cb-backend-interp/*` | MODIFY | A,C | Build `symbol → fn_ptr` table at startup; dispatch by `symbol` (`interp.rs:461`); fatal startup drift assert + content-hash check |
| `runtime/catalog.cpp` | MODIFY | B | `-DCB_METADATA_ONLY` path: `CB_FN` emits `fn_ptr = nullptr` (drop the `reinterpret_cast` at `:209`), guard the string-API pointer (`:728`); emit a content hash |
| `crates/cb-runtime-sys/build.rs` | MODIFY | B | Compile the metadata-only object (Allegro-free); link the full runtime only on the interp path |
| `runtime/cb_runtime_core.h` / `cb_runtime_func.h` | MODIFY | C,D | `CB_CATALOG_VERSION` bump; add `magic`/`plugin-name`/`semver` + content-hash fields to the catalog struct; update the static_assert layout pins |

## Verification

- **Build experiment first (gates Phase B):** compile `catalog.cpp` with `-DCB_METADATA_ONLY` and confirm the resulting object links with **zero** Allegro/subsystem symbols (the one claim not yet verified by build).
- A build/test that type-checks a program (sema) **without** linking the executable runtime (e.g. metadata-only, `--dump-ir` path) — proving the C++/Allegro link is no longer a type-check prerequisite.
- Interpreter still runs every existing fixture — `fn_ptr` overlay resolves all symbols; the startup check fails loudly on any missing/extra symbol or content-hash mismatch (drift guard).
- Catalog metadata matches the linked runtime's catalog under both full-Allegro and SDK-free configs (`HAS_GRAPHICS` gating preserved).
- `cargo test --workspace` green across the FD-025 four-feature matrix; clippy/fmt clean.

## Implementation Outcome (Phases A–C; verified 2026-06-25, Windows, full-Allegro + SDK-free)

Phases A–C landed; **Phase D stays design-only** (plugin loader deferred to FD-009 — no `magic`/`semver`/`CB_CATALOG_VERSION` bump was needed for A–C, the catalog struct layout is unchanged).

- **Phase A — data-model split:** `FuncKind::Runtime { symbol }` (dropped `fn_ptr`); dropped `FuncDesc::fn_ptr`; deleted the dead `fn_ptr` field threaded through `DeclKind::RuntimeFn` / `OverloadVariant` (`scope.rs`, `check.rs`, `lower.rs`, `lower_snapshots.rs`); `decode_catalog` now accepts a **null `fn_ptr` as valid metadata**; added interp-only `resolve_bindings() -> HashMap<symbol, fn_ptr>`. The interpreter resolves the overlay once at startup and dispatches by `symbol`.
- **Phase B — metadata-only object:** `runtime/catalog.cpp` under `-DCB_METADATA_ONLY` — `CB_FN_PTR(fn)` nulls the lone `reinterpret_cast<void(*)(void)>(fn)` address-take, function bodies are `#ifndef`-guarded out, the `cb_runtime_string_api` pointer is guarded, and a distinct `cb_runtime_get_catalog_meta()` entry point links alongside the full runtime. `build.rs` compiles it under the **same `CB_NO_ALLEGRO` switch** as the linked runtime (both `build_sdk_free` and `build_full`), so the two catalogs match by construction. Sema reads the metadata catalog via `load_catalog()` → `fetch_catalog_meta()`.
- **Phase C — drift guard:** chosen as a Rust-side structural `reconcile_catalogs()` (symbol-set + signature-tuple comparison) over a C++-emitted content hash — equivalent coverage, precise drift message. `resolve_bindings_checked()` reconciles metadata vs full catalog at interpreter startup; fatal-by-panic on any missing/extra symbol or signature drift, matching the `string_api`/`runtime_init` init policy.

**Verification results:**
- **Build-experiment (the gating claim), now confirmed by build:** `dumpbin /SYMBOLS` on the **full** metadata-only archive (`cb_runtime_meta.lib`, carrying the graphics/input/text `CB_FN` rows) shows exactly **one** undefined external symbol — `_fltused` (CRT float marker). **Zero** Allegro (`al_*`) refs, **zero** `cb_rt_*` runtime-function-body refs. `-DCB_METADATA_ONLY` severs the Allegro link as predicted.
- `cargo test --workspace` green (exit 0): `cb-runtime-sys` 24/24 (incl. `decode_allows_null_fn_ptr` and the four `reconcile_catalogs` drift tests — matching / missing / extra / signature-drift), `cb-backend-interp` 46/46 (every fixture dispatches through the `symbol → fn_ptr` overlay with the startup reconcile running on each construction), `cb-sema` 180 + 46 lower-snapshots (`fn_ptr`-free lowering, snapshots unchanged).
- **SDK-free config** (`CB_RUNTIME_FORCE_SDK_FREE=1`): `cb-runtime-sys` 24/24 + `cb-backend-interp` 80/80 green — the metadata object + drift guard build and reconcile under `-DCB_NO_ALLEGRO` too.
- `clippy --workspace --all-targets -D warnings` clean; `fmt --all --check` clean.

**Known gap (consistent with FD scope):** the build still links **both** the full runtime and the metadata object — there is no build target yet that links *only* the metadata object, so the "type-check without linking the executable runtime" outcome is established at the ABI/data-model seam but not yet realized as a runtime-free binary. That binary (a metadata-only `cb-runtime-sys` build config + a driver path) is follow-up work; the native compiler that consumes the seam was always future scope here.

Proofread fix folded in at verify: corrected two stale `fetch_catalog`/`string_api` doc comments that still claimed `load_catalog` shares `fetch_catalog()` (it now reads `fetch_catalog_meta()`).

## Related

- [[FD-044]] Backend Trait Seam — surfaced this FD; the executable-topology decision identifies catalog decoupling (not a binary split) as the real native-compiler prerequisite.
- [[FD-012]] Catalog C++ Template DSL — the `CB_FN`/`FuncTraits` source of truth this must preserve.
- [[FD-009]] Runtime Library — documents the intended LLVM-vs-interp runtime relationship (interp links + dispatches; LLVM declares symbols + links the runtime into its *output*).
- [[FD-033]] Catalog Mock for SDK-Free Tests — existing mechanism for catalogs without the full toolchain; must coexist with generated metadata.
- [[FD-029]] Runtime-Defined Constants — constants are part of the catalog metadata this decouples.
- `crates/cb-ir/src/lib.rs` — `FuncKind::Runtime { symbol, fn_ptr }`, `FuncDesc`; the data-level split point.
