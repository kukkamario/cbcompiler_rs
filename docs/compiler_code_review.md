# Compiler Code Review

A structured review of the `cbcompiler_rs` Rust source, focused on
**inconsistencies, duplicate code, and undocumented oversights**, with a bias
toward changes that make the code *clearer to understand*.

## How this report was produced

The workspace (~23k lines across 8 crates) was partitioned into 10 domains and
reviewed independently, then every raw finding was re-verified against the
source by a second pass that traced control flow for bug claims, dropped false
positives, and merged duplicates. Each finding below cites a confirmed
`file:line`. Findings are graded:

- **High** — confirmed miscompile or correctness bug.
- **Medium** — worthwhile clarity/dedup/robustness improvement, or a real but
  lower-impact gap.
- **Low** — nitpick, missing doc, or minor cleanup.

Line numbers reflect the tree at review time and may drift as the code changes.

## Summary

| Domain | High | Medium | Low (open) |
|---|---|---|---|
| Sema (check / lower / types / scope) | 0 | 0 | 1 |
| Frontend (lexer / parser / AST) | 0 | 0 | 0 |
| IR + Interpreter | 0 | 0 | 0 |
| Diagnostics + Runtime/Driver/LLVM | 0 | 0 | 0 |

> **Resolved findings have been removed from this report.** Fixed and dropped:
> the four High-severity `cb-sema` miscompiles (S-H1–S-H4); the "Bundle 1" sema
> validation gaps (S-M1–S-M5); the "Bundle 2" fail-loud robustness items (S-M6,
> S-M7, S-M8, II-V1, II-V2, II-V3 — except the still-deferred `Call`-result/
> signature cross-check — II-V20, II-V27); the "Bundle 3" behavioral
> inconsistencies (DR-R1, DR-R2/DR-R3, II-V26); the "Bundle 4" de-dup sweep
> + AST consolidation (S-M11, S-M12, S-M13, S-M14, II-V10, F-L1, F-L3, F-P2,
> F-A1, F-A2, F-A3, and the F-L6 spec reconciliation); the "Bundle 5" Medium
> clarity/robustness sweep (S-M9, S-M10, F-L2, F-L4, F-L5, F-P1, F-P3, F-A4 — and
> the linked S-L6 resolved by S-M9's inline note); and the **"Bundle 6"** final
> Low-severity sweep, grouped by change type:
>
> - **docs** — S-L1/12/13/14/16/21, F-L7/8/9/15/17/18, F-P5, F-A5/6/10/11/12,
>   II-V8/9/13/15/16/18/19/21/24/29, DR-D1/4/5/8/9/13/14, DR-R4/5/6/7/9;
> - **refactor** — S-L7/10/15/17/18/20, F-L10/11/12/16, F-P7, F-A7/8/9/13,
>   II-V4/12/30/31, DR-D3/7/12, DR-R8;
> - **robustness** — S-L8/9/11, F-L19, II-V7/11/14/17/22, DR-D2/11;
> - **diagnostics** — S-L4/5, F-L13/14, F-P4/6/8, II-V5/6/25/28, DR-D10.
>
> S-L2/S-L3 were already fixed before the sweep. **II-V23** and **DR-R10** were
> converted to Planned FDs rather than fixed inline — see
> [FD-043](features/FD-043_INTERPRETER_TEARDOWN_HOOK.md) (interpreter teardown
> hook) and [FD-044](features/FD-044_BACKEND_TRAIT_SEAM.md) (backend trait seam).
> DR-D6 was a false positive and is recorded under
> [Confirmed non-issues](#confirmed-non-issues-checked-and-rejected). The single
> open finding (**S-L19**) is below.

---

## Sema (`cb-sema`)

### Low

- **S-L19** *(deferred — behavior risk)* — `ExprStmt` bare-call duplicates callee
  resolution from `lower_call` without consulting `resolved_calls`/intrinsics.
  `lower.rs` (the `ExprStmt` bare-identifier arm). **Left as-is in Bundle 6:**
  routing it through `lower_call` is correct for the resolved cases, but not
  provably behavior-preserving for the `OverloadSet` ambiguous-error path — with
  multiple zero-param variants sema emits `E_AMBIGUOUS_OVERLOAD` and records no
  `resolved_calls` entry, so `lower_call` would fall through to its
  `func_id_map[&name]` fallback and panic (OverloadSet names live only in
  `runtime_func_map`). The current arm picks the first empty-params variant and
  never panics. Revisit only with a path that preserves all cases.

---

## Frontend (`cb-frontend`)

All Low findings resolved in Bundle 6.

---

## IR + Interpreter (`cb-ir`, `cb-backend-interp`)

All Low findings resolved in Bundle 6, except the two converted to FDs:

- **II-V23** → [FD-043](features/FD-043_INTERPRETER_TEARDOWN_HOOK.md). The dead
  `runtime_hooks` field / unimplemented `about_to_exit` teardown is tracked there.

---

## Diagnostics + Runtime / Driver / LLVM

All Low findings resolved in Bundle 6, except the one converted to an FD:

- **DR-R10** → [FD-044](features/FD-044_BACKEND_TRAIT_SEAM.md). Materializing the
  `Backend` trait seam is a load-bearing structural decision (CLAUDE.md), tracked
  there for a dedicated design pass before LLVM codegen.

---

## Confirmed non-issues (checked and rejected)

To save future reviewers the re-investigation, these plausible-looking concerns
were checked against the source and found to be correct as written:

- **No LLVM/backend leakage in `cb-ir`** — public types depend only on
  `cb_diagnostics`/`std`; `fn_ptr: unsafe extern "C" fn()` is backend-neutral FFI.
- **`runtime_init` is called** — on the live path at
  `cb-backend-interp/src/interp.rs:112`; the driver correctly does not call it.
- **`HAS_GRAPHICS` is not dead** — used by driver tests (`cli.rs`, `programs.rs`).
- **`CB_TYPE_LONG` (tag 5) maps correctly to `IrType::Long`** and is correctly
  outside the reserved-tag set.
- **String comparison is correct** — bytewise UTF-8 ordering is provably
  equivalent to the §1.7/§5.3 "lexicographic by Unicode code point" rule.
- **`as_cb_string` vs `Display` for floats do not diverge** — both invoke the
  same `f64` `Display` impl. (A separate question — whether Rust's `f64` Display
  matches CB's documented float formatting — is a real latent concern but is
  unrelated to these two functions.)
- **`convert.rs` `(Null, RuntimeType)` arm is not dead** — `is_reference()`
  genuinely excludes `RuntimeType`; the overlap is now documented inline at the
  arm and on `Type::is_reference` (former clarity note S-L6, resolved with S-M9).
- **`LineIndex` / `offset_to_line_char_col` are well-tested** (withdrawn DR-D6) —
  the off-by-one-prone line/column arithmetic is covered by
  `crates/cb-diagnostics/tests/line_index.rs` (CRLF / bare-`\r` / LF terminators,
  past-EOF clamping, multi-byte char columns, FD-021 mid-codepoint flooring). The
  empty `mod tests {}` stub `source.rs` once carried — the basis of the original
  finding — was removed.
