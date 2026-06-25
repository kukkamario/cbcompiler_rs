# FD-046: Core-Runtime String↔Number Conversion Primitives

**Status:** Complete
**Completed:** 2026-06-26
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** Gives the interpreter and the future LLVM/AOT backend **one shared implementation** of every conversion that crosses the `String` type, so the two backends cannot silently diverge on `Str()`/`Int(s$)`/`Float(s$)` (especially float→string formatting). A prerequisite that de-risks the LLVM backend, fully verifiable on the interp path with **zero LLVM dependency**.

## Problem

Today the interpreter performs all number↔string conversion in Rust (`crates/cb-backend-interp/src/value.rs`):

- `as_cb_string` (`value.rs:51-68`) — number→String via Rust's `v.to_string()` (Int/Long/Float/Byte/Short).
- `to_i64` (`value.rs:78-94`) — String→int via `parse_leading_int` (lenient: `"3x"` → 3, trims leading whitespace, optional sign, saturates).
- `to_f64` (`value.rs:101-120`) — String→float via a **strict** full parse (`"3x"` → 0.0).

There is **no `cb_rt_*` equivalent in the C++ runtime** — a grep of `runtime/` finds no number→string formatter. These conversions are invoked by the IR `Convert`/`ConvertExplicit` opcodes: `Int()`/`Integer()`, `Float()`, `Str()` all lower to `ConvertExplicit` (`crates/cb-sema/src/lower.rs:1158-1187`), plus sema inserts implicit `Convert` coercions.

This is the **only silent divergence** in the LLVM-backend readiness picture. Every other interp/native mismatch (traps, OOB, integer overflow) is either loud or explicitly spec-permitted (`docs/cb_syntax.md:482`). But a float that formats `3.14` in the interpreter and `3.140000` in a native build is a wrong answer with **nothing to catch it**. A native `.exe` cannot call back into Rust, so the single shared home for this logic must be the C++ core runtime.

## Solution

Move **only the conversions that cross the `String` type** into the C++ core runtime as **bare exported symbols** (mirroring the existing `CbStringApi` precedent — `cb_rt_string_retain/release/from_literal/len/data/concat`), and have **both** backends call them.

### The three-way boundary (load-bearing — confirmed with the user)

| Conversion | Home | Invoked via |
|---|---|---|
| **Numeric ↔ numeric** (`Int↔Float↔Long↔Byte↔Short`, `Float→Int` round-half-away-from-zero, byte/short narrowing-modulo) | **Each backend, inline** — no shared helper | `Convert`/`ConvertExplicit` opcode (interp: `convert_value` already; LLVM: emit inline) |
| **String ↔ number** (number→String; String→Int/Float; incl. sema's implicit `String` coercions) | **Core runtime, bare exported symbols** (this FD) | `Convert`/`ConvertExplicit` opcode |
| **`Hex$` / `Bin$` / `Chr$` / `Asc`** | **Ordinary catalog runtime functions** (separate future FD — not yet wired) | normal `Call` through the catalog |

**Why `Hex$`/`Bin$`/`Chr$`/`Asc` are NOT in this FD even though they touch `String`:** the dividing line is *how the IR invokes the conversion*, not the type signature. Those are user-facing library functions invoked by a normal `Call`; the **existing** `CbString*` ABI already marshals `String` params/returns for them (the same path graphics/file functions use), so they need no special core primitive. The bare symbols here exist solely to service the `Convert`/`ConvertExplicit` opcodes.

### Bare symbols (proposed minimal set)

In a new Allegro-free TU `runtime/cb_convert.cpp` (+ header), **outside** the `CB_NO_ALLEGRO` guard so it ships in both the SDK-free and full builds and is headless gtest-able — same structure as the Memblock/File subsystems, but exported as **bare symbols, NOT catalog `CB_FN` rows** (stays out of the catalog, the FD-045 drift guard, and `CB_CATALOG_VERSION`):

- `cb_rt_int_to_string(int32_t) -> CbString*` — Byte/Short widen to Int first (lossless, matches interp).
- `cb_rt_long_to_string(int64_t) -> CbString*`
- `cb_rt_float_to_string(double) -> CbString*` — **the one that matters** (see open item).
- `cb_rt_string_to_long(CbString*) -> int64_t` — lenient leading-int (`parse_leading_int` semantics); backend narrows for Int/Short/Byte targets, matching the interp's "parse to i64 then narrow" path.
- `cb_rt_string_to_float(CbString*) -> double` — strict full parse (`to_f64` semantics).

Returned strings follow the established ownership rule (refcount 1, caller owns — see `string_handle.rs` / `ffi.rs`).

### Backend wiring

- **Interp:** keep `to_i64`/`to_f64`/`as_cb_string` as thin Rust wrappers that **call the new symbols**, preserving their "single source of truth" role for the FFI marshaller and array-index coercion. This is a **behavior change**, not a pure addition — float formatting now follows the C++ helper (see open item + Verification).
- **LLVM (future):** lowering a `ConvertExplicit{target: Int}` dispatches on the **source register's type** — numeric source → inline cast; `String` source → `call cb_rt_string_to_long` + narrow. This is a concrete consumer of the **register-type-inference pass** identified in the LLVM-backend readiness analysis: knowing the source type is exactly what selects inline-vs-call here.

### Canonical behaviour (decoded from real CoolBasic — run #1)

Captured empirically by running a throwaway probe program on the **original CoolBasic** runtime (the probe + raw output are not retained in-tree; the decoded behaviour below is the record). Decoded:

**`Int → String`:** plain base-10, `-` sign, **no leading space** (not classic-BASIC `STR$`). Matches the current interpreter.

**`Float → String` (CONFIRMED spec — the formatter to implement):**
- **6 significant digits.** `1234567.0 → "1234570.0"`, `12345678.0 → "12345700.0"`, `1.0/3.0 → "0.333333"`, `10.0/3.0 → "3.33333"`.
- **Fixed-point iff** decimal exponent `E = floor(log10(|x|)) ∈ [-3, 7]`; **scientific otherwise.** Boundaries confirmed: `1e7 → "10000000.0"` (fixed) vs `1e8 → "1.e+008"` (sci); `0.001` (fixed) vs `0.0001 → "1.e-004"` (sci).
- **Fixed:** strip trailing fractional zeros but **always keep ≥1** → `4.0 → "4.0"`, `100.0 → "100.0"` (never `"4"`/`"100.000"`).
- **Scientific:** `m.e±EEE` — mantissa trailing-zeros stripped with the `.` kept, lowercase `e`, **always-signed 3-digit** exponent (`1.e+008`, `1.e-004`, `1.e+020`). *(Multi-digit mantissa confirmed in v2: `123456700.0 → "1.23457e+008"`, `0.0001234 → "1.234e-004"`.)*
- **Decimal separator is `.`** (not Finnish `,`). **`-0.0 → "0.0"`** (no minus). Implicit `"" + n` concat uses the **same** formatter (confirmed §F).
- **Note (out of scope here):** CB Float is **32-bit**; cbcompiler_rs stores f64/double. At 6 sig figs the two agree for all probed values, but the f32-vs-f64 *value* divergence is a separate Float-width decision, not this FD.

**`String → Float` (CONFIRMED — interp fix required):** lenient `strtod`-style prefix parse — skip leading whitespace, optional sign, parse a float **including exponent**, **stop at first invalid char**, `0.0` on no valid prefix. `"22yo"→22.0`, `"3.14xyz"→3.14`, `"1.5e2"→150.0`, `".5"→0.5`, `"5."→5.0`, `"1.2.3"→1.2`, `"+.25"→0.25`. The interpreter's strict full-parse (`value.rs:101-120 to_f64`, `"22yo"→0.0`) is **wrong** and must change to match. `cb_rt_string_to_float` implements this.

**`String → Int` (CONFIRMED partial):** `"+7"→7`, `"   42"→42`, `"42   "→42`, `"007"→7`, `"3x"→3`, `"Hello"/""→0`, `"- 6"→0`, `"0x1F"→0`, `"1e3"→1` (stops at `e` — unlike `Float`). **But** the fractional rule is unresolved (see open items).

### Resolved by v2 — quirks characterised + final spec

A second probe run (dense rounding/parse sweep on the original CoolBasic) confirmed the format spec above and closed every open question.

**Scientific mantissa (confirms the format above).** 6 sig figs, trailing zeros stripped, `.` kept. 6-sig-fig rounding precedes the fixed/sci decision: `99999990.0` rounds to `1e8` (E 7→8) → `"1.e+008"`.

**Implementation hint for the formatter:** `snprintf(buf, "%.5e", x)` yields exactly 6 sig figs + exponent; parse out the mantissa digits and `E`, branch fixed/sci on `E ∈ [-3,7]`, apply the strip-trailing-zeros / keep-`.0` / `1.`-mantissa / 3-digit-exp fixups. Avoids `log10` edge cases. **Not Ryū / not shortest-round-trip.**

**`String → Float` = C `strtod`** (confirms lenient prefix parse): `"1e3"→1000`, `"1.5e-1"→0.15`, dangling `"1.5e"→1.5`, `"1,5"→1.0` (comma is not a separator), `"--5"→0.0`. The interpreter's strict `to_f64` (`value.rs:101-120`) **must change** to this.

**`String → Int` is a CB quirk → diverge cleanly.** Real CB truncates toward zero **except an exact fractional `.5` rounds up, positives only** (`"22.5"→23`, `"23.5"→24`; `"22.7"→22`, `"-22.5"→-22`) and does **not** parse exponents (`"1e3"→1`, `"2.5e1"→2`). **Decision (recommended):** keep the interpreter's `parse_leading_int` (leading-integer parse, stop at the first non-digit incl. `.`, saturating) for `cb_rt_string_to_long`. It matches CB on **everything except the exact-`.5`-positive quirk**, which is deliberately not replicated — consistent with the runtime FDs' treatment of CB bugs/UB. **DECIDED (2026-06-26): clean truncation.**

**`Int(Float)` rounding — recorded, NOT this FD (numeric-cast bucket, backend-inline).** Real CB rounds to nearest with **asymmetric halves: positives half-up (`2.5→3, 4.5→5`), negatives half-to-even (`-1.5→-2, -2.5→-2, -3.5→-4, -4.5→-4`)**. Non-halves are normal nearest. The interpreter uses `f64::round` (half-away-from-zero) — agrees on positives, diverges on negative halves. When the `Convert`/`ConvertExplicit` numeric-cast lowering is specced for the LLVM backend, pin one clean rule (recommend **half-away-from-zero** = keep interp, or **half-up**), documented as a divergence — the asymmetric CB behaviour is not worth bug-for-bug replication. Linked here only because the probe surfaced it.

### Decisions (resolved)

1. **`String → Int` semantics** (core symbol `cb_rt_string_to_long`) — **clean leading-integer truncation** (decided 2026-06-26). The interpreter's existing `parse_leading_int` behaviour stands; CB's exact-`.5`-rounds-up-positives quirk and exponent-ignoring are documented as deliberate divergences. A test asserts the one intentional difference (`"22.5" → 22`, not CB's `23`).

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `runtime/cb_convert.cpp` | CREATE | Bare `cb_rt_*` string↔number conversion symbols (Allegro-free TU, outside `CB_NO_ALLEGRO`) |
| `runtime/cb_convert.h` | CREATE | Declarations for the bare symbols (headless-testable pure helpers where possible) |
| `runtime/CMakeLists.txt` / `crates/cb-runtime-sys/build.rs` | MODIFY | Add the new TU to both the full and SDK-free builds |
| `crates/cb-backend-interp/src/value.rs` | MODIFY | `as_cb_string`/`to_i64`/`to_f64` become thin wrappers calling the new symbols |
| `runtime/` gtest suite | MODIFY | Pin the conversion semantics (6-sig-fig float format, lenient `strtod` float parse, leading-int parse) |
| interp conversion snapshots | MODIFY | Re-baseline to the chosen shared formatter (float-format change) |

## Verification

- Native `ctest` cases pinning: `int/long → string`, the `float → string` format (integral `4.0→"4.0"`, fractional `3.14`, 6-sig-fig rounding `1234567.0→"1234570.0"`, fixed/sci boundary `1e7`/`1e8`, sci mantissa `1.23457e+008`, `-0.0→"0.0"`), leading-int `string → long` (`"  -3x"` → -3, `"22.5"` → 22, no-digits → 0, saturation), lenient `string → float` (`"3x"` → 3.0, `"22yo"` → 22.0, `"1.5e2"` → 150.0, junk → 0.0).
- `cargo test --workspace` green, including re-baselined interp conversion snapshots — the interp must now produce **byte-identical** output to the C++ helper.
- `cargo test -p cb-backend-interp` covering `Str()`/`Int(s$)`/`Float(s$)` and the implicit-coercion sites.
- Build both configs (full Allegro + SDK-free `CB_RUNTIME_FORCE_SDK_FREE=1`) — the TU must be present in both.
- `clippy --workspace --all-targets -D warnings` + `fmt --all --check` clean.
- Confirm the new symbols are **absent from the catalog** (no `CB_FN` row; `cb-runtime-sys` catalog-content asserts unchanged, no `CB_CATALOG_VERSION` bump).

## Verification Result (2026-06-26)

Implemented in `bf262a3` and verified green:

- **`cargo test --workspace`** — all suites pass, 0 failures. The interp ran against the **full CMake-built** `cb_convert.cpp` (linked via `cb_runtime_core`), so the re-baselined interp/driver snapshots are confirmed byte-identical to the C++ formatter.
- **New interp conversion tests** (`str_of_float_uses_cb_six_sig_fig_format`, `float_of_string_is_lenient_prefix_parse`, `int_of_string_takes_leading_integer`) — pass; `Str()`/`Int(s$)`/`Float(s$)` route through the shared symbols.
- **Native gtest** (`ctest -R Convert`) — 9/9 pass, pinning the 6-sig-fig float format, lenient `strtod` float parse, and leading-int parse.
- **SDK-free build** (`CB_RUNTIME_FORCE_SDK_FREE=1`) — compiles `cb_convert.cpp` and passes; the TU is present in both the full and SDK-free builds.
- **`clippy --workspace --all-targets -D warnings`** exit 0, **`fmt --all --check`** clean.
- **Catalog unchanged** — no `catalog.cpp`/`cb_runtime_func.h` change, no `CB_CATALOG_VERSION` bump; the bare symbols stayed out of the catalog/drift-guard machinery as designed.

## Related

- **Reference oracle:** the canonical `Str`/`Int`/`Float` behaviour was decoded by running one-off probes on the **original CoolBasic** runtime; the decoded values inline above are the source of truth for the gtest/snapshot expectations (probes not retained in-tree).
- **Prerequisite for / surfaced by** the LLVM-backend readiness `/fd-deep` analysis (the sole *silent* interp/native divergence). Pairs with the future LLVM-backend FD and its **register-type-inference pass** (which selects inline-cast vs. string-conversion `call`).
- [[FD-045]] — Catalog Metadata Decoupling: established the metadata-vs-binding seam; this FD deliberately stays **out** of the catalog/drift-guard machinery by using bare symbols.
- Precedent: the `CbStringApi` bare-symbol string primitives (FD for the string runtime) — same exported-symbol pattern, not catalog rows.
- Separate future work: `Hex$`/`Bin$`/`Chr$`/`Asc` as ordinary catalog runtime functions.
- `docs/cb_syntax.md` (§ conversion semantics; line ~482 on backend divergence latitude).
