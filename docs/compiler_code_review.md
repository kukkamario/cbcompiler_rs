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

| Domain | High | Medium | Low |
|---|---|---|---|
| Sema (check / lower / types / scope) | 0 | 6 | 21 |
| Frontend (lexer / parser / AST) | 0 | 12 | 28 |
| IR + Interpreter | 0 | 2 | 25 |
| Diagnostics + Runtime/Driver/LLVM | 0 | 3 | 21 |

> The four High-severity `cb-sema` miscompiles originally reported here (S-H1
> implicit For/ForEach loop-variable aliasing, S-H2 `Select … Default` in a
> non-final position dropping later `Case`s, S-H3 logical `Xor` lowered as
> bitwise, S-H4 block-nested top-level `Const` not hoisted) have been **fixed**
> and removed from this report.
>
> The five "Bundle 1" `cb-sema` validation gaps (S-M1 `Select` Case
> constness/convertibility, S-M2 conversion-intrinsic operand, S-M3 array-index
> operand type, S-M4 param/field sigil-`As` disagreement, S-M5 non-constant
> `Const` initializer) have also been **fixed**; their entries below are marked
> ✅ Fixed and kept for traceability.
>
> The "Bundle 2" fail-loud robustness items have also been **fixed**: the IR
> verifier now checks single-assignment (II-V2), result-presence-vs-kind
> (II-V3, except the deferred `Call`-result/signature cross-check), and
> `params`↔`is_param`-locals agreement (II-V1); `default_value` panics on an
> unknown struct (II-V20); `push_frame` debug-asserts call arity (II-V27); the
> `type_def_map` `"<unknown>"` indexing now falls back gracefully (S-M6); and
> misplaced `Break`/`Continue` is a new sema error **E0332** with a lowering
> backstop (S-M7/S-M8). Entries are marked ✅ Fixed.

### Cross-cutting themes

These patterns recur across crates and are worth treating as systemic, not
one-off:

- **Silent fallbacks that mask lowering bugs.** The interpreter is the
  *reference implementation* and should fail loudly on internal inconsistencies,
  but several paths swallow them: `default_value` returning `Null` for an unknown
  struct (II-V20), `push_frame` tolerating arity mismatch (II-V27), and
  `type_def_map[..]` indexing a fabricated `"<unknown>"` key (S-M6). Prefer
  `debug_assert!`/`panic!` at these points.
- **Coercion / widening logic duplicated across crates.** The "Byte/Short widen
  to Int, else keep type" rule appears three times in `cb-sema` (S-M14), and a
  near-identical value→i64 / value→f64 pair exists in both `interp.rs` and
  `ffi.rs` with subtly divergent string handling (II-V10).
- **`RuntimeType` is reference-like but excluded from `is_reference()`**, forcing
  duplicated special-case arms in conversion and equality logic. The exclusion
  is deliberate but undocumented. (S-M9)
- **IR verification is incomplete in ways that can mask lowering bugs** — no
  single-assignment check, no result-presence-vs-instruction-kind check, and
  `Function.params` is never cross-checked against the `is_param` locals.
  (II-V1/V2/V3)
- **Keyword-only structural duplication in the AST** (`Type`/`Struct`,
  `Dim`/`Global` are byte-for-byte identical variants). Per `CLAUDE.md` these are
  load-bearing-adjacent and should be discussed before refactoring. (F-A1/A2)
- **Invariants documented in prose far from the code that relies on them** —
  trap-channel single-slot reentrancy (II-V17), the two-level scope tree
  (S-M10), span ownership in FFI (II-V16), and the IR's reverse-postorder
  dominance assumption (II-V8).
- **One genuine code-vs-spec disagreement:** `\xNN` decodes to a Unicode code
  point, but `cb_syntax.md` §1.6 calls it a byte. (F-L6)

---

## Sema (`cb-sema`)

> The four High-severity findings (S-H1–S-H4) have been fixed and removed; the
> Medium/Low findings below are unchanged.

### Medium

#### S-M1 — `Select` Case values are never checked for constness or convertibility ✅ Fixed
- `check.rs:2065-2088` (`check_select`)
- Category: Oversight
- §6.2 requires every `Case` value to be a constant expression implicitly
  convertible to the scrutinee type. `check_select` runs `check_expr` on each
  value but never evaluates it as a constant nor coerces it against the scrutinee
  type, so `Case "foo"` against an Integer scrutinee, or a non-constant `Case x`,
  passes silently. (Admits programs that should be rejected; does not corrupt
  codegen.)
- Fix: run `eval_const_expr` (E0322 if `None`) and `coerce` each value to the
  scrutinee type (E0317/E0318 on mismatch); add a test.

#### S-M2 — Conversion-intrinsic argument type computed then discarded ✅ Fixed
- `check.rs:1289-1308` (`check_conversion_intrinsic`)
- Category: Oversight
- `Int(x)`/`Float(x)`/`Str(x)` check arity but drop the result of
  `check_expr(args[0])` with no convertibility check, so `Int(myTypeRef)` is
  accepted. Contrast the `Len` arm, which validates operand kinds.
- Fix: validate the operand (numeric/String for Int/Float; numeric/String for
  Str), emitting E0301/E0317 otherwise.

#### S-M3 — `check_index` ignores index operand types ✅ Fixed
- `check.rs:1310-1339` (`check_index`)
- Category: Oversight / Inconsistency
- Index expressions are collected into `_idx_types` and discarded; only array
  rank is checked, so `arr[1.5]` / `arr["x"]` pass. `check_new` and
  `check_redim` both reject non-integer dimensions, so this is also internally
  inconsistent.
- Fix: for each index, emit E0301 when `!ty.is_integer() && !ty.is_error()`.

#### S-M4 — `resolve_var_type` sigil/As-disagreement flag dropped for params and fields ✅ Fixed
- `check.rs:603` (pass1 params), `check.rs:655` & `:692` (fields), `check.rs:2123` (`check_function` params)
- Category: Inconsistency / Oversight
- `Dim`/`Global`/`Const`/return types act on the disagreement flag (E0320), but
  for parameters and `Field` declarations it is discarded, so
  `Function f(count% As Float)` or `Field x% As Float` silently accepts a
  sigil/type contradiction. §1.4 gives no exemption.
- Fix: emit E0320 for params and fields too, or document why they are exempt.

#### S-M5 — Non-constant `Const` initializer silently accepted ✅ Fixed
- `check.rs:1479-1492` (Const arm of `check_stmt`)
- Category: Oversight
- §4.4 requires a constant expression. When `eval_const_expr(value)` is `None`
  (e.g. `Const x = someVar`), the declaration keeps its zero placeholder and no
  error is emitted.
- Fix: emit E0322 at the value span when `eval_const_expr` returns `None`.

#### S-M6 — `type_def_map[&type_name]` panics on the `"<unknown>"` fallback ✅ Fixed
- `lower.rs:811` (`New(Type)`), `lower.rs:1125` (first/last), `lower.rs:1922` (`lower_for_each_type`)
- Category: Oversight
- On a non-`TypeRef` resolved type the code fabricates an interned `"<unknown>"`
  symbol and immediately indexes `type_def_map[&type_name]` with a key that is
  never present → panic. Only reachable on error/degenerate inputs, but an
  ungraceful failure.
- Fix: use `type_def_map.get(&type_name)` and fall back to a safe IR value (e.g.
  `ConstNull`) when absent.

#### S-M7 — `Break`/`Continue` silently no-op when no enclosing context is found ✅ Fixed
- `lower.rs:1382-1398` (`Break`), `lower.rs:1399-1415` (`Continue`)
- Category: Oversight
- If `Break N` can't find the Nth enclosing loop (or `Continue` has an empty
  context stack), nothing is emitted and the block is left unterminated and
  falls through. Sema presumably validates placement, but if anything slips
  through this is a silent miscompile with no documented assumption.
- Fix: document the sema-validated-placement assumption, and emit an
  unreachable/trap terminator (or `debug_assert!`) when no target is found.

#### S-M8 — `Continue` in a `Select` last arm leaves the block unterminated ✅ Fixed
- `lower.rs:1408-1412` (`Continue`, Select context)
- Category: Bug / Oversight
- For a `Select` arm, `Continue` should fall through to the next case body, but
  the lowering only terminates when `next_arm_body` is `Some`. For the last arm
  it emits nothing and relies on the `Goto(merge_block)` guard at `lower.rs:2182`
  — silently ignoring intent.
- Fix: confirm against §6.2 whether `Continue` in the final case is legal; if
  illegal make it a sema error, if legal terminate explicitly (likely
  `merge_block`).

#### S-M9 — `RuntimeType` excluded from `is_reference()`, forcing scattered special-casing
- `types.rs:60-65` (`is_reference`), `types.rs:277-279` (Eq/NotEq), `convert.rs:77-80`
- Category: Inconsistency
- §3.5 says opaque runtime types "behave like references," yet `is_reference()`
  covers only `TypeRef | Array | FnPtr`. As a result `convert.rs` needs a
  separate `(Null, RuntimeType)` arm duplicating the `(Null, t) if t.is_reference()`
  arm, and `binary_result_type` adds three explicit RuntimeType clauses
  duplicating ref/ref and Null/ref logic. The exclusion is deliberate (identity
  equality, no ordering) but undocumented.
- Fix: document why RuntimeType is excluded, or add `is_reference_like()`
  covering it and route the Null-conversion and equality clauses through it.

#### S-M10 — `lookup` two-level scope-tree assumption is load-bearing but undocumented
- `scope.rs:174-218` (`lookup`)
- Category: Oversight / Clarity
- The visibility filter `from_function && ps.kind == TopLevel` is computed from
  the *leaf* scope's kind and applied to every TopLevel parent. This is correct
  only because the tree is exactly two levels deep (functions can't nest, §7.1).
  Never asserted or documented.
- Fix: comment that the tree is at most TopLevel→Function;
  optionally `debug_assert!` no second Function scope appears in a parent chain.

#### S-M11 — Duplicated field-collection loops in `pass1_type_def` / `pass1_struct_def`
- `check.rs:645-662`, `check.rs:682-699`
- Category: Duplication
- Byte-for-byte identical field-info collection loops differing only in the final
  `DeclKind`/`ty` (both now also emit the sigil/`As` E0320 per S-M4, so a
  `collect_fields` extraction would dedup that too).
- Fix: extract `fn collect_fields(&mut self, fields: &[NodeId]) -> Vec<FieldInfo>`.

#### S-M12 — Duplicated `Dim`-init / `Global`-init coercion that recomputes `var_ty`
- `check.rs:1466-1476` (Global-with-init), `check.rs:1893-1904` (`check_dim`)
- Category: Duplication
- The "coerce initializer to declared type" logic is copy-pasted, and both
  recompute `resolve_var_type` purely to get `var_ty` for the single-name init.
- Fix: factor `fn coerce_initializer(...)` (or route Global-with-init through
  `check_dim`); at minimum reuse the already-computed `var_ty`.

#### S-M13 — Repeated loop-scaffolding pattern across five loop lowerings
- `lower.rs:1618-1880` (while / repeat-forever / repeat-while / for), `lower.rs:1909-2080` (for-each type/array)
- Category: Duplication
- Each loop repeats push `ControlContext::Loop` / lower body / pop /
  `if !terminated { Goto }` / `switch_to(exit)` ~5 times; a fallthrough fix must
  touch every copy.
- Fix: extract `fn lower_loop_body(&mut self, body, continue_block, exit_block, fallthrough, span)`.

#### S-M14 — "Byte/Short → Int, else clone" widening duplicated three times
- `types.rs:209-213` (`numeric_promote`), `types.rs:260-263` (shift LHS), `types.rs:316-321` (`unary_result_type::widen`)
- Category: Duplication
- §3.4's storage-widening rule is implemented three times with identical match
  arms; adding e.g. `Single` would touch all three.
- Fix: extract `fn widen_storage(t: &Type) -> Type`.

### Low

- **S-L1** — `collect_consts_recursive` doc comment is copied from the label
  collector and describes label collection, not const hoisting. `check.rs:411-413`.
- **S-L2** — Multi-value `Case` computes `else_target` then `let _ = else_target;`;
  dead binding obscures the chained-comparison logic. `lower.rs:2147-2170`.
- **S-L3** — Single-value vs multi-value `Case` comparison is duplicated; the
  `values.len() == 1` fast path duplicates one loop iteration. `lower.rs:2113-2172`.
- **S-L4** — `New T` gives `StructVal`/`RuntimeType` only the generic "New
  requires a Type name" message. `check.rs:1386-1402`.
- **S-L5** — Field access on `RuntimeType` falls through to a `{:?}`-formatted
  E0309 leaking `Symbol(n)`; several diagnostics use `{:?}` on `Type`. Consider a
  `Display` for `Type`. `check.rs:1349-1367`.
- **S-L6** — `find_implicit_conversion` Null arms `(Null, t) if t.is_reference())`
  and `(Null, RuntimeType)` overlap with no explanation (the second is *not* dead;
  ties to S-M9). `convert.rs:77-80`.
- **S-L7** — `maybe_convert` uses `IrType::Void` as the "unknown source type"
  sentinel, so Void means "void", "error", and "type unknown". `lower.rs:846-865`.
- **S-L8** — `reset_function_state` leaves `current_block` stale, relying on an
  immediate `fresh_block`; a future caller emitting first would panic.
  `lower.rs:279-289`.
- **S-L9** — `force_declare` overwrites silently; the "command-only" invariant
  lives only in prose. Add a `debug_assert!` or rename. `scope.rs:136-144`.
- **S-L10** — `lookup` doc lists a stale subset of hoisted kinds (omits
  Constant/Label/RuntimeFn/OverloadSet/RuntimeTypeDef). Consider
  `DeclKind::is_hoisted()`. `scope.rs:167-173`.
- **S-L11** — `update_function_scope` / `update_const_value` silently no-op on
  miss/kind-mismatch; a `debug_assert!` would catch pass-ordering bugs.
  `scope.rs:232-256`.
- **S-L12** — `Declaration.is_global` is consulted only for variables but
  undocumented. `scope.rs:33-38`.
- **S-L13** — Logical-op numeric-only restriction is undocumented vs the
  truthiness prose. `types.rs:302-308`.
- **S-L14** — `Byte`/`Short` unsignedness undocumented at the type definition.
  `types.rs:11-12`.
- **S-L15** — `numeric_promote` is `pub(crate)` while sibling type-algebra fns
  are `pub`. `types.rs:197`.
- **S-L16** — `resolve_type_expr` doc omits the `RuntimeType` refinement path.
  `types.rs:100-105`.
- **S-L17** — Recurring `diagnostics.push(Diagnostic::error(...))` and
  expect-integer boilerplate; add `err(...)` / `expect_integer(...)` helpers.
  e.g. `check.rs:1204-1218, 1395-1413, 2249-2255`.
- **S-L18** — Dead `file_id` field on `Checker` (`#[allow(dead_code)]`, never
  read). `check.rs:31-32`.
- **S-L19** — `ExprStmt` bare-call duplicates callee resolution from `lower_call`
  without consulting `resolved_calls`/intrinsics. `lower.rs:1207-1240`.
- **S-L20** — `lower_function_def` destructures AST fields (`params: _`, …) it
  then re-fetches from the symbol table. `lower.rs:528-566`.
- **S-L21** — `For` direction test recomputed every iteration (acceptable for the
  interpreter); comment conflates default-step and direction-zero constants.
  `lower.rs:1744-1747`.

---

## Frontend (`cb-frontend`)

### Medium

#### F-L6 — `\xNN` decodes to a Unicode code point, contradicting `cb_syntax.md` §1.6
- `string_value.rs:140-184`; spec `docs/cb_syntax.md:179`
- Category: Inconsistency (code vs spec)
- The spec table calls `\xNN` a byte, but the implementation pushes
  `char::from_u32(cp)` for `cp ∈ 0..=0xFF`, so `\xFF` becomes U+00FF (UTF-8
  `C3 BF`), not byte `0xFF`. The code comment acknowledges the divergence; the
  spec was never updated.
- Fix: reconcile — either update §1.6 to say "code point in 0..0xFF" (matches the
  impl and the `escaped_hex_ff` test) or implement true byte semantics.

#### F-L1 — Float path hand-rolls underscore-stripping three times
- `lexer.rs:677-739` vs the shared `strip_underscores` at `lexer.rs:595-612`
- Category: Duplication
- The int/hex/binary paths call `strip_underscores`, but the float branch
  open-codes the same "copy into scratch, skip `_`, bail on overflow" loop three
  times (int part, frac part, exponent), each repeating the buffer-overflow guard.
- Fix: factor an `append_stripped(buf, &mut n, bytes) -> Result<(), Overflow>`
  helper used by all three sub-parts.

#### F-L2 — Two parallel separator-validation mechanisms; numeric paths inconsistent
- `lexer.rs:535-590` (`scan_digit_run_inner` → `bad_sep`), `lexer.rs:925-946` (`has_separator_issue`); hex `:827`, binary `:892`, decimal `:667`
- Category: Duplication / Inconsistency
- `scan_digit_run_inner` already diagnoses and flags leading/doubled/trailing
  underscores. Hex/binary additionally OR in `has_separator_issue`, which
  re-scans the same bytes for the same conditions but emits *no* diagnostic;
  decimal trusts the flag alone. If `has_separator_issue` ever fired while
  `bad_sep` was false, an error token would be produced with no diagnostic.
- Fix: drop `has_separator_issue` and rely on the `scan_digit_run` flag uniformly.

#### F-L3 — `scan_number_hex` and `scan_number_binary` are ~65-line near-duplicates
- `lexer.rs:775-844` (hex), `lexer.rs:846-909` (binary)
- Category: Duplication
- Differ only in prefix byte, digit predicate, radix label, and parse base; the
  rest is copy-pasted.
- Fix: extract `fn scan_radix_number(&mut self, start, prefix, is_digit, radix, label)`.

#### F-L4 — `$_` / `%_` empty-run emits two diagnostics with a mismatched error-token kind
- `lexer.rs:812-823` (hex), `lexer.rs:882-890` (binary)
- Category: Inconsistency / Clarity
- For `$_` the pre-check emits `E0105 InvalidDigitSeparator` and consumes `_`;
  then the empty-`raw` path emits a second diagnostic `E0106 UnexpectedChar` but
  pushes a token of kind `InvalidDigitSeparator` — so one literal yields two
  diagnostics and the token kind doesn't match the dominant diagnostic.
- Fix: pick one primary error and keep the token kind aligned with it.

#### F-L5 — Float exponent reconstruction back-scans instead of reusing forward-scan offsets
- `lexer.rs:708-739`
- Category: Clarity / Bug-risk
- The forward scan already consumed `e`/`E` and the optional sign at known
  positions but discarded them; the rebuild reverse-scans with a loop that
  assumes at most one sign byte and exactly one `e`/`E` immediately preceding.
- Fix: capture the `e`/`E` position and sign during the forward scan and reuse
  them, removing the back-scan coupling.

#### F-P1 — Block-`If` header-newline handling diverges from every other block opener
- `parser.rs:1587-1589` vs `parse_while:1816`, `parse_for:1899/1934`, `parse_repeat:1839`, `ElseIf:1751-1752`
- Category: Inconsistency
- Block-`If` decides "block form" with `matches!(peek, Newline)` and bumps
  exactly one newline, while While/For/Repeat call `eat_newlines()` and `ElseIf`
  uses `require_newline_after_block_then(...)` then `eat_newlines()` — three ways
  to consume the end-of-header newline, with leading-`If` differing from `ElseIf`
  in the same construct.
- Fix: factor one `eat_block_header_newline` helper, or comment why leading-`If`
  consumes only one newline (it relies on `parse_block_until`'s own `eat_newlines`).

#### F-P2 — "Missing loop closer" recovery block copy-pasted three times
- `parser.rs:1818-1831`, `:1901-1910`, `:1936-1945`
- Category: Duplication
- Three near-identical blocks differing only in `Wend`/`Next` and the opener
  name string.
- Fix: extract `fn close_loop_block(&mut self, closer: Kw, opener: Span, name: &str) -> Span`.

#### F-P3 — `Select` aborts the whole block on one bad arm; record bodies catch-and-continue
- `parser.rs:2037, 2040` (`parse_case_arm()?`) vs `parse_record_body:2311-2317`
- Category: Inconsistency
- A parse error in a `Case`/`Default` arm propagates via `?` out of
  `parse_select`, discarding all already-parsed arms. `parse_record_body` instead
  matches `Ok/Err` per field, records the diagnostic, resyncs, and keeps going —
  materially different recovery granularity.
- Fix: wrap the arm parsers in an `Ok/Err` match (record + resync) like
  `parse_record_body`, or document why `Select` aborts wholesale.

#### F-A1 — `Stmt::Type` and `Stmt::Struct` are structurally identical
- `ast.rs:210-217`; collapsed in `ast_print.rs:190-192`
- Category: Inconsistency / Clarity
- Both carry `name_span: Span, fields: Vec<NodeId>` and are handled identically
  wherever traversed. The Type-vs-Struct distinction is real (§3.3) but encoded
  as two whole variants rather than a discriminant.
- Fix (load-bearing-adjacent — raise with the user first): consider
  `Stmt::TypeDecl { kind: TypeDeclKind, name_span, fields }`. If kept as two
  variants, add a doc comment on each citing §3.3.

#### F-A2 — `Stmt::Dim` and `Stmt::Global` are structurally identical
- `ast.rs:145-154`; collapsed in `ast_print.rs:102`
- Category: Inconsistency / Duplication
- Identical fields (`names`, `ty`, `init`); `Global`'s top-level-only rule is a
  sema/parse check, not a reason for a separate AST shape.
- Fix (load-bearing-adjacent — raise first): consider
  `Stmt::VarDecl { is_global: bool, names, ty, init }` (also resolves F-A3).

#### F-A4 — `Expr::Paren` / `TypeExpr::Paren` retained with no doc explaining why
- `ast.rs:121-123`, `ast.rs:314-316`
- Category: Clarity / Oversight
- Both wrap a single `inner` and add no semantic info (§5.4: `(T)` is the same
  type as `T`). Keeping them for span/round-trip fidelity is legitimate but
  undocumented — contrast the detailed FD-004 note on `Expr::Field`. A sema
  author can't tell if `Paren` is load-bearing or dead weight.
- Fix: add a one-line doc on each `Paren` stating it is kept for span/round-trip
  fidelity and is transparent to type/value, noting where it must be unwrapped.

### Low

- **F-L7** — `DIGIT_SCRATCH` doc wrongly claims i64/overflow-short-circuit
  rationale; integers parse as `u64`. `lexer.rs:49-52`.
- **F-L8** — `LONGEST_KEYWORD_LEN` doc says the scratch buffer lives "in the
  lexer"; it lives in `lookup`. `keywords.rs:75-77`.
- **F-L9** — `FloatBits` doc claims it enables `Token` `Hash`, but `Token`/`TokenKind`
  aren't `Hash`; the reason is `Eq`. `token.rs:80-86`.
- **F-L10** — `scan_string` has a redundant `is_some()` guard wrapping an
  identical `match self.peek_byte()`. `lexer.rs:396-411`.
- **F-L11** — CR/LF line-terminator consumption duplicated verbatim in
  `scan_newline` and `scan_continuation_or_backslash`. `lexer.rs:234-245, 260-271`.
- **F-L12** — `+`/`-`/`=` open-coded while sibling single-char tokens use
  `emit_single`. `lexer.rs:998-1014`.
- **F-L13** — `\u` invalid-scalar recovery drops the escape while `\x`/unknown
  arms copy source verbatim — inconsistent recovery. `string_value.rs:206-223`.
- **F-L14** — `body_offset_in_lit` recomputes a weaker predicate than
  `strip_single_quotes`; on a start-but-no-end quote they disagree, misaligning
  escape-diagnostic spans by one byte. `string_value.rs:66-72`.
- **F-L15** — `decode_raw` normalizes whitespace-only lines to empty, diverging
  from the "strip common indent" doc. `string_value.rs:365-373`.
- **F-L16** — `utf8_char_len` hand-rolls UTF-8 length walking over a known-valid
  `&str`; `char_indices`/`len_utf8` would be simpler. `string_value.rs:78-92, 249-263`.
- **F-L17** — `peek_char` relies on an undocumented char-boundary invariant
  (unlike `bump_byte`). `lexer.rs:91-93`.
- **F-L18** — `scan_digit_run` suppress-mode pre-sets `bad_sep = true`;
  undocumented. `lexer.rs:545-546`.
- **F-L19** — `Kw::as_str` and the `KEYWORDS` phf map are hand-maintained
  parallel lists with no round-trip test. `token.rs:188-258`, `keywords.rs:7-73`.
- **F-P4** — `parse_repeat` fabricates `RepeatForever` on a wrong non-EOF closer
  with no diagnostic (unlike While/For). `parser.rs:1855-1860`.
- **F-P5** — `is_block_end_marker` includes `While` but `parse_block_until`'s
  "let parent decide" list does not; behaviour is correct but the asymmetry is
  uncommented. `parser.rs:2758-2759` vs `:1477-1484`.
- **F-P6** — Trailing commas produce a misleading "expected expression"
  diagnostic in comma lists. `parser.rs:687-696` and peers.
- **F-P7** — `consume_block_closer` mismatched-split arm recomputes
  `split_end_to_joined` and uses `.expect`. `parser.rs:1516-1518`.
- **F-P8** — `Break` count `u32::MAX` bound rejects in-range-but-too-big literals
  with a misleading "must be a positive integer literal" message. `parser.rs:1378, 1389`.
- **F-A3** — Global-ness modelled inconsistently: variant split for Dim/Global,
  bool flag for Const. Resolve with F-A2. `ast.rs:145-161`.
- **F-A5** — `Expr::Ident` carries `sigil` but `Expr::Field` does not, while
  `FieldDecl` does. Confirm whether sigils are allowed at field-access sites.
  `ast.rs:90-93, 111-120`.
- **F-A6** — `IntLit(u64)` storage semantics undocumented at the AST node
  (contrast `FloatBits` and the token-level note). `ast.rs:83`.
- **F-A7** — `Stmt::Break { count: Option<u32> }` encoding undocumented; consider
  `NonZeroU32`. `ast.rs:232-234`.
- **F-A8** — `Goto.label_span` breaks the `name_span` naming convention.
  `ast.rs:227`.
- **F-A9** — `Const`/`FieldDecl` inline the `(name_span, sigil)` pair instead of
  reusing `DimName`. `ast.rs:155-161, 218-222`.
- **F-A10** — `Param.default` has no note that it is meaningless in fn-ptr type
  position. `ast.rs:287-295`.
- **F-A11** — `NewKind::Array.dims` vs `TypeExpr::Array.rank` asymmetry
  undocumented. `ast.rs:130-133, 306-309`.
- **F-A12** — `Arena` indexing / `NodeId` validity contract (ids valid only in
  their arena; AST immutable post-parse) undocumented. `ast.rs:46-49, 62-68`.
- **F-A13** — `ast_print::debug_print` is not re-exported at the crate root,
  unlike `ast::*`. `lib.rs:15-24`.

---

## IR + Interpreter (`cb-ir`, `cb-backend-interp`)

### Medium

#### II-V1 — `Function.params` duplicates `locals[is_param]` and is never cross-checked ✅ Fixed
- `cb-ir/src/lib.rs:203` (`params`), `lib.rs:167-171` (`Local.is_param`); consumers `print.rs:53`, `interp.rs:225-226`
- Category: Inconsistency / Oversight
- Parameter info is stored twice: the printer renders the signature from
  `func.params`; the interpreter sets up its frame from `func.locals` +
  `is_param`. `verify()` never asserts they agree (nor that either matches
  `decl.sig.params`), so a mismatch desyncs printer from interpreter silently.
- Fix: add a verifier check that `func.params` equals the types of the leading
  `is_param` locals (ideally also the `UserDefined` decl's `sig.params`), or
  derive `params` from `locals` and drop the field.

#### II-V2 — Verifier does not check single-assignment of result registers ✅ Fixed
- `cb-ir/src/verify.rs:91-93`
- Category: Oversight
- The forward pass does `defined_regs.insert(r)` and discards the bool, although
  the module doc frames the IR as SSA-like with a single def site per `Reg` that
  the interpreter relies on. A `Reg` defined twice passes undetected.
- Fix: `assert!(defined_regs.insert(r), "register {r} defined more than once")`,
  or document that redefinition is intentionally permitted.

#### II-V3 — Verifier never checks result-register presence vs instruction kind ✅ Fixed (subset; Call/signature cross-check deferred)
- `cb-ir/src/verify.rs:83-94`
- Category: Oversight
- Value-producing instructions must have `result: Some`; pure-effect ones must
  have `result: None`. The verifier asserts neither, so a `BinOp` with
  `result: None` or a `StoreLocal` with a spurious result both pass.
- Fix: classify each `InstKind` as value-producing vs void and assert
  accordingly (Call's void-ness comes from the callee's `sig.ret`).

#### II-V10 — Duplicated value→i64 / value→f64 coercion across `interp.rs` and `ffi.rs`
- `interp.rs:1200-1231` (`value_to_i64`/`value_to_f64`) vs `ffi.rs:82-102` (`value_as_i64`/`value_as_f64`)
- Category: Duplication
- Two near-identical coercion pairs that differ subtly: the interp path parses
  `Value::String` via `parse_leading_int`, while the ffi path returns `0` for
  strings. A fix to one will likely miss the other.
- Fix: extract a single coercion helper (methods on `Value`) used by both,
  documenting why the ffi path never legitimately sees strings (see II-V11).

#### II-V20 — `default_value` returns `Value::Null` for an unknown `StructVal` name — silent ✅ Fixed
- `value.rs:84-98` (fallback at 95-97)
- Category: Bug / Oversight
- If `StructVal(name)` references a struct missing from `struct_defs`,
  `default_value` returns `Value::Null` instead of a `Struct`. Downstream
  `GetField`/`StorePlace` then trap with `NullDeref`, producing a misleading
  error far from the real cause (a missing struct def — a lowering bug). The
  reference impl should fail loudly here.
- Fix: `panic!`/`debug_assert!` on a missing struct def, or at minimum document
  that `Null` here means "internal: unknown struct".

#### II-V26 — Non-shift integer binops only handle same-width `(Int,Int)`/`(Long,Long)`
- `interp.rs:930-936`; compare shift path `interp.rs:918-927` and `eval_unop` widening `interp.rs:1131-1152`
- Category: Inconsistency
- `eval_binop`'s arithmetic/comparison match has arms only for `(Int,Int)`,
  `(Long,Long)`, Float, and String pairs; `(Byte,Byte)`, `(Short,Short)`, and any
  mixed pair fall through to a generic "type mismatch". This relies entirely on
  sema inserting `Convert` before every BinOp — yet the *shift* path and
  `eval_unop` both directly widen Byte/Short. So shifts and unops tolerate
  Byte/Short but other binops don't, an internal asymmetry that turns Byte/Short
  arithmetic into a spurious type error if the coercion invariant ever slips (or
  for hand-written IR).
- Fix: document the "sema pre-converts all BinOp operands to Int/Long/Float"
  invariant at the top of `eval_binop`, or widen Byte/Short to Int there for
  consistency with `eval_unop` and the shift path.

### Low

- **II-V4** — Four `verify_inst_*` helpers duplicate bounds-check boilerplate;
  factor a shared `check_index(value, limit, label)`. `verify.rs:104-180`.
- **II-V5** — `DeleteLvalue` and `DeleteLvalueGlobal` print the same mnemonic,
  breaking the `_global`-suffix convention. `print.rs:204-209`.
- **II-V6** — `Redim` and `RedimGlobal` both print `redim`. `print.rs:282, 292`.
- **II-V7** — `ConstInt` and `ConstLong` both carry `i64` with no documented
  range distinction; an out-of-`Int`-range value in `ConstInt` has no verifier
  check. `inst.rs:137-138`.
- **II-V8** — The reverse-postorder dominance assumption is documented in the
  module doc but not at the `defined_regs` pass that depends on it; `verify` does
  not validate real dominance. `verify.rs:20-28, 74-94`.
- **II-V9** — `BasicBlock.terminator_span` pairing (meaningful only once
  `terminator` is `Some`) is undocumented. `cb-ir/src/lib.rs:195-196`.
- **II-V11** — ffi `value_as_i64`/`value_as_f64` string + fallthrough arms are
  dead and silently zero; would mask a type mismatch. Add `debug_assert!`/`unreachable!`.
  `ffi.rs:82-102`.
- **II-V12** — Three parallel "read registers into args/indices" loops, and the
  Call/CallIndirect dispatch tails are line-for-line parallel; factor
  `read_args` and `dispatch_call`. `interp.rs:462-483, 809-830`.
- **II-V13** — `Convert`-to-int uses leading-prefix string parse (`"3x"`→3) while
  `Convert`-to-float requires a full parse (`"3x"`→0.0); document or align.
  `interp.rs:1161-1231`.
- **II-V14** — Integer `Pow` negative-exponent arm uses magic return values for
  code that is effectively dead (§1.7: `^` always yields Float, confirmed by
  sema). Delete the arm or comment that it is hand-written-IR-only. `interp.rs:1023-1034`.
- **II-V15** — `value_to_i64` truncates Float toward zero and feeds array
  indices/dims; `2.9` silently becomes `2`. Document the integer-typed
  expectation. `interp.rs:1200-1213`.
- **II-V16** — `CbStringHandle::from_raw` ownership invariant (handles are owned,
  never borrowed) is documented only in the ffi module header, not at the
  safety-critical call site. `ffi.rs:174-185`, `string_handle.rs:27-29`.
- **II-V17** — `PENDING_TRAP` single-slot reentrancy assumption is unasserted;
  safe today (single FFI chokepoint) but a future nested FFI would silently lose
  the first trap. `interp.rs:35-41, 1336-1346`.
- **II-V18** — `find_main` reverse-scans the func table (last-wins) with no
  rationale. `interp.rs:195-204`.
- **II-V19** — `StrLen` counts UTF-8 codepoints inline; the LLVM backend will use
  a runtime char-length call, so the two definitions must be kept identical.
  `interp.rs:780-801`.
- **II-V21** — `default_value` collapses `Array`/`TypeRef`/`RuntimeType`/`Null`/`Void`
  to `Null` without distinguishing deliberate (CB semantics) from degenerate
  fallback. `value.rs:99-104`.
- **II-V22** — Deferred `after_inst` reconstruction relies on an unasserted
  `pc-1` heuristic; observability is the interpreter's whole point. Assert
  `insts[pc-1]` is the matching call. `interp.rs:346-366`.
- **II-V23** — `runtime_hooks` is dead (`#[allow(dead_code)]`); reserved
  `about_to_exit` teardown is never invoked. Track in an FD. `interp.rs:97-102`.
- **II-V24** — `CbStringHandle::is_empty` does an FFI round-trip and is used in
  the hot `is_truthy` path; undocumented per-call cost. `string_handle.rs:54-60`.
- **II-V25** — Shift with Float/String LHS falls through to a generic "type
  mismatch" rather than "shift requires integer operand". `interp.rs:918-928`.
- **II-V27** — ✅ Fixed. `push_frame` now `debug_assert_eq!`s `args.len()`
  against the parameter count. `interp.rs:225-238`.
- **II-V28** — `StorePlace`/`GetElement` index resolution wraps negative indices
  via `as usize`; caught later as out-of-bounds but with a less precise
  diagnostic than `resolve_dims`. `interp.rs:561-567, 688-710`.
- **II-V29** — `Previous` guards the head sentinel but `Next` does not; correct
  today but fragile and unexplained. `interp.rs:606-651`.
- **II-V30** — `convert_value` eagerly computes both int and float coercions
  regardless of target type (parses strings twice). `interp.rs:1168-1169`.
- **II-V31** — `TrapKind` → message map lives in the interpreter's `error.rs`,
  while the printer keeps its own separate names; a shared `Display` in `cb-ir`
  would unify them. `error.rs:33-40`.

---

## Diagnostics + Runtime / Driver / LLVM

### Medium

#### DR-R1 — Runtime catalog (and sema) run before the dump-only short-circuit
- `cb-driver/src/main.rs:187-193` (catalog load), `:196` (sema), `:214` (`dump_ast`)
- Category: Bug / Inconsistency
- `load_catalog()` and `cb_sema::analyze(..., &runtime_catalog)` both run *before*
  the `if dump_ast` block. The crate doc and `CLAUDE.md` advertise a
  `--no-default-features` "dump-only binary suitable for AST inspection," but a
  catalog-load failure exits with `USAGE` before any AST prints — so `--dump-ast`
  actually depends on a loadable runtime catalog. (Note: sema also consumes the
  catalog, so it can't simply be skipped; pure AST printing needs only the arena.)
- Fix: move the `--dump-ast` print ahead of catalog load + sema (it needs only
  `arena`/`program`), or update the docs to state that even dump-only builds
  require a loadable catalog. The former matches documented intent.

#### DR-R2 — `string_api()` panics while `load_catalog()` returns `Err` on the same conditions
- `cb-runtime-sys/src/lib.rs:183-200` (`string_api`) vs `:238-244` (`load_catalog`), `:256-262` (`decode_catalog`)
- Category: Inconsistency / Duplication
- Both call `cb_runtime_get_catalog()` and null-check it. On a null catalog or
  version mismatch, `string_api` **panics** while `load_catalog` returns
  `Err(String)` — same conditions, opposite handling. The version check is also
  duplicated with two differently-worded messages (folds in DR-R3). FD-024
  documents fatal-by-panic at init, so the divergence is partly intentional but
  undocumented at the call site.
- Fix: factor a private `fn catalog() -> Result<&'static CbCatalog, String>`
  (fetch + null-check + version-check, one message) used by both; document at
  `string_api` why startup failure is fatal-by-panic (cite FD-024) while
  catalog-load returns an error.

#### DR-D6 — `source.rs` test module is an empty stub; the crate's subtlest code is untested
- `cb-diagnostics/src/source.rs:296-297` (`#[cfg(test)] mod tests {}`)
- Category: Oversight
- `LineIndex` (CRLF vs bare `\r` vs `\n`, `partition_point` lookup, clamping) and
  `offset_to_line_char_col` (multi-byte columns, mid-codepoint flooring) are the
  most off-by-one-prone code in the crate yet have no direct tests; `render.rs`
  tests exercise emit paths but not the line/col arithmetic.
- Fix: add unit tests — CRLF as one terminator, bare `\r`, empty file
  (`line_count == 1`), offset-past-EOF clamping, char-column on a multi-byte
  line, and the mid-codepoint flooring path.

### Low — Diagnostics

- **DR-D1** — Span `end >= start` enforced in two layers with divergent teeth
  (`Span::new` debug-only vs `validate_label` all-builds); add a cross-reference.
  `diagnostic.rs:26-29`, `render.rs:108`.
- **DR-D2** — `len()` (saturating) and `is_empty()` (`start == end`) contradict
  each other on an inverted span; neither is `#[must_use]`. `diagnostic.rs:31-39`.
- **DR-D3** — Duplicated "exhaustion sentinel" boilerplate in Interner and
  SourceMap; factor `alloc_id(len, what) -> u32`. `intern.rs:92-98`, `source.rs:142-152`.
- **DR-D4** — `Interner::intern` overflow guard is mis-ordered; the `try_from`
  expect is unreachable in the boundary case it claims to guard. `intern.rs:92-98`.
- **DR-D5** — `offset_to_line_char_col` relies on a correct-but-unasserted
  invariant that `byte_col` stays within the line (no defensive clamp, untested).
  `source.rs:54-73`.
- **DR-D7** — `validate_label` recomputes `text_len` that `LineIndex::text_len()`
  already stores. `render.rs:124-125`.
- **DR-D8** — `validate_label` does not reject mid-codepoint span bounds
  (asymmetry with `offset_to_line_char_col`); codespan tolerates it, so document
  the trust assumption. `render.rs:107-135`.
- **DR-D9** — `Renderer` doc's two-bucket error story (`InvalidInput` vs
  `InvalidData`) doesn't match `emit`, which maps every non-`Io` error to
  `InvalidData`. `render.rs:22-28` vs `:84-91`.
- **DR-D10** — `eprintln!` side-channel baked into the renderer surfaces the
  error twice and adds unsuppressable stderr noise for LSP/JSON consumers.
  `render.rs:85, 113, 121, 131`.
- **DR-D11** — `Interner::resolve` raw-indexes: `DUMMY` panics with a generic
  out-of-bounds message, and a cross-interner `Symbol` silently misresolves.
  `intern.rs:116-118`.
- **DR-D12** — `fold` allocates a throwaway `String` on every `intern`, including
  cache hits (lexer hot path). `intern.rs:36-38`.
- **DR-D13** — `LineIndex` doc "`newline_offsets[i]` … line `i + 2`" mixes 0- and
  1-based numbering; adjacent methods do too. `source.rs:173-176`.
- **DR-D14** — `SourceMap::add` does an O(n) name scan plus full-text compare on
  duplicate names (O(n²) for many files); undocumented. `source.rs:108-124`.

### Low — Runtime / Driver / LLVM

- **DR-R3** — Version mismatch validated twice with two different messages (folds
  into DR-R2). `cb-runtime-sys/src/lib.rs:190-194, 257-262`.
- **DR-R4** — `CbFuncDesc::flags` is decoded nowhere; the interp-specific "drains
  the trap channel after every call" rationale doesn't generalize, so an LLVM
  backend wanting `CB_FUNC_CAN_TRAP` would need catalog re-plumbing. Note this
  for when LLVM lands. `cb-runtime-sys/src/lib.rs:41-45`.
- **DR-R5** — `decode_catalog` doc claims it validates "uniqueness," but only
  type-*tag* uniqueness is enforced (names/symbols are deliberately allowed to
  collide). Reword to "tag uniqueness". `cb-runtime-sys/src/lib.rs:246-249, 289`.
- **DR-R6** — `cb-backend-llvm` is an optional dep the driver never references;
  the `llvm` feature only flips `HAS_LLVM` and adds the enum variant. Comment
  that the dep is wired ahead of codegen. `cb-driver/src/main.rs:252-259`.
- **DR-R7** — `exit::BACKEND_UNIMPLEMENTED` (code 3) is `#[cfg(feature = "llvm")]`
  but documented as an unconditional exit-code contract. `cb-driver/src/main.rs:36-37`.
- **DR-R8** — `strip_unc` non-UTF-8 fallback is moot — the path dies at a later
  `.to_str().unwrap()` anyway. `cb-runtime-sys/build.rs:6-17` vs `:188`.
- **DR-R9** — The empty-config `cb_runtime_link_libs_.txt` candidate is a
  fragile, undocumented guess at CMake's `$<CONFIG>` filename scheme.
  `cb-runtime-sys/build.rs:229-242`.
- **DR-R10** — No materialized `Backend` trait seam yet; the driver dispatches
  each backend via cfg'd `match` arms. Establish the trait (likely in `cb-ir`)
  before LLVM codegen begins. `cb-backend-llvm/src/lib.rs:1-4`.

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
  genuinely excludes `RuntimeType` (kept as the clarity note S-L6).
