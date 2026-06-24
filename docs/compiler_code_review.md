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
| Sema (check / lower / types / scope) | 0 | 0 | 20 |
| Frontend (lexer / parser / AST) | 0 | 0 | 27 |
| IR + Interpreter | 0 | 0 | 24 |
| Diagnostics + Runtime/Driver/LLVM | 0 | 0 | 20 |

> **Resolved findings have been removed from this report.** Fixed and dropped:
> the four High-severity `cb-sema` miscompiles (S-H1–S-H4); the "Bundle 1" sema
> validation gaps (S-M1–S-M5); the "Bundle 2" fail-loud robustness items (S-M6,
> S-M7, S-M8, II-V1, II-V2, II-V3 — except the still-deferred `Call`-result/
> signature cross-check — II-V20, II-V27); the "Bundle 3" behavioral
> inconsistencies (DR-R1, DR-R2/DR-R3, II-V26); the "Bundle 4" de-dup sweep
> + AST consolidation (S-M11, S-M12, S-M13, S-M14, II-V10, F-L1, F-L3, F-P2,
> F-A1, F-A2, F-A3, and the F-L6 spec reconciliation); and the "Bundle 5" Medium
> clarity/robustness sweep (S-M9, S-M10, F-L2, F-L4, F-L5, F-P1, F-P3, F-A4 — and
> the linked S-L6 resolved by S-M9's inline note). DR-D6 was a false positive
> and is recorded under [Confirmed non-issues](#confirmed-non-issues-checked-and-rejected).
> The findings below are the open remainder.

### Cross-cutting themes

These patterns recur across crates and are worth treating as systemic, not
one-off:

- **Invariants documented in prose far from the code that relies on them** —
  trap-channel single-slot reentrancy (II-V17), span ownership in FFI (II-V16),
  and the IR's reverse-postorder dominance assumption (II-V8). (The two-level
  scope tree, S-M10, and the deliberate `RuntimeType`/`is_reference()` split,
  S-M9, were of this kind and are now documented inline.)

---

## Sema (`cb-sema`)

> The High-severity (S-H1–S-H4), Bundle 1/2 (S-M1–S-M8), and Bundle 5 Medium
> (S-M9, S-M10) findings have been fixed and removed; the Low findings below are
> the open remainder.

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

> The Medium findings (F-L2, F-L4, F-L5, F-P1, F-P3, F-A4) have been fixed and
> removed; the Low findings below are the open remainder.

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
- **II-V11** — the shared `Value::to_i64`/`to_f64` non-numeric (`String` + `_`)
  arms silently return 0 when reached from the ffi marshaller (II-V10 unified the
  two old `value_as_*` copies onto these). They are dead under well-typed IR but
  would mask a type mismatch; consider a `debug_assert!`/`unreachable!` on the
  marshalling path. `value.rs` (`to_i64`/`to_f64`), `ffi.rs` (`marshal`).
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
  genuinely excludes `RuntimeType`; the overlap is now documented inline at the
  arm and on `Type::is_reference` (former clarity note S-L6, resolved with S-M9).
- **`LineIndex` / `offset_to_line_char_col` are well-tested** (withdrawn DR-D6) —
  the off-by-one-prone line/column arithmetic is covered by
  `crates/cb-diagnostics/tests/line_index.rs` (CRLF / bare-`\r` / LF terminators,
  past-EOF clamping, multi-byte char columns, FD-021 mid-codepoint flooring). The
  empty `mod tests {}` stub `source.rs` once carried — the basis of the original
  finding — was removed.
