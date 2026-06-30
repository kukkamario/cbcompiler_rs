# FD-056: User Function Overloading

**Status:** Pending Verification
**Priority:** Medium
**Effort:** High (> 4 hours)
**Impact:** Lets a user define multiple `Function`s sharing a name, distinguished by parameter signature — the same way runtime commands are already overload sets.

## Problem

Today each **user** function name binds to exactly one definition. A second
`Function` with the same name is rejected with `E0319`
(`E_DUPLICATE_DEFINITION`, emitted from `check.rs` when declaring the function).
The language spec currently codifies this in two places:

- `docs/cb_syntax.md` §7.2 (line ~1043): *"There is no function overloading:
  each function name is bound to exactly one definition."*
- `docs/cb_syntax.md` §7.4 (line ~1077): the function-pointer **address-of**
  rule (`fn = MySub` takes the address of `MySub`) is justified by *"Because
  there is no overloading … `MySub` always names exactly one function, so this
  is unambiguous."*

This is a **deliberate extension** of the dialect (the original CoolBasic has no
user overloading) — so it relaxes a documented invariant and the spec must be
*amended*: the semantics defined in this FD are authoritative for CB-rs.

Notably, the overload machinery already exists on the **runtime** side:
runtime *commands* are modelled as `DeclKind::OverloadSet { variants }` in sema,
with call-site resolution already implemented (FD-009 introduced `FuncId`-based
dispatch and overload resolution; `lower.rs` already walks `OverloadSet`
variants, and `check.rs` already ranks candidates and emits `E0323`
ambiguous / `E0324` no-match). User functions, by contrast, use
`DeclKind::Function { … }` and the single-binding
`try_declare(…, E_DUPLICATE_DEFINITION)` path. The core of this feature is
letting user functions participate in that same overload-set model.

## Design decisions

These were resolved with the user up front (they govern the implementation):

1. **Nature of the change** — an **intentional language extension**, not
   fidelity to original CoolBasic. `cb_syntax.md` is amended accordingly.
2. **Resolution key — full parameter types**, not arity alone. Overloads are
   distinguished by their parameter *types* (and count), so `f(Int)` and
   `f(String)` are distinct overloads.
3. **Ranking & ambiguity** — same rule as the existing runtime-command path:
   exact match preferred, then widening, then narrowing implicit conversions
   (§3.4). Reuse the existing diagnostics — `E0323` (ambiguous: multiple
   candidates match equally well) and `E0324` (no matching overload) — rather
   than inventing new codes.
4. **Default parameters (§7.2)** — defaulted trailing params may make two
   overloads' effective arities overlap. Declaration is permitted; any *call*
   that more than one candidate matches equally well is an **ambiguity error**
   (`E0323`). Rule of thumb: *always error when a call is ambiguous.*
5. **Return type / sub-vs-function** — **allowed.** Two overloads may differ
   only by return type, and a sub (no return) may share a name with a function
   (with return). Selection therefore considers the **call context** — a
   statement-form call (no value required) vs an expression-form call (value
   required) — alongside the parameter signature. Any residual tie is an
   `E0323` ambiguity error per decision 4.
6. **Function-pointer address-of (§7.4)** — the target is chosen from the
   **explicitly declared** function-pointer type of the destination. A bare
   overloaded name is *not* inferable on its own: if the destination's
   `Function(…)` type is not specified (e.g. relying on FD-042 default type
   inference), it is an **error**. With the type given —
   `Dim fn As Function(Int) As Int : fn = foo` — the declared signature selects
   the matching `foo` overload.

## Solution

Primarily a **`cb-sema`** change, reusing the existing runtime-command overload
infrastructure:

- **Declaration (`cb-sema/check.rs`).** Stop emitting `E0319` for same-named
  user functions whose signatures are distinguishable (decisions 2/5). Instead
  of one `DeclKind::Function`, accumulate same-named user functions into a
  `DeclKind::OverloadSet` (or a user-function analogue), reusing the variant
  representation already used for runtime commands. Keep `E0319` only for
  genuinely indistinguishable redefinitions (same parameter types *and* same
  sub/function role).
- **Call-site resolution (`cb-sema/lower.rs` + `check.rs`).** Extend the
  existing `OverloadSet` resolution path to rank user-function candidates by
  parameter type against the actual argument types (exact → widen → narrow),
  factoring in call context (statement vs expression) per decision 5, and select
  one `FuncId` to lower the call to. Reuse `E0324` for no viable candidate and
  `E0323` for an ambiguous match (decisions 3/4).
- **Address-of (`cb-sema`).** Resolve the §7.4 bare-name-in-value-context case
  against the destination's *explicitly declared* function-pointer type; error
  if that type is absent (no inference) or if it still can't narrow the set to
  one function (decision 6).
- **IR / backends.** Likely **no IR change**: each overload is already a distinct
  IR function with its own `FuncId`; calls already lower to a concrete callee.
  The work is choosing the callee in sema/lower, so `cb-ir`,
  `cb-backend-interp`, and `cb-backend-llvm` should be untouched. Confirm during
  design.
- **Frontend (`cb-frontend`).** Expected to need no parser change — multiple
  `Function` declarations already parse; the single-binding restriction lives in
  sema. Confirm.
- **Spec.** Update `docs/cb_syntax.md` §7.2 (replace the no-overloading
  statement with the rules above) and §7.4 (revise address-of — see below).

### Planned §7.4 revision — address-of an overloaded name

A bare name in a value context may now denote an **overload set** rather than a
single function, so the §7.4 "always one function, unambiguous" justification is
replaced by this rule:

- Taking the address of an overloaded name requires the **destination's
  function-pointer type to be explicitly declared**. There is no inference: a
  bare `fn = foo` whose `fn` has no declared `Function(…)` type is an error
  (decision 6) — FD-042 default inference does **not** apply to overloaded names.
- The destination's declared type selects the overload by an **exact signature
  match** — both *parameter types* **and** the *presence/absence of a return
  type*. This deliberately avoids function-pointer variance (no
  implicit-conversion ranking on address-of; that ranking applies only to
  *calls*, decision 3).
- The presence/absence of `As <ReturnType>` in the destination type is what
  discriminates a **sub** overload from a **function** overload of the *same
  parameter signature* (the decision-5 corner case):

  ```cb
  Function handle(x As Integer)              // sub  (no return)
  EndFunction
  Function handle(x As Integer) As Integer   // function (returns Integer)
      Return x
  EndFunction

  Dim asSub  As Function(Integer)              // no `As` → selects the sub
  Dim asFunc As Function(Integer) As Integer   // `As Integer` → selects the function
  asSub  = handle    // unambiguous: matches the sub overload
  asFunc = handle    // unambiguous: matches the function overload

  Dim bad As Function(Integer) As Float        // matches neither exactly
  bad = handle       // error: E0324 (no exact match — no return-type variance)
  ```

- If the declared type exactly matches more than one overload, it is `E0323`
  (ambiguous); if it matches none, `E0324` (no matching overload).

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-sema/src/check.rs` | MODIFY | Build a user-function overload set instead of `E0319`-rejecting distinguishable same-named functions; rank candidates; emit `E0323`/`E0324`; keep `E0319` for indistinguishable redefs |
| `crates/cb-sema/src/lower.rs` | MODIFY | Extend overload-set resolution to user functions at call sites; pick the callee `FuncId` (incl. statement-vs-expression context) |
| `crates/cb-sema/src/scope.rs` | MODIFY (likely) | Declaration/lookup support for user overload sets |
| `crates/cb-sema/src/diagnostics.rs` | NONE expected | Reuse existing `E0323`/`E0324`; no new codes anticipated |
| `crates/cb-sema/tests/lower_snapshots.rs` | MODIFY | Snapshots pinning resolution choices |
| `docs/cb_syntax.md` | MODIFY | Revise §7.2 (overloading rules) and §7.4 (address-of disambiguation) |
| `crates/cb-driver/tests/fixtures/programs/*.cb` | CREATE | End-to-end fixtures exercising overload selection (interp == native) |

## Verification

- `cargo test -p cb-sema` — declaration + resolution unit/snapshot tests:
  exact-match, implicit-conversion (widen/narrow) ranking, `E0323` ambiguity,
  `E0324` no-viable-candidate, default-param overlap, sub-vs-function sharing a
  name, and the §7.4 address-of cases (typed destination resolves; untyped/
  ambiguous errors).
- Add `.cb` fixtures and run them through the `diff_llvm` differential suite so
  interp and native agree on which overload is selected and the result.
- Confirm `E0319` still fires for truly indistinguishable redefinitions.

## Implementation notes

Implemented as designed; cross-phase identity is the function's `Stmt::Function`
`NodeId` (`DeclKind::Function.def`, `OverloadTarget::User { def }`,
`ResolvedCall::UserDefined { def }`, lowering's `func_def_map`). The expected-type
helper `check_value_with_expected` drives address-of resolution at all three
sites (assignment/Dim-init, function-pointer argument, return). Two refinements
surfaced during implementation:

- **Body scope per overload.** `DeclKind::Function.scope` linked a function to
  its body scope, but an overloaded name is an `OverloadSet`; `OverloadVariant`
  gained a `scope` field and `update_function_scope` keys it by definition node,
  so each overload's locals/consts resolve correctly in lowering.
- **Sub/function tie-break is user-only.** The void-vs-value call-context
  tie-break (decision 5) is gated to user-function variants; runtime overloads
  keep the prior "any tie ⇒ `E0323`" behaviour (preserves existing tests). A
  statement-position call (parenthesised or bare) is checked with
  `value_required = false`.

Verified: full `cargo test --workspace` (829 tests) + `cargo clippy --workspace
--all-targets -D warnings` clean + the 55-fixture `diff_llvm` differential suite
(interp == native), including the new `overloading` fixture.

## Related

- `docs/cb_syntax.md` §7.1–7.4 (functions, parameters, first-class functions)
- [FD-009](archive/FD-009_RUNTIME_LIBRARY.md) — introduced `FuncId`-based dispatch
  and overload resolution for runtime commands (the machinery to reuse)
- [FD-042](archive/FD-042_DEFAULT_TYPE_INFERENCE.md) — default type inference;
  decision 6 deliberately does *not* extend it to bare overloaded names
- `E0319` (`E_DUPLICATE_DEFINITION`), `E0323` (`E_AMBIGUOUS_OVERLOAD`),
  `E0324` (`E_NO_MATCHING_OVERLOAD`) in `crates/cb-sema/src/diagnostics.rs`
