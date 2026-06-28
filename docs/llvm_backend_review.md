# LLVM Backend Review — FD-049 (IR → LLVM Lowering)

**Date:** 2026-06-28
**Branch:** `claude/fd-049-ir-to-llvm-lowering`
**Scope reviewed:** the full FD-049 diff vs. `master` (~4,800 insertions) — `crates/cb-backend-llvm/src/codegen/{mod,types,regtypes,runtime,func}.rs`, `crates/cb-backend-llvm/src/{emit,lib}.rs`, the C/C++ runtime helpers (`runtime/cb_{array,type,string,standalone}.cpp` + headers), the differential harness (`crates/cb-driver/tests/diff_llvm.rs`) and its fixtures, plus the CI/build plumbing.

## Method

This review was run as a two-phase multi-agent audit:

- **Phase 1 — 8 parallel review agents**, each owning a distinct dimension: (1) type mapping / numeric conversions / runtime-call ABI, (2) string refcount discipline, (3) arrays, (4) type instances & the linked list, (5) value structs, (6) function pointers + control flow / terminators / SSA register mapping, (7) program entry / runtime lifecycle / C-runtime memory safety, (8) build / CI / harness / fixture coverage.
- **Phase 2 — 4 independent verifiers** that re-checked every finding against the actual code, rejected incorrect findings (including those that turned out to be *documented, intentional* scoped limitations of FD-049), and de-duplicated.

The governing principle throughout: **the interpreter (`cb-backend-interp`) is the reference oracle.** Any behavioral divergence between the LLVM backend and the interpreter is, by definition, an LLVM-backend bug.

**Outcome:** 19 raw findings → **13 confirmed**, **2 duplicates**, **4 rejected**.

---

## Summary

| ID | Area | Title | Severity | Status |
|----|------|-------|----------|--------|
| F4 | refcount | Returning a value-struct with a `String` field is a use-after-free | **Critical** | Confirmed |
| F10 | correctness | `BranchIf` with a `Float` condition panics in codegen | **High** | Confirmed |
| F2 | abi | Shift width uses RHS `Long`-ness — `Int << Long` shifts in 64-bit | Medium | Confirmed |
| F15 | coverage | `Redim` grow/shrink, multi-dim `Redim`, and `RedimGlobal` untested | Medium | Confirmed |
| F16 | coverage | No coverage of `Byte`/`Short` arithmetic wraparound | Medium | Confirmed |
| F17 | coverage | Array-of-struct with `String` fields untested (combined refcount path) | Medium | Confirmed |
| F3 | correctness | Null function pointer compared to `Null` diverges from the interpreter | Low | Confirmed |
| F5 | correctness | Fall-through `String` return yields a null `CbString*`, not the empty sentinel | Low | Confirmed |
| F8 | missing-edge-case | `New <Type>` leaves nested value-struct `String` sub-fields null | Low | Confirmed |
| F12 | correctness | Native exe writes `Print` through Windows text-mode stdout (CR+LF divergence) | Low | Confirmed |
| F14 | coverage | `Previous()` type-list walk has no differential fixture | Low | Confirmed |
| F18 | quality | Differential harness ignores stderr and treats signal-kill exit codes as equal | Low | Confirmed |
| F19 | coverage | No coverage of empty arrays and negative array indices | Low | Confirmed |
| F9 | refcount | Returning a value-struct with `String` fields = UAF | (Critical) | **Duplicate → F4** |
| F13 | coverage | No fixture for integer division/modulo by zero | (High) | **Duplicate → F1** |
| F1 | memory-safety | Integer `Div`/`Mod` emit raw `sdiv`/`srem` (UB on `/0` and `INT_MIN/-1`) | (High) | **Rejected — documented Phase-4 work** |
| F6 | quality | `cb_rt_array_new` lacks a negative-rank / null-dims guard | (Info) | **Rejected — unreachable** |
| F7 | refcount | `String` sub-fields of value-struct array elements default to null | (Info) | **Rejected — documented limitation** |
| F11 | quality | Cross-block register availability relies on def-before-use block order | (Low) | **Rejected — not triggerable** |

---

## Confirmed findings

### F4 — [CRITICAL] Returning a value-struct with a `String` field is a use-after-free

- **Category:** refcount · **Confidence:** high
- **Location:** `crates/cb-backend-llvm/src/codegen/func.rs:1480-1500` (Return), with `func.rs:488-496` (`load_slot`) and `func.rs:1520-1546` (`release_string_locals`)
- *(Independently re-discovered as **F9** by the value-structs agent — same bug, mechanism, and example.)*

**What's wrong.** A function that returns a value-`Struct` containing a `String` field releases the struct local's `String` sub-fields at `Return` and *then* returns the loaded aggregate as a **borrowed view** with no retain. The caller's binding subsequently retains the already-freed `String` fields — a use-after-free / double-free. The interpreter, by contrast, deep-clones (retains) the returned struct, so its strings stay alive independently of the callee frame.

**Evidence.** The Phase-3b borrowed-view model makes a struct *slot* own +1 per `String` sub-field while a struct *value in a register* is borrowed: `load_slot` retains only for `IrType::String`, not `StructVal` (`func.rs:488-496`, the raw-`v` "borrowed view" arm), and `store_slot_value` retains the incoming aggregate's strings on a store (`func.rs:523-531`). At `Terminator::Return { value: Some(r) }` the code calls `release_string_locals()` **first** — which for a `StructVal` local loads the aggregate and runs `release_struct_strings`, dropping each `String` field's refcount (potentially to 0 → `free`) — and *then* does `let v = self.regs[r]; build_return(&v)` with **no retain** on `v`.

Trace `Function MakeP() As Person / Dim p As Person / p.name = "Alice" / Return p`:
1. `StorePlace p.name = "Alice"` → slot owns `"Alice"` at refcount 1.
2. `r = LoadLocal p` → borrowed aggregate holding the `"Alice"` pointer (no retain).
3. `release_string_locals` → releases `p.name` (1 → 0) → `std::free` (`cb_string.cpp:127-132`).
4. `build_return(r)` → returns the aggregate with a dangling `"Alice"` pointer.
5. Caller `Dim q As Person = MakeP()` → `store_slot_value` → `retain_struct_strings` → `cb_rt_string_retain` on freed memory (UAF).

Interpreter oracle (`interp.rs:412-437`): `ret_val = frame.registers[r].clone()` deep-clones the `StructObj`, each `CbStringHandle` clone = retain; the popped frame's locals then drop. The returned struct independently owns its strings — correct. The `String`-return path only works because `LoadLocal` of a `String` retains (`func.rs:488-490`); the struct path has no such retain, so the symmetry is broken. `fn_type`/`basic_type` already accept a `StructVal` return (`types.rs:40-50,76-79`) and the syntax permits `Function f() As <Struct>`, so this is a compilable, reachable program. **No fixture returns a struct with a `String` field**, so the diff harness never exercises it; on typical allocators the freed header often survives, masking the UAF while leaving it latent.

**Suggested fix.** Before `release_string_locals()` in the `Return { value: Some(r) }` path, if the returned value is a `StructVal` (recursively for nested structs) retain its `String` sub-fields via `retain_struct_strings`, so the returned aggregate owns +1 and the local release cannot free it (this alone converts the UAF into a bounded, FD-acceptable leak). For a fully balanced fix, adopt the symmetric owned-model for struct returns: treat a `Call`/`CallIndirect` result of `StructVal` type as already-owned, have the consuming `store_slot_value` move it in without an extra retain, and have `regtypes` release unconsumed struct temps — mirroring the existing `String` discipline. Add a `struct_return_string.cb` fixture to gate it.

---

### F10 — [HIGH] `BranchIf` with a `Float` condition panics in LLVM codegen

- **Category:** correctness · **Confidence:** high
- **Location:** `crates/cb-backend-llvm/src/codegen/func.rs:1463-1479` (the `let c = self.ival(*cond)?;` at line 1468)

**What's wrong.** CoolBasic allows any *numeric* expression as a condition (`If f#`, `While f#`, `Repeat … Until f#`), including a bare `Float`. Sema's `check_condition` only verifies the condition is numeric and inserts **no** coercion, so the raw `Float` register flows into `Terminator::BranchIf`. The interpreter handles it via `is_truthy` (`Float(v) => *v != 0.0`); the LLVM `BranchIf` lowering unconditionally calls `self.ival(*cond)?`, which does `into_int_value()` on the operand. For a `FloatValue` this **panics** ("Found FloatValue but expected the IntValue variant") — a hard codegen crash on a program the interpreter runs correctly.

**Evidence.** Sema `check.rs:2085-2095` only errors when `!cty.is_numeric()` (Float passes), no coercion. `lower_if` (`lower.rs:1652`), `lower_while` (`:1753`), `lower_repeat_while` (`:1797`) feed the raw expr reg into `BranchIf { cond }`, so for `Dim f# : f#=1.5 : If f# Then Print "y"` the cond reg is `Float`-typed. Interpreter (`interp.rs:396-411` → `value.rs:39`) prints `"y"`. LLVM (`func.rs:1468` → `ival` → `regs[reg].into_int_value()`) panics. No fixture covers a bare-float condition; existing fixtures use comparison/`And`-based (Int-result) conditions.

**Suggested fix.** In `lower_terminator`'s `BranchIf` arm, dispatch on the condition register's IR type (via `self.info.type_of(*cond)`), mirroring `is_truthy`: if `Float`, compute truthiness with `build_float_compare(FloatPredicate::UNE, fval, f64 0.0)`; otherwise keep the integer `build_int_compare(NE, ival, 0)` path. Add a `float_condition` diff fixture (`If f#`, `While f#`).

---

### F2 — [MEDIUM] Shift width uses RHS `Long`-ness — `Int/Byte/Short << Long` shifts in 64-bit

- **Category:** abi · **Confidence:** high
- **Location:** `crates/cb-backend-llvm/src/codegen/func.rs:1155-1159`

**What's wrong.** For integer ops the lowerer picks the operation width as 64 if *either* operand is `Long`. Shifts share this code, but shift width must depend only on the LHS (the value being shifted); sema does not coerce shift operands. When the shifted value is `Byte`/`Short`/`Int` but the shift count is `Long`, the LLVM backend computes a 64-bit shift (count masked `&63`, result `i64`) while the interpreter computes a 32-bit shift (count masked `&31`, result `Int`). The result is also `i64` while `regtypes` types the result reg as `Int`, so a downstream use mismatches widths (likely a hard LLVM type error at the next consumer/store).

**Evidence.** `func.rs:1155-1159`: `let width = if matches!(lty, IrType::Long) || matches!(rty, IrType::Long) { 64 } else { 32 };`, then `Shl/Shr/Sar` use `mask_shift(r, width)`. Interpreter (`interp.rs:1066-1076`): `let wide = matches!(lhs, Value::Long(_));` — width is decided by the LHS alone, count via `rhs.to_i64()`; `int_binop` (`interp.rs:1208-1219`) for non-wide does `(a as i32).wrapping_shl((b as u32) & 31)`. So `Int << Long` with count 33: interp masks `&31` → shift by 1, 32-bit, `Int`; LLVM masks `&63` → shift by 33, 64-bit, `i64`. `regtypes::binop_result` (`regtypes.rs:142-155`) routes shifts through the `_` arm typing the result from the LHS (`Int`), creating a width inconsistency with the `i64` value. Sema explicitly skips operand-coercion for `Shl/Shr/Sar` (`check.rs:1005`).

**Suggested fix.** Compute the shift width from the LHS type only (`Long` → 64 else 32) for `Shl/Shr/Sar`, independent of the RHS; extend/truncate the shift count to that LHS width before masking. Keep the existing `lty||rty` rule for genuinely same-typed arithmetic/bitwise/compare ops (sema coerces those, so operands are already equal).

---

### F15 — [MEDIUM] `Redim` grow/shrink, multi-dim `Redim`, and `RedimGlobal` are untested

- **Category:** coverage-gap · **Confidence:** medium
- **Location:** `crates/cb-driver/tests/fixtures/programs/array_redim.cb:1-8`; oracle `interp.rs:879-899`

**What's wrong.** `array_redim.cb` only redims a 1-D local array to the **same** size (`[3]→[3]`). There is no fixture that grows, shrinks, or multi-dim-redims an array, and `InstKind::RedimGlobal` (`interp.rs:890`, lowered at `func.rs:390`) has **no** differential fixture at all — its distinct path (store the new handle into a global slot, release the old global handle) is completely uncovered.

**Suggested fix.** Add fixtures redimming to a larger and a smaller size (asserting zero-fill / no stale data), a multi-dim `Redim`, and at least one `RedimGlobal` (a top-level array mutated inside a function, so it lowers to a global).

---

### F16 — [MEDIUM] No differential coverage of `Byte`/`Short` scalar arithmetic wraparound

- **Category:** coverage-gap · **Confidence:** medium
- **Location:** no fixture; LLVM binop `crates/cb-backend-llvm/src/codegen/func.rs:1155-1168`

**What's wrong.** No fixture declares a `Byte`/`Short` local and overflows it via native arithmetic. The LLVM binop computes at width 32/64 (`func.rs:1155-1159`) and relies on a separate truncation on store to a narrow slot; `Convert`/`ext_int` (`func.rs:1593-1624`) applies **unsigned** zero-extension for `Byte`/`Short`. That zero-extension + Convert-truncation path is real code with zero differential coverage; a sign/zero-extension mismatch on narrow-integer semantics would be invisible. (Verifier note: the finding's "interp wraps to the declared type" is slightly imprecise — interp's `wrap` collapses to `Int`/`Long`, matching `regtypes` — but the coverage gap is accurate.)

**Suggested fix.** Add a `byte_short_overflow.cb` declaring `Byte`/`Short` locals, performing additions/multiplications that overflow 8/16 bits, and printing the wrapped results, asserting `interp == llvm`.

---

### F17 — [MEDIUM] Array-of-struct with `String` fields is untested (combined refcount path)

- **Category:** refcount / coverage-gap · **Confidence:** medium
- **Location:** `crates/cb-driver/tests/fixtures/programs/struct_array_elem.cb`, `struct_string_field.cb`

**What's wrong.** `struct_array_elem.cb` uses a struct with only an `Int` field; `struct_string_field.cb` tests `String`-field refcounting only in a plain (non-array) local. No fixture combines them: an array whose elements are value structs containing a `String` field. That combination stresses three refcount-heavy paths at once — per-element `String` slot init (empty sentinel), assignment (release-old / retain-new) through an `Index`+`Field` `StorePlace`, and per-element `String` release on array teardown — which is exactly where the struct refcount fragility shown in F4/F9 is most likely to surface. (Distinct from the rejected F7, which is only a null-init behavior note.)

**Suggested fix.** Add a fixture: `Struct Item : Field name As String : Field n As Int` plus `Dim arr As Item[] = New Item[3]`; assign `String` fields per element, copy elements, reassign, and print, asserting `interp == llvm` (and run under a leak check if available).

---

### F3 — [LOW] Null function pointer compared to `Null` diverges from the interpreter

- **Category:** correctness · **Confidence:** low (reachability confirmed by verifier)
- **Location:** `crates/cb-backend-llvm/src/codegen/func.rs:1112-1143`

**What's wrong.** Reference-equality lowering compares pointer-class operands (including `FnPtr`) by pointer identity via `ptrtoint`, so a null function pointer compared with `Null` yields `1` (true). The interpreter represents a default/unassigned function pointer as `Value::FnPtr(None)` (not `Value::Null`); `(FnPtr(None), Null)` hits the generic `(_, Null)` arm → `Eq = 0`. So `f = Null` on an unassigned fn-ptr local gives `1` under LLVM but `0` under the interpreter.

**Evidence.** LLVM `func.rs:1112-1143` treats `FnPtr` as `is_ref`, `ptrtoint` both null pointers → equal → 1. Interp `interp.rs:1125-1134`: `(_, Value::Null) => Eq => Int(0)`. Default fn-ptr is `Value::FnPtr(None)` (`value.rs:176`). The verifier confirmed reachability: `f = Null` typechecks as reference equality, the `Null` operand stays `Value::Null` through `convert_value` (`interp.rs:1346-1359`), so `Print (f = Null)` yields interp `0` vs LLVM `1`.

**Note.** The interpreter is arguably the *semantically wrong* side here (CB-correct is: a null/unassigned fn-ptr **equals** `Null`). But since the interpreter is the oracle, the LLVM result is the reportable divergence. Resolve the canonical semantics first; the likely fix is to make the interpreter treat `Value::FnPtr(None)` as null in the `Eq`/`NotEq`-against-`Null` arms (or normalize default fn-ptrs to `Value::Null`), then add a differential fixture.

---

### F5 — [LOW] Fall-through `String` return yields a null `CbString*` instead of the empty sentinel

- **Category:** correctness · **Confidence:** medium
- **Location:** `crates/cb-backend-llvm/src/codegen/func.rs:1487-1497` (`Return None` path), `func.rs:204-219` (`default_value` String)

**What's wrong.** When a value-returning function reaches an implicit fall-through `Return { value: None }`, the backend returns `default_value(return_type)`. For a `String` return type this is a **null pointer**, not the immortal empty sentinel — violating the runtime's never-null `CbString*` invariant and diverging from the interpreter, which yields `Value::Void` (coerced to the empty string downstream). The code comment claims this block is "unreachable", but sema does not enforce it: it emits a synthetic `Return { value: None }` for any unterminated body (`lower.rs:580-586`), and the only missing-return diagnostic (E0315) fires solely on an *explicit* valueless `Return` (`check.rs:2371-2380`) — a `String` function with no `Return` statement at all compiles, making the synthetic null-return reachable.

**Evidence.** Masked in practice because runtime string primitives null-check, but storing the null into a `String` slot leaves that slot null rather than the sentinel — a real latent invariant violation and a factually-wrong "unreachable" comment. This is **not** one of the FD's documented simplifications (the documented one is String-*global* null-init).

**Suggested fix.** In the `Return { value: None }` fall-through for a value-returning function, special-case `IrType::String` (and `String` sub-fields of a `StructVal` return) to materialize the empty sentinel via `empty_string()` instead of a null pointer, matching the interpreter's Void→empty coercion. Correct the "unreachable" comment.

---

### F8 — [LOW] `New <Type>` leaves nested value-struct `String` sub-fields uninitialized

- **Category:** missing-edge-case · **Confidence:** medium (severity downgraded medium→low by verifier)
- **Location:** `crates/cb-backend-llvm/src/codegen/func.rs:809-832` (`build_new_type`, loop at 820-830)

**What's wrong.** When a user `Type` has a field whose type is a value `Struct` (`StructVal`) containing `String` sub-fields, `build_new_type` initializes only direct `IrType::String` fields to the empty sentinel and leaves all nested-struct `String` sub-fields calloc-zero (null `CbString*`). The interpreter initializes them to the empty sentinel. This contradicts `build_new_type`'s own comment that the invariant forbids a null `CbString*`, and is inconsistent with value-struct *local* slots, which call `init_struct_strings` (`func.rs:155-157`).

**Evidence.** Interp `NewType` (`interp.rs:610-627`) maps each field through `default_value`; `default_value` for `StructVal` (`value.rs:156-166`) recurses, giving every nested `String` sub-field `CbStringHandle::empty`. LLVM `build_new_type` (`func.rs:820-830`) loops with `if matches!(fty, IrType::String)` only — a `StructVal` field is skipped, so `cb_rt_type_new`'s calloc leaves its `String` sub-fields null. The recursive helper `init_struct_strings` (`func.rs:1005-1031`) exists and is used for locals but is **not** called from `build_new_type`. This Type-node-with-value-struct-field case is **not** in the FD's documented limitation set (which covers String globals, struct globals, and struct *array* elements only), so it is a genuine undocumented missing edge case. Currently masked (all string primitives null-check), so no stdout/exit-code-observable divergence — hence low severity — but a latent never-null-invariant violation.

**Suggested fix.** In `build_new_type`, extend the field loop: for `IrType::StructVal(sub)`, GEP element `5 + i` and call `self.init_struct_strings(fptr, *sub)?`, mirroring `func.rs:155-157`.

---

### F12 — [LOW] Native exe writes `Print` through Windows text-mode stdout (CR+LF divergence)

- **Category:** correctness · **Confidence:** medium
- **Location:** `runtime/cb_standalone.cpp:61-81` (entry/lifecycle); `runtime/catalog.cpp:71-79` (`cb_rt_print`)

**What's wrong.** The AOT executable's stdout is left in default Windows text mode, so `cb_rt_print` translates every LF (`0x0A`) it writes into CRLF and passes through any pre-existing CR verbatim. The interpreter writes raw bytes to a Rust `std::io::stdout()` sink (no translation). The harness papers over the common single-LF case with `\r\n→\n` normalization, but a printed string that itself contains an adjacent CR+LF pair (e.g. `Chr$(13)+Chr$(10)`, or text read from a CRLF file) produces different normalized stdout on Windows.

**Evidence.** `cb_rt_print` does `fwrite(...)+putchar('\n')`; grep for `_setmode`/`_O_BINARY`/`freopen`/`setvbuf` across `runtime/` finds no binary-mode switch. Tracing an embedded `{0x0D,0x0A}`: interp → `"\r\n"` → `normalise` → `"\n"`; native (text mode) → `"\r\r\n"` → `normalise` → `"\r\n"` — a real mismatch.

**Suggested fix.** In `cb_rt_standalone_run` (before invoking `user_main`), on `_WIN32` call `_setmode(_fileno(stdout), _O_BINARY)` so the native exe emits the same raw bytes as the interpreter (this also makes the harness `\r\n` normalization unnecessary, though harmless to keep).

---

### F14 — [LOW] `Previous()` type-list walk has no end-to-end differential fixture

- **Category:** coverage-gap · **Confidence:** medium (severity downgraded medium→low by verifier)
- **Location:** no fixture; oracle `interp.rs:760`; lowering `func.rs:431-435` → `cb_rt_type_previous`

**What's wrong.** No fixture in the diff suite uses CB `Before()`/`Previous()`. The IR `InstKind::Previous` lowering (call `cb_rt_type_previous` + apply the sentinel-hiding guard) is exercised only by the C-level gtest `Type.PreviousHidesSentinel`, never through the LLVM lowering against the interpreter. The interpreter has a distinct sentinel guard (`interp.rs:760-787`) that the differential harness does not pin. Downgraded to low because the LLVM lowering is a trivial passthrough to the gtest-pinned `cb_rt_type_previous`, bounding the realistic undetected-divergence risk.

**Suggested fix.** Add a `type_previous.cb` fixture that walks the type-instance list backwards from `Last` via `Previous()` (including stepping off the first node to `Null`).

---

### F18 — [LOW] Differential harness ignores stderr and treats signal-kill exit codes as equal

- **Category:** quality · **Confidence:** low
- **Location:** `crates/cb-driver/tests/diff_llvm.rs:62-75`

**What's wrong.** The harness compares only newline-normalized stdout and `status.code()`. Two soundness gaps: (1) trap fixtures (`array_oob`, `fnptr_null`) never compare **stderr**, so a divergent or empty trap message on the LLVM side passes silently — despite FD-049 stating traps lower "to match the interpreter's trap messages". (2) On Unix a signal-killed process yields `status.code() == None`; a `SIGSEGV`/`SIGABRT` on the produced exe surfaces as `None`, which `assert_eq!` compares equal to an interp that was also signal-killed, masking a crash.

**Suggested fix.** For fault fixtures, also assert the produced exe printed the expected trap substring to stderr (or at least a non-`None`, specific exit code). Optionally assert `run.status.code().is_some()` so a signal-kill can never compare equal-by-absence.

---

### F19 — [LOW] No coverage of empty arrays and negative array indices

- **Category:** coverage-gap · **Confidence:** low
- **Location:** `crates/cb-driver/tests/fixtures/programs/array_oob.cb:1-5`

**What's wrong.** `array_oob.cb` only tests a positive out-of-bounds index (`5` on a length-3 array). The negative-index branch and the empty-array case (`New Int[0]`: `Len()==0`, `For Each` over it, index `0` trapping) are distinct trap-trigger paths, pinned only at the C gtest level, never end-to-end through the LLVM lowering against the interpreter.

**Suggested fix.** Add fixtures for a negative index (`arr[-1]`) and an empty array (`New Int[0]`: print `Len`, `For Each`, then index `0` to trap), asserting `interp == llvm`.

---

## Duplicates (removed)

- **F9 → F4.** The value-structs agent independently found the same return-value-struct use-after-free as the string-refcount agent (F4). Same mechanism, location (`func.rs:1480-1500`), and example program. F4 is canonical.
- **F13 → F1.** The "no div-by-zero fixture" coverage gap is the test-side shadow of F1 (same `func.rs:1167-1168` citation, same `interp.rs:1176-1187` oracle). It adds no distinct root cause. *(F1 itself is rejected below as documented Phase-4 work, so the div-by-zero topic is tracked there.)*

---

## Rejected findings (and why)

- **F1 — Integer `Div`/`Mod` emit raw `sdiv`/`srem` (UB on `/0` and `INT_MIN/-1`).** The code reading is accurate — `func.rs:1167-1168` has no divisor guard, and the interpreter traps `DivisionByZero` (`interp.rs:1176-1187`). **But this is explicitly documented Phase-4 work.** FD-049's status is "In Progress (Phase 3 complete)", and Phase 4 ("Traps") names verbatim "the runtime checks that reach it (… **division by zero** …), lowered to match the interpreter's trap messages and exit codes." The finding's argument that Phase 4 only lowers `Terminator::Trap` (and so would miss the inline div check) misreads Phase 4, which scopes *the checks themselves*. The FD does not claim div-by-zero works in the delivered Phase-3 scope. → **Tracked as Phase-4 work, not an in-scope bug.** *(Worth keeping on the Phase-4 checklist: guard integer `Div`/`Mod` with a zero/`INT_MIN`-`-1` compare branching to the trap path, and add the `div_by_zero.cb` fixture from F13.)*
- **F6 — `cb_rt_array_new` lacks a negative-rank / null-dims guard.** Unreachable: the sole caller `build_new_array` (`func.rs:625-642`) always passes `rank = dims.len()` (≥0) and a valid scratch pointer; the interpreter has no rank field at all. The finding itself concedes "it is not a live bug … purely a future-safety robustness gap." → Not a real divergence/safety bug. *(Optional hardening only.)*
- **F7 — `String` sub-fields of value-struct array elements default to null.** This is precisely FD-049's **documented Phase-3b scoped limitation**; the finding concedes it is "NOT more severe than documented" and "currently observably identical to the oracle" (null-tolerant primitives). → Documented intentional limitation, not a bug. *(Note: distinct from F17, which is a test-coverage gap and is confirmed.)*
- **F11 — Cross-block register availability relies on def-before-use block order.** The structural observation is accurate (one `regs` map, never reset; blocks emitted in declaration order), but the finding is self-admittedly non-triggerable. For reachable code, sema's block order respects dominance (all existing CFG fixtures pass); for unreachable blocks the worst case is a clean codegen `Err`, not UB or an interpreter divergence. → No demonstrated bug. *(A debug assertion / clearer "undefined reg" diagnostic would be a reasonable defensive nicety.)*

---

## Recommended priority

1. **Fix F4/F9 (critical UAF)** — retain the returned struct's `String` sub-fields before releasing locals; add `struct_return_string.cb`. This is a real memory-safety bug on a compilable program.
2. **Fix F10 (high)** — handle a `Float` `BranchIf` condition (or coerce conditions to `Int` in sema); add a `float_condition` fixture. Currently crashes the compiler on valid programs.
3. **Fix F2 (medium)** — base shift width on the LHS only.
4. **Close the test-coverage gaps F15/F16/F17** (medium) and F14/F18/F19 (low) — several confirmed-bug areas (struct `String` refcount, narrow-int semantics, `RedimGlobal`) are entirely unexercised by the differential harness, which is why F4 went undetected.
5. **Address the latent invariant violations F5/F8 (low)** while in the relevant code, and **F12 (low)** by switching the AOT stdout to binary mode on Windows.
6. **Carry F1 (div/mod by zero) into Phase 4** as already planned.
