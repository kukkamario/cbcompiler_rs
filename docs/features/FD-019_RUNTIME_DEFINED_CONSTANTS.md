# FD-019: Runtime-Defined Constants

**Status:** Open
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Lets the runtime predeclare global constants (`On`, `Off`, `PI`, `cbKeyEsc`, `cbKeyA`, …) so CB programs can use the documented symbolic names instead of magic numbers.

## Problem

CoolBasic programs lean on a large set of predefined global constants supplied by
the runtime/standard library:

- Boolean-ish conveniences: `On == 1`, `Off == 0`. (`True`/`False` are **not** in
  scope — they are already language keywords / boolean literals in the lexer
  (`Kw::True`/`Kw::False`), so the runtime must not redeclare them.)
- A math constant: `PI` (Float).
- The full `cbKey*` scancode family used with `KeyDown`/`KeyUp`/`KeyHit` —
  `cbKeyEsc`, `cbKeyA` … `cbKeyZ`, `cbKey1` …, arrows, function keys, etc.
  These mirror the CB DirectInput-style scancode table already ported verbatim
  into `runtime/cb_input.cpp` (`sCBKeyMap`, indices 1..221).
- Likely more families over time (mouse buttons, draw modes, …).

The language already supports user `Const` declarations (`docs/cb_syntax.md` §4.4,
`DeclKind::Constant { value: ConstValue }` in `cb-sema`), and these fold at compile
time. But there is **no mechanism for the runtime to inject its own constants** into
the global scope. Today a program must hard-code `If KeyDown(1)` instead of
`If KeyDown(cbKeyEsc)`, and there is no `On`/`Off`.

The runtime already declares its **types** and **functions** to the compiler through
the FFI catalog (`CbCatalog` → `RuntimeCatalog`, seeded into sema's global scope by
`register_runtime_catalog` in `crates/cb-sema/src/check.rs`). Constants are the third
member of that surface and have no equivalent channel.

## Solution

Add a **constants table** to the catalog ABI, mirroring the existing types/functions
plumbing end to end. Because CB constants fold at compile time (`Const` values are
inlined during lowering), this is almost entirely frontend/catalog work — **no IR or
interpreter changes are expected**, and the LLVM backend is unaffected.

Pipeline (each stage mirrors what types/functions already do):

1. **C ABI (`runtime/cb_runtime_core.h`).** Add a `CbConstDesc` descriptor and a
   `const_count` / `consts` pair on `CbCatalog`. Bump `CB_CATALOG_VERSION` 5 → 6.
   A constant carries a name, a type tag, and a value. **Decision (Q1/Q6):** only
   `CB_TYPE_INT` and `CB_TYPE_FLOAT` are supported for now; the value is a tagged
   union `{ CbTypeTag tag; union { int64_t i; double f; } v; }`. String constants
   are deferred (no `CbString`/refcounting concerns this FD); the union can grow
   later without an ABI break beyond the version bump.

2. **Catalog assembly (`runtime/catalog.cpp`).** Provide a DSL/macro to declare
   constants in the same spirit as the `CB_FN` template DSL (FD-012), e.g.
   `CB_CONST_INT("On", 1)`, `CB_CONST_INT("Off", 0)`, `CB_CONST_FLOAT("PI", 3.14159265358979)`,
   `CB_CONST_INT("cbKeyEsc", 1)`. The `cbKey*` family
   is generated from the same X-macro table as `sCBKeyMap` to avoid drift (**Q3**).

3. **Rust mirror (`crates/cb-runtime-sys/src/lib.rs`).** Add `#[repr(C)] CbConstDesc`,
   read the new `consts` slice in `load_catalog`, bump `CB_CATALOG_VERSION` to 6,
   and emit a `Vec<RuntimeConstDesc>` (new type in `cb-ir`) on `RuntimeCatalog`.

4. **IR (`crates/cb-ir`).** Add `RuntimeConstDesc { name: String, value: ... }` and a
   `constants` field on `RuntimeCatalog`. The value carrier mirrors the Int/Float
   subset of sema's `ConstValue`, kept backend-agnostic (Bool/String variants can be
   added when those constant types ship).

5. **Sema (`crates/cb-sema/src/check.rs`).** In `register_runtime_catalog`, loop over
   `catalog.constants` and `declare` each as
   `DeclKind::Constant { value: ConstValue::… }` in the global scope (interned
   lowercase, like types/functions — CB is case-insensitive). Everything downstream
   (constant-expression folding, `Select`/`Case`, type checking, inlining at lower
   time) already handles `DeclKind::Constant` with no further changes.

6. **Conflict policy (Q2) — decided: error, and it falls out for free.** Runtime
   constants are seeded into the top scope *before* pass-1 processes user
   declarations. A user `Const On = 5` (or any collision) then reaches `try_declare`,
   finds the existing name, and emits **E0303 duplicate declaration** — exactly the
   mechanism that already guards runtime function/type names. No special handling
   needed; just add a sema test pinning E0303 on a colliding user `Const`.

### Why catalog-based rather than hard-coded in Rust

It keeps the "the runtime declares its own surface" model consistent with types and
functions, keeps the `cbKey*` constants physically next to the `sCBKeyMap` table they
must agree with, and lets future plugin DLLs (the FD-009/FD-016 plugin ABI direction)
ship their own constants without touching the compiler. The cost is one more ABI
bump and a tagged-union value carrier.

## Resolved Decisions

- **Q1 / Q6 — Value encoding & supported types.** Tagged union, **Int and Float only**
  for now: `struct CbConstDesc { const char* name; CbTypeTag tag; union { int64_t i; double f; } v; }`
  (`tag` ∈ {`CB_TYPE_INT`, `CB_TYPE_FLOAT`}). `PI` is a Float. String (and Bool)
  constants are deferred — the union grows later behind the same version field.
- **Q2 — User/runtime name collisions: error.** Achieved automatically via the
  existing seed-before-pass-1 ordering → `try_declare` → **E0303**. See Solution step 6.
- **Q4 — `Off`, `True`/`False`.** `On == 1`, `Off == 0`. `True`/`False` are **language
  keywords / boolean literals** (`Kw::True`/`Kw::False`), so they are *not* runtime
  constants and must not be registered.
- **Q5 — Initial scope.** Ship the framework + `On`/`Off` + **`PI`** + the **core**
  `cbKey*` set (see Q3). Other families (mouse buttons, draw modes, …) deferred.

- **Q3 — `cbKey*` single source of truth & key set.** Resolved on three points:

  **(a) Mechanism — one shared table.** Define a single X-macro list in a new shared
  header (e.g. `runtime/cb_keys.def`):
  ```c
  //      CB constant name   scancode   Allegro keycode
  CB_KEY( cbKeyEsc,          1,         ALLEGRO_KEY_ESCAPE )
  CB_KEY( cbKeyA,            30,        ALLEGRO_KEY_A )
  ...
  ```
  `cb_input.cpp` expands it to populate `sCBKeyMap[scancode] = allegro_key` (replacing
  the hand-written `init_key_map` body), and `catalog.cpp` expands the *same* list to
  emit `CB_CONST_INT(name, scancode)` entries. A scancode can no longer drift between
  the lookup table and the symbolic name — they are the same datum.

  **(b) Pause/NumLock fix.** The shared table uses the **real-CoolBasic / DirectInput**
  scancodes: **69 = `cbKeyNumlock` (`ALLEGRO_KEY_NUMLOCK`)**, **197 = `cbKeyPause`
  (`ALLEGRO_KEY_PAUSE`)**. This *corrects* the current `sCBKeyMap`, which has 69→Pause
  / 197→NumLock swapped (a latent bug inherited verbatim from cbEnchanted's
  `inputinterface.cpp`). Update the `docs/cb_runtime.md` scancode table to match and add
  a regression note. `70 = cbKeyScroll` (`ALLEGRO_KEY_SCROLLLOCK`) is unchanged.

  **(c) Naming & set.** Identifiers are taken **verbatim** from cbEnchanted's
  `tools/cbkeys_mapping/keys_coolbasic.cb` (a dump of the real CoolBasic names) —
  e.g. `cbKeyEsc` (not `cbKeyEscape`), `cbKeyReturn` (main Enter, 28), `cbKeyEnter`
  (numpad, 156), `cbKeyLControl`/`cbKeyRControl`, `cbKeyLAlt`/`cbKeyRAlt`, `cbKeyApps`
  (221). Names are interned case-insensitively, so source casing is free. Ship the
  **core, unambiguous** set this FD (all verified consistent with `sCBKeyMap` modulo
  the 69/197 fix):
  - Letters `cbKeyA`–`cbKeyZ`; digits `cbKey0`–`cbKey9`; function `cbKeyF1`–`cbKeyF12`.
  - Arrows `cbKeyUp`/`cbKeyDown`/`cbKeyLeft`/`cbKeyRight`.
  - Editing/whitespace `cbKeyEsc`, `cbKeySpace`, `cbKeyReturn`, `cbKeyEnter`, `cbKeyTab`,
    `cbKeyBackspace`.
  - Modifiers `cbKeyLShift`/`cbKeyRShift`, `cbKeyLControl`/`cbKeyRControl`,
    `cbKeyLAlt`/`cbKeyRAlt`, `cbKeyCapsLock`, `cbKeyLWin`/`cbKeyRWin`, `cbKeyApps`.
  - Navigation `cbKeyInsert`, `cbKeyDel`, `cbKeyHome`, `cbKeyEnd`, `cbKeyPgUp`, `cbKeyPgDown`.
  - Numpad digits `cbKeyNum0`–`cbKeyNum9`.
  - Locks/system `cbKeyNumlock`, `cbKeyScroll`, `cbKeyPause`, `cbKeyPrint`.

  **Deferred to a follow-up:** the punctuation / OEM / numpad-operator keys
  (`cbKeyMinus`, `cbKeyEquals`, the bracket/semicolon/quote/grave/backslash/comma/
  period/slash keys, `cbKeyOEM102`, and numpad `cbKeyAdd`/`cbKeySubtract`/
  `cbKeyMultiply`/`cbKeyDivide`/`cbKeyDecimal`). Reason: `keys_coolbasic.cb` uses
  non-identifier placeholder labels for several of these (`cbkey]`, `cbkey[`) and its
  scancode↔key assignments for the punctuation block disagree with `sCBKeyMap`, so the
  exact names/values need confirmation before shipping. The shared-table mechanism makes
  adding them later a one-line-per-key change.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_runtime_core.h` | MODIFY | Add `CbConstDesc`; add `const_count`/`consts` to `CbCatalog`; bump `CB_CATALOG_VERSION` 5→6 |
| `runtime/catalog.cpp` | MODIFY | `CB_CONST_INT`/`CB_CONST_FLOAT` DSL + the entries (`On`/`Off`/`PI`/`cbKey*`) |
| `runtime/cb_keys.def` | CREATE | Shared X-macro `(name, scancode, allegro_key)` table — single source of truth for `sCBKeyMap` and the `cbKey*` constants (Q3a) |
| `runtime/cb_input.cpp` | MODIFY | Expand `cb_keys.def` to build `sCBKeyMap` (replaces hand-written `init_key_map`); fixes the 69/197 Pause↔NumLock swap (Q3b) |
| `docs/cb_runtime.md` | MODIFY | Correct the scancode table: 69=NumLock, 197=Pause (Q3b) |
| `crates/cb-runtime-sys/src/lib.rs` | MODIFY | Mirror `CbConstDesc`; read `consts` slice in `load_catalog`; bump version; update catalog test |
| `crates/cb-ir/src/...` | MODIFY | `RuntimeConstDesc` + `constants` field on `RuntimeCatalog` |
| `crates/cb-sema/src/check.rs` | MODIFY | Seed runtime constants into global scope in `register_runtime_catalog` |
| `crates/cb-driver/tests/fixtures/...` | CREATE | Golden program using `KeyDown(cbKeyEsc)` / `On` / `Off` / `PI` |

## Verification

- `cargo test -p cb-runtime-sys` — catalog round-trip test asserts the new constant
  entries load with correct names/types/values (extend `load_catalog_returns_expected_entries`).
- `cargo test -p cb-sema` — a program referencing `cbKeyEsc`/`On`/`Off`/`PI`
  type-checks; using one in a `Select`/`Case` and constant expression folds correctly.
  Plus a test pinning **E0303** on a user `Const On = 5` collision (Q2).
- `cargo test -p cb-runtime-sys` / input goldens — a `KeyDown`/`KeyHit` regression that
  pins the corrected 69=NumLock / 197=Pause mapping (Q3b).
- Golden fixture: a CB program that does `If KeyDown(cbKeyEsc)` / `x = On` /
  `f# = PI` compiles and runs under the interpreter, producing the same result as the
  magic-number / literal form.
- `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` all green.
- Confirm the LLVM backend (`--features llvm`) is unaffected — constants are inlined
  before IR, so codegen never sees them.

## Related

- `docs/cb_syntax.md` §4.4 (Constants), §4.1–4.3 (Dim/Global)
- `docs/cb_runtime.md` — runtime surface; `cbKey*` scancode documentation
- FD-013 (Extending Runtime Support — `cb_input.cpp`, `sCBKeyMap` scancode table)
- FD-012 (Catalog DSL via C++ templates — the `CB_FN` pattern to mirror with `CB_CONST_*`)
- FD-011 (Runtime Custom Types — the types-table plumbing this mirrors)
- FD-016 (Runtime Core/Functionality Split — plugin ABI direction that motivates catalog-based constants)
- cbEnchanted reference runtime at `../cbEnchanted` (authoritative key constant values)
