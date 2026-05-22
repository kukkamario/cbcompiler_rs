# FD-002: Parser

**Status:** Complete
**Priority:** High
**Effort:** High (> 4 hours)
**Impact:** Second stage of the frontend — turns the lexer's token stream into an AST that semantic analysis and IR lowering can consume. Without it, nothing past the lexer can land.

## Problem

FD-001 produced a recovering token stream (`Vec<Token>` + `Vec<Diagnostic>`) but no AST. CoolBasic's grammar has several quirks that need explicit design before code lands:

- **Line-oriented statements.** Newlines (`Newline` tokens) terminate statements; `:` (`Punct::Colon`) is an inline statement separator. The parser has to treat both as terminators consistently — not skip them as trivia.
- **`Then` is sometimes a single-line marker.** `If x Then stmt` is a one-line `If` with no `EndIf`; `If x Then \n ... EndIf` is the block form. Disambiguation is a lookahead decision the parser owns.
- **Block constructs with paired `End...` keywords.** `Type/EndType`, `Struct/EndStruct`, `Function/EndFunction`, `If/EndIf`, `Select/EndSelect`. The lexer already emits both joined (`EndIf`) and split (`End` `If`) forms — the parser must accept both.
- **Sigils on identifiers carry type information.** `x%`, `name$` are `Ident { sigil: Some(_) }`. The AST needs to surface the sigil so sema can reconcile it with explicit `As Integer` annotations.
- **Expressions need a precedence model.** Arithmetic, comparison, and keyword operators (`And`/`Or`/`Xor`/`Not`/`Mod`/`Shl`/`Shr`/`Sar`/`BinAnd`/`BinOr`/`BinXor`/`BinNot`) all participate; precedence comes from `docs/cb_syntax.md` §5.1.
- **Error recovery has to keep going past a bad statement** — same constraint as the lexer, because the IDE needs an AST even for broken source. Synchronise on `Newline`/`Colon`/`End*` keywords.

## Solution

### Resolved decisions

- **Parser strategy.** Hand-written recursive descent for statements + Pratt for expressions. Matches the lexer's hand-written style, gives the most control over error recovery, no extra dep.
- **Authoritative grammar.** `docs/cb_syntax.md` is the single source of truth. Consult it before every parsing decision; if a corner case turns out to be undocumented, update the syntax doc (not this FD) first, then implement.
- **Error recovery synchronisation set.** `Newline`, `Punct::Colon`, and any `End*` keyword. On a parse error the parser drops tokens until it sees one of these, then resumes statement-level parsing.
- **Where the AST lives.** `pub mod ast` inside `cb-frontend`. Promote to a separate `cb-ast` crate only if `cb-ir` or another crate ends up sharing AST types.
- **AST representation: arena-allocated.** Each node lives in a contiguous `Vec<Node>` keyed by a `Copy` `NodeId`; children are stored as `NodeId`s rather than `Box`. Side tables (resolved types from sema, symbol-table entries, lint findings) attach as `Vec<T>` indexed by `NodeId`. Traversals take `&Arena`. The cost is plumbing — `impl Index<NodeId>`, two-step pattern matching (`arena[id]` then `match`) — and we accept it.

  Rationale: the design target is a compiler fast enough that LSP use does not need incremental reparsing. That means every edit reparses the whole file, and downstream passes (sema, IR lowering, diagnostics) will lean heavily on per-node side data. Arenas make both cheap: contiguous allocation keeps the full-file reparse fast (one bulk allocation, cache-friendly traversal, O(1) drop), and `Vec<T>` side tables keyed by `NodeId` are the natural shape for everything sema and IR will want to attach.

  Considered and rejected: **owned `Box<Expr>` tree** (idiomatic, but scattered allocations and awkward side-tables for sema metadata fight the perf goal); **lossless red-green à la `rowan`** (preserves trivia and enables incremental reparsing, but the design goal explicitly makes incremental reparsing unnecessary, and the boilerplate + untyped green layer are not worth it for a batch compiler).

  Implementation note: a hand-rolled `Vec<Node>` + newtype `NodeId(u32)` is likely enough; `id_arena` is fine if it removes boilerplate but is not required. Decide when writing `ast.rs`.

The implementation outline is roughly:

1. AST node types (`Expr`, `Stmt`, `Decl`, `Type`, paired with `Span` and node IDs).
2. Cursor-style parser over `&[Token]` skipping trivia; explicit handling of `Newline`/`Colon` as terminators.
3. Pratt expression parser keyed off `Op` and the keyword-operator `Kw`s.
4. Statement parsers for: assignment, `If/ElseIf/Else/EndIf` (both single-line and block), `While/Wend`, `Repeat/Forever`, `Repeat/While`, `For/To/Step/Next`, `For Each`, `Select/Case/Default/EndSelect`, `Function/EndFunction`, `Type/Field/EndType`, `Struct/Field/EndStruct`, `Dim`, `Redim`, `Const`, `Global`, `Return`, `Goto`, `Include`, `Break`, `Continue`.
5. Snapshot tests on `.cb` fixtures (extend the FD-001 fixture set; share the `tests/fixtures` directory) plus `proptest` for "lexer output is always parseable into *something* without panic, even on garbage".

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-frontend/src/ast.rs` | CREATE | Arena-backed AST node types with spans + `NodeId` |
| `crates/cb-frontend/src/parser.rs` (or module dir) | CREATE | Recursive-descent / Pratt parser |
| `crates/cb-frontend/src/lib.rs` | MODIFY | Re-export AST and `parse()` entry point |
| `crates/cb-frontend/tests/parser_units.rs` | CREATE | Per-construct unit tests |
| `crates/cb-frontend/tests/parser_snapshots.rs` | CREATE | `insta` snapshots over `.cb` fixtures |
| `crates/cb-frontend/tests/parser_props.rs` | CREATE | `proptest` no-panic / always-recovers properties |

## Verification

- `cargo test -p cb-frontend` green
- All existing FD-001 lexer fixtures parse to expected ASTs (snapshots)
- Error-recovery fixtures: malformed source still produces an AST plus diagnostics, no panics
- Round-trip pretty-printer over the AST regenerates equivalent source for clean fixtures (deferred to a follow-up FD if too large here)

## Related

- `docs/cb_syntax.md` — authoritative language reference for parsing decisions
- `docs/features/archive/FD-001_LEXER.md` — token stream this parser consumes
- Future: FDs for semantic analysis (name resolution, type checking) and IR lowering, both blocked on this
