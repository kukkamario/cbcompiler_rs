# FD-014: Runtime String ABI

**Status:** Open
**Priority:** High — blocks the String portion of [FD-013](FD-013_EXTENDING_RUNTIME_SUPPORT.md) and any new runtime function that takes/returns a string beyond `print`. Also blocks meaningful work on the LLVM backend, since strings cross the FFI boundary on every non-trivial CB program.
**Effort:** Medium (design + small implementation; surface area is `cb_runtime.h`, `catalog.cpp`, `cb-backend-interp/src/ffi.rs`, `cb-backend-interp/src/value.rs`, plus the LLVM emission scheme — even if `cb-backend-llvm` is still a stub, decide the contract now).
**Impact:** Locks down the cross-language string contract so every future runtime FD (string library, file I/O, text rendering, error messages) can land without re-litigating ownership, encoding, or lifetime each time — *and* so the eventual LLVM backend has a string representation it can emit efficient IR for.

## Problem

Today's catalog declares `CB_TYPE_STRING = const char*` (null-terminated UTF-8) and `cb-backend-interp/src/ffi.rs` round-trips through `CString::new` / `to_string_lossy`. That works for `print("hello")`. It does not work for either of the things we actually need:

1. **A full string runtime** — `Left`, `Right`, `Mid`, `Upper`, `Lower`, `Trim`, `Replace`, `InStr`, `Str`, `Chr`, `Asc`, `Len`, … per `docs/cb_runtime.md`. Many of these *construct* strings; the catalog has no ownership / release hook.
2. **LLVM-emitted code that uses strings cheaply.** This is the dominant consumer. The interpreter calls a runtime function ~once per source-level operation; LLVM-emitted code calls runtime string operations on the hot path of nearly every CB program (string vars, `+` concat, comparisons, `Print` of formatted values, file I/O, text rendering). The shape of the string ABI directly determines the size and cost of every hot-path IR sequence.

### What the CoolBasic spec already settles

Re-read `docs/cb_syntax.md` before designing — two points pin the design space tightly:

- **§3.1 / §4.1** — `String` is **UTF-8**. The encoding debate (UTF-8 vs UTF-32 vs UTF-16) is closed. The legacy `LString` is UTF-32 internally for O(1) code-point indexing, but CB-the-language never exposes `[]` indexing on strings (§5.3: "Indexing strings with `[]` is **not** part of the language; use runtime-library functions"). We can keep UTF-8 everywhere and pay the code-point walk cost inside `cb_rt_mid` / `cb_rt_left` / `cb_rt_asc` where it belongs.
- **§5.6.1 type table** — "String — by value (logically); implementation may copy-on-write". Refcounted sharing is *spec-blessed*, not a hack we invent.

### Why the interpreter isn't the right lens

`Value::String(Rc<str>)` in the interpreter is fine — Rust-side ownership, immutable, refcounted, UTF-8. The ABI question is purely "how does a string cross the C boundary." On the interpreter side that's one allocation per call, which we accept. The LLVM-emitted side is the bigger pressure:

- Every CB `String` local lives in an LLVM IR alloca. Its lifetime needs an end-of-scope cleanup hook.
- Every `a$ = b$` needs to either copy bytes (value semantics, expensive) or share the underlying storage (cheap, needs the spec-blessed COW).
- Every `Len(s$)` should be O(1), not a `strlen` walk.
- Every string literal needs to lower to something LLVM IR can emit as a constant initializer (`@.str = private unnamed_addr constant ...`) without runtime construction.

## Solution

**Not yet decided — this FD's job is to pick one.** Three candidates, evaluated primarily through the LLVM lens:

### Option A — Bare `const char*` (status quo, with explicit ownership rules)

Inputs: `const char*` (borrowed). Outputs: `const char*` (caller-owned heap allocation via a documented `cb_rt_alloc` / `cb_rt_free` pair).

- **Problem:** Incompatible with value semantics without copying on every assignment. `a$ = b$` either aliases (wrong: a later mutation of `a` mutates `b`) or copies (expensive: every assignment is O(n)). Since CB strings *are* immutable from the user's perspective, aliasing isn't strictly observable — but the LLVM IR for `a$ = b$ + c$ ; b$ = "x"` has to either copy `b$ + c$` into `a$`'s slot or implement refcounting anyway. Once we're doing refcounting, just expose the refcount in the ABI (Option B).
- **`Len(s$)` is O(n)** (`strlen` walk). LLVM emits a runtime call for every length read. This alone is disqualifying.

**Verdict: rejected.** Listed for completeness; don't pick this.

### Option B — Opaque refcounted handle `CbString*` (port of legacy `LString` / `CB_StringData*`)

Strings flow as `CbString*` — an opaque pointer to a header `{ atomic refcount; size_t byte_len; size_t capacity; uintptr_t offset; uint8_t data[] }` (essentially the legacy `LStringData` layout, minus the UTF-32 `mUtf8String` cache since we are UTF-8 natively).

Catalog gains three primitives:
- `CbString* cb_rt_string_retain(CbString*)` — atomic increment, returns same handle.
- `void cb_rt_string_release(CbString*)` — atomic decrement; frees on zero. No-op on the static-data sentinel (see below).
- `CbString* cb_rt_string_from_literal(const char* data, size_t len)` — returns a handle wrapping a static literal *without copying*. The header is laid out so `offset` distinguishes inline-vs-external data (legacy `isStaticData()` check at `lstring.cpp:103`). Release is a no-op on these.

All string-returning runtime functions return `CbString*`. All string-taking runtime functions take `CbString*` (borrowed unless documented otherwise — Rust-style borrow semantics).

**LLVM emission sketch:**

```llvm
; CB source: a$ = b$ + c$
%t1 = call %CbString* @cb_rt_string_concat(%CbString* %b, %CbString* %c)
; store into a$, releasing old value first
%old = load %CbString*, %CbString** %a_slot
call void @cb_rt_string_release(%CbString* %old)
store %CbString* %t1, %CbString** %a_slot

; CB source: a$ = b$       (shared-storage assignment)
call void @cb_rt_string_retain(%CbString* %b)
%old = load %CbString*, %CbString** %a_slot
call void @cb_rt_string_release(%CbString* %old)
store %CbString* %b, %CbString** %a_slot

; CB source: Len(s$)
%len = call i64 @cb_rt_string_len(%CbString* %s)   ; load from header.byte_len, O(1)

; CB source: "hello"
@.str.hello = private constant %CbStringStatic { i32 -1, i64 5, i64 5, i64 24, [5 x i8] c"hello" }
%lit = bitcast %CbStringStatic* @.str.hello to %CbString*
```

Refcount=−1 (or any sentinel) marks static data so `cb_rt_string_release` short-circuits. The static-data initializer is fully constexpr-emittable in LLVM IR — no per-call construction.

- **Pros:**
  - `Len` is O(1) — single load.
  - `a$ = b$` is one retain + one release (well-known ARC-style pattern; can later be optimized to elide pairs on dead values).
  - String literals are emitted as constant initializers; no per-call malloc.
  - Refcount lets `cb_rt_string_concat` etc. *share* storage in common cases (e.g., concat with empty string).
  - Direct port of `LString` machinery — we are not designing from scratch.
  - Interpreter ffi.rs can wrap each `CbString*` in a small RAII `Value::String(CbStringHandle)` that calls retain/release on Clone/Drop. (`Rc<str>` goes away in favor of a thin `Rc<CbStringHandle>` wrapper — or we keep `Rc<str>` for ergonomics and only materialize handles at FFI boundaries, accepting one alloc per call on the interpreter path.)
- **Cons:**
  - LLVM emission has to insert retain/release pairs at every assignment / scope exit. Non-trivial IR plumbing; reuses the same machinery we'll eventually need for `Type` instances too (FD-010 slab handles aren't refcounted today, but the principle is the same).
  - The catalog gains a destructor concept. That's OK — strings are the first type that needs one; eventually `Memblock`, `File`, `Image` will too. Better to bake in a generic "type has a release hook" slot in `CbTypeDesc` than to special-case strings.
  - First *primitive* type that's also a handle. The tag stays `CB_TYPE_STRING` (don't promote to a runtime-defined ≥10 tag) — the difference is purely "this primitive has a release hook" recorded in the catalog.

### Option C — Fat pointer `{ const char* data; size_t len }`, unique-owned, callee-allocated

`CbStr` struct passed by value across the boundary. Returns are caller-owned; caller must call `cb_rt_free` (or transfer ownership into the interpreter `Rc<str>` / LLVM-side wrapper).

- **Pros:**
  - Length-prefixed, so `Len` is O(1).
  - Simpler ABI than B (no atomic refcount, no header arithmetic).
  - Struct-by-value is well-defined in libffi and LLVM.
- **Cons:**
  - `a$ = b$` either copies (O(n) per assignment, no sharing) or you bolt refcounting on top of the fat pointer (and now you have all of Option B's complexity plus a bigger ABI value).
  - String literal sites still emit a constant `{ @.str, 5 }` initializer — fine — but every `a$ = "literal"` either takes a borrow (lifetime questions) or copies into a heap allocation (wasteful for short-lived literals).
  - Doesn't match `LString` — no port path, every function in `cb_string.cpp` is rewritten.

**Verdict:** Worth keeping in the running only if we discover B's retain/release codegen is unacceptable. The bet is that ARC-style codegen is well-understood and the optimization wins (no per-assignment copy) dominate.

### Decision matrix (LLVM-emitted hot-path cost)

| Operation | A: bare `const char*` | **B: refcounted handle** | C: fat pointer (unique) |
|---|---|---|---|
| String literal `"hello"` | `gep .rodata` | const-initialized struct, no runtime call | const `{gep, len}` struct |
| `Len(s$)` | call → `strlen` (O(n)) | call → header load (O(1)) | load `.len` field (O(1), no call) |
| `a$ = "lit"` | store ptr | retain (no-op for static) + release old + store | store struct |
| `a$ = b$` | aliases or O(n) copy | retain + release old + store | O(n) copy (or refcount → reinvents B) |
| `a$ = b$ + c$` | 2 FFI calls (size, then write) | 1 FFI call → handle | 1 FFI call → fat ptr |
| Scope exit | requires bookkeeping for ownership | release call | free call |
| String compare `=` | `memcmp` after `strlen` | ptr-eq fast path + `memcmp` on header lens | `memcmp` on struct lens |

### Recommendation

**Option B**, with the exact `LString` layout ported from `../CBCompiler/Runtime/lstring.{h,cpp}` modulo:
1. Drop the UTF-32 internal representation; data is UTF-8 inline (CB §3.1 says UTF-8, and §5.3 forbids `[]` indexing so code-point-indexed access never needs to be O(1)).
2. Drop the cached `mUtf8String` since the inline data *is* UTF-8.
3. Keep the static-data sentinel (legacy `mOffset != sizeof(LStringData)` → adapt to "refcount = -1" or "high bit set").
4. Keep atomic refcount semantics (`atomicint.h`) so the type is thread-safe by default — same cost as non-atomic on x86, future-proof.

Catalog ABI v4 adds:
- `CbString` opaque struct (forward-declared in `cb_runtime.h`).
- `CB_TYPE_STRING` continues to mean "string" — but `CbTypeDesc`-equivalent for primitives needs an optional `release_fn` slot (or just hardcode the three string primitives `cb_rt_string_retain` / `_release` / `_from_literal` and require the backend to know about them).
- Three primitives: `cb_rt_string_retain`, `cb_rt_string_release`, `cb_rt_string_from_literal`. These are *not* CB-visible functions; they're backend plumbing exported through the catalog struct as named fields, not via the function table.

The interpreter migration is: `Value::String(Rc<CbStringHandle>)` where `CbStringHandle` is a thin Rust wrapper that calls `cb_rt_string_retain` on `Clone` and `cb_rt_string_release` on `Drop`. ffi.rs no longer allocates per call — it just passes the handle through.

## LLVM-side open work this FD unblocks

These don't need to be designed here, but the ABI choice should be checked against them:

- **String-typed locals in LLVM IR.** Each becomes an `alloca %CbString*` initialized to the empty-string literal handle; scope exit emits `cb_rt_string_release`.
- **Concat lowering.** `a + b` (string-typed) lowers to a single `call %CbString* @cb_rt_string_concat(a, b)`. No inline concat IR.
- **Numeric → string coercion** (§3.4 / §3.471). Emit a call to `cb_rt_string_from_int` / `cb_rt_string_from_float`. Returns a fresh handle (refcount 1).
- **Comparison.** `=` / `<>` on strings lowers to `call i32 @cb_rt_string_cmp` (or a Rust-side intrinsic — keep the implementation behind one symbol).
- **Optimization pass: retain/release elision.** Like Swift ARC — pair a retain with the next release in the same block. Future work, not blocking.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime.h` | MODIFY | Forward-declare `CbString`; add `CbStringStatic` layout (for emission-time constant initializers); declare `cb_rt_string_retain` / `_release` / `_from_literal` / `_len` / `_concat`; bump `CB_CATALOG_VERSION` to 4. |
| `runtime/cb_string.cpp` (new) | CREATE | Port `LString` / `LStringData` to UTF-8 inline storage. Implements the retain/release/literal/len/concat primitives. |
| `runtime/catalog.cpp` | MODIFY | Adjust `type_tag<>` specializations: `CbString*` → `CB_TYPE_STRING`. Add the string primitives to the catalog (likely as named fields on `CbCatalog` rather than as CB-visible functions). |
| `crates/cb-backend-interp/src/ffi.rs` | MODIFY | Replace `CString::new` / `to_string_lossy` with handle pass-through; call `cb_rt_string_from_literal` for constants. |
| `crates/cb-backend-interp/src/value.rs` | MODIFY | `Value::String(Rc<CbStringHandle>)` or equivalent; Clone calls retain, Drop calls release. |
| `crates/cb-ir/src/types.rs` | NONE (likely) | `IrType::String` unchanged. Backend decides representation. |
| `docs/cb_runtime.md` | MODIFY | Document the ABI: `CbString*` opaque, refcount semantics, static-data convention, primitive functions. |
| `docs/cb_syntax.md` | MAYBE MODIFY | Pin down empty vs. null string semantics if §3.1 doesn't already (re-check). |

## Verification

- All existing FD-010 / FD-012 string tests still pass (`print("hello")` fixtures).
- New micro-fixtures: a string returned from `cb_rt_create_test_string()` is released exactly once when the receiving `Value` is dropped (instrumentation: refcount counter visible in tests).
- Static-data verification: a fixture that loops `for i = 1 to 1000000 : a$ = "hello" : next` allocates zero heap bytes (literal handle path).
- Refcount stress: 10k-iteration concat loop, no resident-memory growth.
- Catalog version bump (3 → 4) detected at startup; old runtimes rejected with a clear error.

## Open questions

- **Single decision blocker:** confirm Option B is the call. If anything pushes us off it, name the specific case.
- Static-data sentinel encoding: refcount = `-1` (signed) vs. high-bit-set (`UINT32_MAX`) vs. legacy `mOffset != sizeof(LStringData)`. Pick whatever maps cleanest to an LLVM constant initializer.
- Atomic vs. non-atomic refcount. CB is single-threaded today; atomic is "free" on x86 and future-proofs for an eventual threaded runtime. Recommendation: atomic.
- Empty string `""` vs. null `CbString*` — do CB programs ever observe a null string? Per §5.3 / §5.6.1, no — an uninitialized String is `""` (§3.5 default value for `String` is `""`). So `cb_rt_*` may assume non-null and avoid null checks; backend is responsible for never producing a null handle. Confirm before locking in.
- Should the three primitives (`retain`/`release`/`from_literal`) be CB-visible (callable from CB source) or backend-only? Recommend backend-only — exposed through named fields on `CbCatalog`, not the function table.
- Catalog struct shape: add a sibling `CbStringApi` substruct on `CbCatalog` for these, or inline three pointers? Recommend the substruct — it's the pattern other "primitive types with operations" will follow (memblocks, etc., eventually).

## Related

- [FD-013](FD-013_EXTENDING_RUNTIME_SUPPORT.md) — runtime expansion; blocked on this decision for any string-touching subsystem.
- [FD-012](archive/FD-012_CATALOG_CPP_TEMPLATE_DSL.md) — catalog ABI v3; this FD bumps to v4.
- [FD-011](archive/FD-011_RUNTIME_CUSTOM_TYPES.md) — opaque-handle convention; this FD generalizes "primitive type with release hook" alongside it.
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — runtime library architecture.
- [FD-010](archive/FD-010_INTERPRETER_BACKEND.md) — `Value::String(Rc<str>)` representation.
- Legacy: `../CBCompiler/Runtime/lstring.{h,cpp}` — `LString` / `CB_StringData` reference; the recommended port target.
- Legacy: `../CBCompiler/Runtime/cb_string.cpp` — string library functions; mechanical port once ABI is fixed.
- Spec: `docs/cb_syntax.md` §3.1 (UTF-8), §5.3 (no `[]` indexing), §5.6.1 (COW permitted).
