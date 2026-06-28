# FD-050: Optional Trap Generation (safe vs. max-performance codegen)

**Status:** Planned
**Priority:** Medium (quality / performance; not blocking the two-backend milestone)
**Effort:** Medium
**Impact:** Lets the LLVM/AOT backend choose between *checked* codegen (runtime safety checks that trap with a diagnostic, matching the interpreter — the debuggable default) and *unchecked* codegen (raw operations, UB on a faulting path — max performance, C-like). Also closes the one runtime check the native backend never implemented: integer division/modulo by zero.

## Problem

The interpreter (`cb-backend-interp`) is the debuggable reference backend and **always** performs runtime safety checks, trapping with a specific `TrapKind` message + exit code: null deref, deleted/double-delete access, array index out of bounds, null function-pointer call, and division by zero (`interp.rs` `trap_error(...)`).

FD-049 made the LLVM backend mirror **most** of these as *always-on* checks:

- Array bounds / negative index / rank → trap inside `runtime/cb_array.cpp` (`raise_error` → `cb_rt_exit(1)`).
- Type instance null / deleted / double-delete access → trap inside `runtime/cb_type.cpp`.
- Null function-pointer call → `cb_rt_trap_null_fnptr` → stderr message + `exit(1)` (FD-049 review F18).

Two gaps remain:

1. **Division/modulo by zero is unchecked in the native backend** (FD-049 review F1/F13). `lower_binop` emits raw `build_int_signed_div` / `build_int_signed_rem` (`func.rs:1201-1202`) with no guard, so `Print 5/0` is UB (SIGFPE / signal exit) and `INT_MIN/-1` overflows — diverging from the interpreter, which traps `DivisionByZero` cleanly and handles `INT_MIN/-1` via `wrapping_div`/`wrapping_rem`. This is a real divergence on a compilable program.

2. **There is no way to turn the checks off.** For AOT / optimized native builds a user may legitimately want maximum performance and accept UB on faulting paths (the C contract). Today every check is hardwired on; bounds checks in hot loops can't be elided.

These are one concern — *trap-generation policy* — that cuts across every trap site, which is why it is split out of FD-049 (which delivers the lowering itself) into its own FD.

## Goal

Make runtime-check / trap generation a **codegen policy** the user selects:

- **Checked (default):** emit every runtime check; on a fault, trap with the interpreter-matching message and exit code. Safe and debuggable; the LLVM backend behaves like the interpreter oracle.
- **Unchecked:** skip the checks; emit raw operations / addresses. UB on a faulting path, but no per-operation overhead.

…and finish the one missing *checked* path (div/mod by zero) so "checked" is genuinely complete and the LLVM backend matches the interpreter on **all** trap kinds.

The interpreter stays **always checked** — it is the reference/debuggable backend and is not part of this toggle.

## Scope

Items **carried over from FD-049 Phase 4** (the checked path; do these regardless of the toggle):

- **A. Integer `Div`/`Mod` by zero guard (review F1/F13).** In checked mode, guard the divisor (`== 0`, plus `INT_MIN && -1` for signed `Div` overflow) → conditional branch to a trap block emitting the `DivisionByZero` message + `exit(1)`, reusing the fnptr-null trap idiom (`func.rs:1443-1453`). Add a `div_by_zero.cb` differential fixture.
- **B. `Terminator::Trap(kind)` proper lowering.** Today `func.rs:1603-1608` lowers *any* `Trap` terminator to a generic `exit(1)`, ignoring `kind` / `kind.message()`. Emit the kind-specific message + the matching exit code. *(Note: sema does not currently construct `Terminator::Trap` at all — the interpreter's traps are inline `trap_error` calls, not the terminator — so this arm is presently unreachable. Part of this item is deciding whether anything should emit it, or whether it stays a defensive no-miscompile fallback.)*
- **C. Trap-message parity.** The differential harness deliberately does **not** compare stderr trap text today (the Rust interp's strings differ from the C runtime's); fault fixtures only assert exit-code parity + non-empty stderr. Route both backends through a single source of truth (`TrapKind::message()` already exists on the IR side) so the harness can assert exact stderr in checked mode.

The **new feature** (the toggle itself):

- **D. Checked/unchecked codegen switch.** In unchecked mode the LLVM backend skips: the div/mod guard (A), the null-fn-ptr null check, the array bounds check, and the type null/deleted guards — emitting raw ops / direct addressing instead. The array and type checks currently live **inside** the C helpers (`cb_array.cpp` / `cb_type.cpp`), so unchecked mode needs either unchecked helper variants (e.g. `cb_rt_array_elem_addr_unchecked`) or inline GEP addressing that bypasses the helper. The interpreter is untouched.

## Open questions (load-bearing — resolve at design time, do not pick unilaterally)

- **Toggle mechanism.** A driver runtime flag (e.g. `--checks on|off`, `--unsafe`, or implied by an `-O`/optimization level) vs. a cargo feature vs. both? The architecture ground rule wants backends selectable at compile time *and* ideally at runtime; trap policy is per-invocation codegen, so a runtime CLI flag fits best, but confirm with the user.
- **Default.** Checked-by-default (safety first), unchecked strictly opt-in — recommended, confirm.
- **Granularity.** All-or-nothing first, or per-category (e.g. keep the one-compare div-by-zero guard always, drop only the expensive in-loop bounds checks)? The cheap checks may be worth keeping even in "fast" mode.
- **C-runtime array/type traps.** Unchecked helper variants vs. inline addressing that skips the helper. Inline is faster but duplicates the addressing logic the helper centralizes (and the row-major Horner fold).

## Crates & Areas Touched

| Area | Action | Purpose |
|------|--------|---------|
| `cb-backend-llvm` (codegen) | MODIFY | Gate each runtime check on the policy; add the div/mod guard (checked); kind-specific `Trap` terminator lowering. |
| `runtime/` (C++ core) | MODIFY | Possibly unchecked helper variants for array/type addressing; shared trap-message text for parity. |
| `cb-driver` | MODIFY | The checked/unchecked flag (mechanism per the open question). |
| `cb-ir` / shared | MAYBE | Make `TrapKind::message()` the single source of trap text for both backends. |
| `cb-driver/tests` (`diff_llvm`) + fixtures | CREATE | `div_by_zero.cb`; exact-stderr assertion in checked mode; an unchecked-mode smoke (UB paths excluded from differential equality). |

## Verification

- **Checked mode (default):** the existing `diff_llvm` differential suite stays green; add `div_by_zero.cb` (interp traps vs. native traps, same exit code) and tighten fault fixtures to compare exact stderr once message parity (C) lands.
- **Unchecked mode:** non-faulting programs must still match the interpreter bit-for-bit (the checks are the only difference). Faulting programs are **not** differential targets in unchecked mode (UB has no defined oracle) — cover them with a build/run smoke that the exe is produced and links, not an equality assertion.
- Default `cargo build` / `cargo test --workspace` stay LLVM-free (unchanged rule).

## Related

- [FD-049](archive/FD-049_IR_TO_LLVM_LOWERING.md) — the IR→LLVM lowering this extends; trap *policy* was descoped from it into this FD. Source of the carried-over review findings F1/F13 (div/mod) and the always-on Phase 2–3 checks.
- [FD-015](archive/FD-015_RUNTIME_TRAP_CHANNEL.md) — the runtime trap channel (`raise_error`) the checked native traps already route through.
- `cb-backend-interp/src/interp.rs` — the always-checked reference oracle (`trap_error`, `TrapKind`); the behavior checked-mode codegen must match.
