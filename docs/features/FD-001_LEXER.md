# FD-001: Lexer

**Status:** Pending Verification
**Priority:** High
**Effort:** High (> 4 hours)
**Impact:** First stage of the compiler frontend — turns CoolBasic source text into a token stream consumable by the parser. Nothing else in the frontend can land until this exists.

## Problem

`cb-frontend` is currently an empty crate. CoolBasic source needs to be lexed into tokens before any parsing, semantic analysis, or IR work can begin. CoolBasic has several non-obvious lexical features (see `docs/cb_syntax.md`) that need to be designed in from the start rather than retrofitted:

- Type sigils on identifiers (`$`, `#`, etc.) — these are part of the identifier token, not separate punctuation.
- Keywords are case-insensitive (`If` / `IF` / `if` all match)
- Line-oriented grammar: newlines are significant; `:` is a statement separator
- Line continuation with `\` at end of line
- `'` and `Rem` comments to end of line
- String literals with `"..."`
- Numeric literals (integers, floats, possibly hex)
- `Type ... EndType` and other multi-word constructs are still lexed as separate keyword tokens; the parser composes them

## Solution

Hand-written lexer in `cb-frontend`. Design decisions, resolved with IDE-readiness, pluggable diagnostics, and low allocation as primary constraints:

### Token representation — flat `TokenKind` + `Span`, zero-allocation

```rust
#[derive(Copy, Clone)]
struct Token { kind: TokenKind, span: Span }

enum TokenKind {
    Ident,                 // lexeme recovered from source via span
    Keyword(Kw),           // small Copy enum
    IntLit(i64),           // value parsed inline (incl. $hex, 0b…, digit separators)
    FloatLit(f64),
    StrLit(StrLitKind),    // see below
    Sigil(Sigil),          // $ # % & etc. (attached to preceding ident at parse time, or carried as separate token — TBD per syntax doc)
    Punct(Punct),          // ( ) , : ; [ ] etc.
    Op(Op),                // + - * / = < > <= >= <> And Or …
    Newline,               // significant — statement terminator
    Continuation,          // `\` followed by line ending — usually skipped, kept for IDE/formatter fidelity
    Comment(CommentKind),  // ' or Rem — preserved so IDE tooling can see them
    Error(LexErrorKind),   // recoverable: lexer never aborts
    Eof,
}
```

- `Token` is `Copy`, ~16 bytes — passes by value, lives in `Vec<Token>` cheaply.
- No `String` per token. Identifier lexemes are recovered from source via `span` when needed; interning is deferred to the AST/sema stage (`string-interner`, returning `Symbol(u32)`).
- Numeric values are parsed once into `i64`/`f64` so the parser doesn't re-parse digit strings.
- Trivia (whitespace, comments, line continuations) is preserved as tokens, gated behind a `LexerOptions { preserve_trivia: bool }` flag — off for compilation, on for IDE/formatter use.

### String literal classification — `StrLitKind`

The lexer pre-classifies string literals so the parser/sema can fast-path the easy cases:

```rust
enum StrLitKind {
    Plain,    // single-line "...", no escapes — value is span minus the two quotes, no further processing
    Escaped,  // single-line "...", contains at least one `\` — needs C-escape unescape pass
    Raw,      // triple-quoted """…""" multi-line — no escapes, but needs common-indent stripping per cb_syntax.md §4.3
}
```

The lexer decides this on the fly (it already scans the body to find the closing delimiter; it just observes whether any `\` appeared and whether the open was `"""`). Lets the parser switch into the right unescape mode immediately without re-scanning, and lets `Plain` skip the unescape step entirely.

### Span strategy — byte offsets, line/col lazy

`Span { start: u32, end: u32, file: FileId }` — 12 bytes, `Copy`. Line/column derived on demand via a `LineIndex` (cached newline offsets) when a diagnostic is rendered. Faster lexer, smaller tokens, and `LineIndex` is also what an LSP needs to translate positions.

### Error handling — recoverable, error tokens + side-channel diagnostics

Lexer never returns `Result::Err`. Errors become `TokenKind::Error(LexErrorKind)` and structured diagnostics are pushed to a `Vec<Diagnostic>` carried on the lexer session. Required for LSP — the IDE must keep getting tokens past a malformed region.

### Diagnostics — own crate, pluggable renderers

Spin up a new crate `cb-diagnostics` (sibling of `cb-frontend`, no dependency on it) with a minimal core type:

```rust
struct Diagnostic {
    severity: Severity,
    code: Option<&'static str>,
    message: String,
    primary: Label,                 // Label { span: Span, message: Option<String> }
    secondary: Vec<Label>,
    notes: Vec<String>,
}

trait Renderer { fn emit(&mut self, diag: &Diagnostic, sources: &SourceMap); }
```

Frontend only produces structured diagnostics. Renderers plug in independently:
- CLI renderer wraps `codespan-reporting` (best terminal output today)
- LSP renderer maps to `lsp_types::Diagnostic`
- Test/JSON renderer for snapshot tests

This matches the project's "pluggable backend" ground rule: diagnostics is just another backend.

### Case-insensitivity — canonical lower, ASCII compare

Keyword table maps lowercase strings to `Kw`. Lookup uses `eq_ignore_ascii_case` (keywords are ASCII). `phf` for the table (compile-time perfect hash, no `HashMap` allocation, no startup cost).

### Implementation outline

1. `cb-diagnostics` crate scaffolded with `Diagnostic`, `Severity`, `Label`, `Renderer`, `SourceMap`/`FileId`/`LineIndex`. CLI renderer wrapping `codespan-reporting`.
2. `cb-frontend`: `span.rs`, `token.rs`, keyword table (`phf`).
3. `lexer.rs`: cursor-style scanner exposing `fn tokenize(src: &str, file: FileId, opts: LexerOptions) -> (Vec<Token>, Vec<Diagnostic>)`. Sub-scanners: identifier/keyword, numeric literal (incl. `$hex`, `0b…`, digit separators), single-line string (sets `Escaped` if `\` seen, else `Plain`), triple-quoted raw string (`Raw`), `'` and `Rem` comments, operator/punctuation, newline/whitespace/`\`-continuation.
4. Snapshot tests with `insta` against `.cb` fixtures.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-diagnostics/` | CREATE | New crate: `Diagnostic`, `Severity`, `Label`, `Renderer`, `SourceMap`, `FileId`, `LineIndex`. CLI renderer wrapping `codespan-reporting`. |
| `Cargo.toml` (workspace) | MODIFY | Add `cb-diagnostics` to workspace members |
| `crates/cb-frontend/Cargo.toml` | MODIFY | Depend on `cb-diagnostics`; add `phf` (keyword table) and dev-dep `insta` |
| `crates/cb-frontend/src/lib.rs` | MODIFY | `pub mod span; pub mod token; pub mod lexer;` |
| `crates/cb-frontend/src/span.rs` | CREATE | `Span { start: u32, end: u32, file: FileId }` (re-exports `FileId` from `cb-diagnostics`) |
| `crates/cb-frontend/src/token.rs` | CREATE | `Token`, `TokenKind`, `Kw`, `Op`, `Punct`, `Sigil`, `StrLitKind`, `CommentKind`, `LexErrorKind`, keyword table |
| `crates/cb-frontend/src/lexer.rs` | CREATE | Cursor scanner, `tokenize()` entry point, sub-scanners |
| `crates/cb-frontend/tests/lexer/` | CREATE | `insta` snapshot tests against `.cb` fixtures |

## Verification

- `cargo test -p cb-frontend` and `cargo test -p cb-diagnostics` pass
- Hand-curated `.cb` fixtures lex to expected token sequences (`insta` snapshots)
- Edge cases covered:
  - Case-insensitive keywords (`If` / `IF` / `if`)
  - Line continuation `\` at end of line (per `cb_syntax.md:12`)
  - `'` and `Rem` comments to end of line
  - `:` statement separator
  - Type sigils on identifiers (`$`, `#`, `%`, `&`, …)
  - Integer literals: decimal, `$hex`, `0b…` binary, **underscore digit separators** (`1_000`, `$dead_beef`), and invalid-separator-position errors (`$_ff`, `1__000`)
  - Float literals with digit separators (`1_000.5`)
  - `StrLitKind::Plain`: `"hello"` lexes with no escape pass needed
  - `StrLitKind::Escaped`: `"a\nb"` flagged so the parser invokes unescape
  - `StrLitKind::Raw`: triple-quoted `"""…"""` multi-line literal
  - Literal newline inside `"…"` produces a recoverable error token
- Error cases produce structured `Diagnostic`s with correct spans (e.g., unterminated string literal, invalid digit separator) — and the lexer continues past them rather than aborting
- Trivia mode: with `preserve_trivia: true`, the round-tripped lexeme sequence reconstructs source byte-for-byte

## Implementation notes

Decisions made during implementation that resolve open points in this FD:

- **Comments.** The lexer follows `cb_syntax.md` §1.2: `//` and `REM` line comments, plus nested `/* */` block comments. The earlier mention of `'` tick comments in this FD was a holdover from classic BASIC dialects and is **not** in the CoolBasic spec — dropped.
- **Sigil token representation.** Sigils (`%`, `#`, `$`, `!`) are part of the `Ident` token: `TokenKind::Ident { sigil: Option<Sigil> }`. The sigil's byte is included in the token span. This is cleaner for the parser (one token per variable reference) and matches the semantic model that `x` and `x%` are the same name with a type annotation.
- **Keyword + sigil precedence.** Keyword lookup runs *before* the sigil peek in `scan_ident`. So `If%` lexes as `Keyword(If)` followed by `%` (which on its own is a malformed binary literal). Keywords never carry sigils.
- **Raw string indent stripping deferred.** The lexer classifies triple-quoted strings as `StrLitKind::Raw` and finds the closing `"""`, but the common-indent-strip rule from `cb_syntax.md` §1.6 is left to the parser/sema pass — keeping the lexer allocation-free.
- **`Else If` / `End If` stay as two tokens.** The lexer emits separate `Keyword` tokens for the split forms; composition is the parser's job. The joined forms (`ElseIf`, `EndIf`) are single keywords.
- **Error recovery is exhaustive.** Every `LexErrorKind` path advances the cursor by at least one byte. The lexer never returns `Err`; structured `Diagnostic`s are side-channeled. Error codes assigned: `E0101` newline-in-string, `E0102` unterminated string, `E0103` unterminated block comment, `E0104` number overflow, `E0105` invalid digit separator, `E0106` invalid/unexpected char, `E0107` malformed number (e.g. exponent without digits).
- **Diagnostics live in `cb-diagnostics`.** New sibling crate housing `Diagnostic`, `Severity`, `Label`, `Renderer`, `SourceMap`, `FileId`, `LineIndex`, and a `CliRenderer` wrapping `codespan-reporting`. `cb-frontend` depends on it; future LSP/JSON renderers plug in without touching the frontend.
- **Test coverage.** 80 hand-written unit tests, 7 `insta` snapshot fixtures, 4 `proptest` properties (round-trip with `preserve_trivia`, no-panic, single-Eof terminator, determinism), 6 `LineIndex` unit tests. `cargo test --workspace` is green; `cargo clippy --workspace --all-targets -- -D warnings` is clean; `cargo fmt --all --check` is clean.

## Related

- `docs/cb_syntax.md` — authoritative reference for what tokens exist
- Future: FD for parser (depends on this), FD for diagnostics infrastructure (likely spun out)
