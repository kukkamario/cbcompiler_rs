//! Per-sub-scanner unit tests for the CoolBasic lexer.
//!
//! Tests assert against the public API (`tokenize` / `LexerOptions`). Helper
//! functions at the top of the file keep the per-module assertions terse.

use cb_diagnostics::Diagnostic;
use cb_frontend::span::FileId;
use cb_frontend::{LexerOptions, Token, TokenKind, tokenize};

fn lex(src: &str) -> Vec<Token> {
    let (tokens, diags) = tokenize(src, FileId(0), LexerOptions::default());
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    tokens
}

fn lex_with_diags(src: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    tokenize(src, FileId(0), LexerOptions::default())
}

fn lex_trivia(src: &str) -> Vec<Token> {
    let (tokens, diags) = tokenize(
        src,
        FileId(0),
        LexerOptions {
            preserve_trivia: true,
        },
    );
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    tokens
}

fn kinds(tokens: &[Token]) -> Vec<TokenKind> {
    tokens.iter().map(|t| t.kind).collect()
}

fn float_value(t: Token) -> f64 {
    match t.kind {
        TokenKind::FloatLit(v) => v,
        other => panic!("expected FloatLit, got {other:?}"),
    }
}

fn int_value(t: Token) -> i64 {
    match t.kind {
        TokenKind::IntLit(v) => v,
        other => panic!("expected IntLit, got {other:?}"),
    }
}

fn approx_eq(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    if a.is_finite() && b.is_finite() {
        let scale = a.abs().max(b.abs()).max(1.0);
        (a - b).abs() < 1e-9 * scale
    } else {
        false
    }
}

mod ident {
    use super::*;
    use cb_frontend::{Sigil, TokenKind};

    #[test]
    fn plain_ascii_ident() {
        let toks = lex("foo");
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Ident { sigil: None }, TokenKind::Eof]
        );
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 3);
    }

    #[test]
    fn underscore_lead() {
        let toks = lex("_total");
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Ident { sigil: None }, TokenKind::Eof]
        );
        assert_eq!(toks[0].span.end, 6);
    }

    #[test]
    fn lone_underscore_is_ident() {
        let toks = lex("_");
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Ident { sigil: None }, TokenKind::Eof]
        );
    }

    #[test]
    fn underscore_with_digits() {
        let toks = lex("_123");
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Ident { sigil: None }, TokenKind::Eof]
        );
        assert_eq!(toks[0].span.end, 4);
    }

    #[test]
    fn non_ascii_ident_with_multibyte_char() {
        // résumé2 — 'é' is 2 bytes in UTF-8 (0xC3 0xA9). String length = 9 bytes.
        let src = "résumé2";
        assert_eq!(src.len(), 9);
        let toks = lex(src);
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Ident { sigil: None }, TokenKind::Eof]
        );
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 9, "span should cover all 9 bytes");
    }

    #[test]
    fn ident_with_integer_sigil() {
        let toks = lex("x%");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident {
                    sigil: Some(Sigil::Integer)
                },
                TokenKind::Eof
            ]
        );
        // Sigil byte INCLUDED in span.
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 2);
    }

    #[test]
    fn ident_with_float_sigil() {
        let toks = lex("x#");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident {
                    sigil: Some(Sigil::Float)
                },
                TokenKind::Eof
            ]
        );
        assert_eq!(toks[0].span.end, 2);
    }

    #[test]
    fn ident_with_string_sigil() {
        let toks = lex("x$");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident {
                    sigil: Some(Sigil::String)
                },
                TokenKind::Eof
            ]
        );
        assert_eq!(toks[0].span.end, 2);
    }

    #[test]
    fn ident_with_bool_sigil() {
        let toks = lex("x!");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident {
                    sigil: Some(Sigil::Bool)
                },
                TokenKind::Eof
            ]
        );
        assert_eq!(toks[0].span.end, 2);
    }

    #[test]
    fn keyword_does_not_take_sigil() {
        // `If%` — `If` is a keyword, the `%` cannot attach. The first token
        // should be Keyword(If), span 0..2 (i.e. NOT including the `%`).
        let (toks, _diags) = lex_with_diags("If%");
        // First token is Keyword(If) with no sigil.
        match toks[0].kind {
            TokenKind::Keyword(cb_frontend::Kw::If) => {}
            other => panic!("first token expected Keyword(If), got {other:?}"),
        }
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 2, "span must not include trailing %");
    }

    #[test]
    fn digit_prefix_breaks_ident() {
        // `2cool` — first token must be IntLit(2). The lexer breaks on the
        // letter; second token will be Ident("cool").
        let toks = lex("2cool");
        match toks[0].kind {
            TokenKind::IntLit(2) => {}
            other => panic!("expected IntLit(2), got {other:?}"),
        }
    }

    #[test]
    fn hyphen_does_not_join_idents() {
        // `my-var` → Ident("my") Op::Minus Ident("var")
        let toks = lex("my-var");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident { sigil: None },
                TokenKind::Op(cb_frontend::Op::Minus),
                TokenKind::Ident { sigil: None },
                TokenKind::Eof
            ]
        );
    }
}

mod keywords {
    use super::*;
    use cb_frontend::Kw;

    #[test]
    fn case_insensitive_if() {
        for src in ["If", "IF", "if", "iF"] {
            let toks = lex(src);
            assert_eq!(
                kinds(&toks),
                vec![TokenKind::Keyword(Kw::If), TokenKind::Eof],
                "case variant `{src}` should lex to Keyword(If)"
            );
        }
    }

    #[test]
    fn int_and_integer_alias_to_same_keyword() {
        let int_toks = lex("Int");
        let integer_toks = lex("Integer");
        assert_eq!(
            kinds(&int_toks),
            vec![TokenKind::Keyword(Kw::Integer), TokenKind::Eof]
        );
        assert_eq!(
            kinds(&integer_toks),
            vec![TokenKind::Keyword(Kw::Integer), TokenKind::Eof]
        );
    }

    #[test]
    fn uint_and_uinteger_alias_to_same_keyword() {
        let uint_toks = lex("UInt");
        let uinteger_toks = lex("UInteger");
        assert_eq!(
            kinds(&uint_toks),
            vec![TokenKind::Keyword(Kw::UInteger), TokenKind::Eof]
        );
        assert_eq!(
            kinds(&uinteger_toks),
            vec![TokenKind::Keyword(Kw::UInteger), TokenKind::Eof]
        );
    }

    #[test]
    fn else_if_two_tokens_vs_elseif_single_token() {
        let split = lex("Else If");
        assert_eq!(
            kinds(&split),
            vec![
                TokenKind::Keyword(Kw::Else),
                TokenKind::Keyword(Kw::If),
                TokenKind::Eof,
            ]
        );

        let joined = lex("ElseIf");
        assert_eq!(
            kinds(&joined),
            vec![TokenKind::Keyword(Kw::ElseIf), TokenKind::Eof]
        );
    }

    #[test]
    fn end_function_split_and_joined() {
        let split = lex("End Function");
        assert_eq!(
            kinds(&split),
            vec![
                TokenKind::Keyword(Kw::End),
                TokenKind::Keyword(Kw::Function),
                TokenKind::Eof,
            ]
        );
        let joined = lex("EndFunction");
        assert_eq!(
            kinds(&joined),
            vec![TokenKind::Keyword(Kw::EndFunction), TokenKind::Eof]
        );
    }

    #[test]
    fn end_if_split_and_joined() {
        let split = lex("End If");
        assert_eq!(
            kinds(&split),
            vec![
                TokenKind::Keyword(Kw::End),
                TokenKind::Keyword(Kw::If),
                TokenKind::Eof,
            ]
        );
        let joined = lex("EndIf");
        assert_eq!(
            kinds(&joined),
            vec![TokenKind::Keyword(Kw::EndIf), TokenKind::Eof]
        );
    }

    #[test]
    fn end_select_split_and_joined() {
        let split = lex("End Select");
        assert_eq!(
            kinds(&split),
            vec![
                TokenKind::Keyword(Kw::End),
                TokenKind::Keyword(Kw::Select),
                TokenKind::Eof,
            ]
        );
        let joined = lex("EndSelect");
        assert_eq!(
            kinds(&joined),
            vec![TokenKind::Keyword(Kw::EndSelect), TokenKind::Eof]
        );
    }

    #[test]
    fn end_struct_split_and_joined() {
        let split = lex("End Struct");
        assert_eq!(
            kinds(&split),
            vec![
                TokenKind::Keyword(Kw::End),
                TokenKind::Keyword(Kw::Struct),
                TokenKind::Eof,
            ]
        );
        let joined = lex("EndStruct");
        assert_eq!(
            kinds(&joined),
            vec![TokenKind::Keyword(Kw::EndStruct), TokenKind::Eof]
        );
    }

    #[test]
    fn end_type_split_and_joined() {
        let split = lex("End Type");
        assert_eq!(
            kinds(&split),
            vec![
                TokenKind::Keyword(Kw::End),
                TokenKind::Keyword(Kw::Type),
                TokenKind::Eof,
            ]
        );
        let joined = lex("EndType");
        assert_eq!(
            kinds(&joined),
            vec![TokenKind::Keyword(Kw::EndType), TokenKind::Eof]
        );
    }

    #[test]
    fn continue_is_keyword_not_comment() {
        let toks = lex("Continue");
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Keyword(Kw::Continue), TokenKind::Eof]
        );
    }

    #[test]
    fn ident_followed_by_keyword() {
        let toks = lex("myFunc If");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident { sigil: None },
                TokenKind::Keyword(Kw::If),
                TokenKind::Eof,
            ]
        );
    }
}

mod numbers {
    use super::*;

    #[test]
    fn decimal_zero() {
        let toks = lex("0");
        assert_eq!(int_value(toks[0]), 0);
    }

    #[test]
    fn decimal_small() {
        let toks = lex("123");
        assert_eq!(int_value(toks[0]), 123);
    }

    #[test]
    fn decimal_with_separators() {
        let toks = lex("1_000");
        assert_eq!(int_value(toks[0]), 1000);
        let toks = lex("5_342_100");
        assert_eq!(int_value(toks[0]), 5_342_100);
    }

    #[test]
    fn hex_basic() {
        let toks = lex("$2f4E4");
        assert_eq!(int_value(toks[0]), 0x2F4E4);
    }

    #[test]
    fn hex_with_separators() {
        let toks = lex("$dead_beef");
        assert_eq!(int_value(toks[0]), 0xDEAD_BEEFi64);
    }

    #[test]
    fn binary_basic() {
        let toks = lex("%1010");
        assert_eq!(int_value(toks[0]), 10);
        let toks = lex("%1");
        assert_eq!(int_value(toks[0]), 1);
    }

    #[test]
    fn float_basic_no_exponent() {
        let toks = lex("0.23");
        assert!(approx_eq(float_value(toks[0]), 0.23));

        let toks = lex("23.205421");
        assert!(approx_eq(float_value(toks[0]), 23.205421));
    }

    #[test]
    fn float_with_positive_exponent_implicit_sign() {
        let toks = lex("12.4e23");
        assert!(approx_eq(float_value(toks[0]), 12.4e23));
    }

    #[test]
    fn float_with_negative_exponent() {
        let toks = lex("1.0e-7");
        assert!(approx_eq(float_value(toks[0]), 1.0e-7));
    }

    #[test]
    fn float_with_positive_signed_exponent() {
        let toks = lex("6.022e+23");
        assert!(approx_eq(float_value(toks[0]), 6.022e23));
    }

    #[test]
    fn float_with_separators() {
        let toks = lex("1_000.5");
        assert!(approx_eq(float_value(toks[0]), 1000.5));
    }

    #[test]
    fn ident_with_integer_sigil_not_binary_literal() {
        // `x%` is Ident with Integer sigil — not an ident followed by %.
        use cb_frontend::Sigil;
        let toks = lex("x%");
        assert_eq!(
            toks[0].kind,
            TokenKind::Ident {
                sigil: Some(Sigil::Integer)
            }
        );
    }

    #[test]
    fn percent_at_start_is_binary_literal() {
        let toks = lex("%10");
        assert_eq!(int_value(toks[0]), 2);
    }

    #[test]
    fn ident_with_sigil_then_number() {
        // `x%10` → Ident { sigil: Integer } then IntLit(10)
        use cb_frontend::Sigil;
        let toks = lex("x%10");
        assert_eq!(
            toks[0].kind,
            TokenKind::Ident {
                sigil: Some(Sigil::Integer)
            }
        );
        assert_eq!(int_value(toks[1]), 10);
    }

    /// Per spec §1.6: `.5` is NOT a valid float; it lexes as Dot then IntLit(5).
    #[test]
    fn leading_dot_not_a_float() {
        let toks = lex(".5");
        use cb_frontend::Punct;
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Punct(Punct::Dot),
                TokenKind::IntLit(5),
                TokenKind::Eof,
            ]
        );
    }

    /// Per spec §1.6: `5.` is NOT a valid float; it lexes as IntLit(5) then Dot.
    #[test]
    fn trailing_dot_not_a_float() {
        let toks = lex("5.");
        use cb_frontend::Punct;
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::IntLit(5),
                TokenKind::Punct(Punct::Dot),
                TokenKind::Eof,
            ]
        );
    }
}

mod strings {
    use super::*;
    use cb_frontend::StrLitKind;

    #[test]
    fn plain_string() {
        let toks = lex("\"hello\"");
        assert_eq!(toks[0].kind, TokenKind::StrLit(StrLitKind::Plain));
        // Span covers including quotes.
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 7);
    }

    #[test]
    fn escaped_string_has_backslash() {
        // The lexer just classifies; it doesn't unescape. `\n` flips the kind
        // to Escaped.
        let toks = lex("\"a\\nb\"");
        assert_eq!(toks[0].kind, TokenKind::StrLit(StrLitKind::Escaped));
    }

    #[test]
    fn empty_string() {
        let toks = lex("\"\"");
        assert_eq!(toks[0].kind, TokenKind::StrLit(StrLitKind::Plain));
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 2);
    }

    #[test]
    fn triple_quoted_with_literal_backslash_n_on_one_line() {
        // Raw — backslashes are NOT escapes inside `"""…"""`.
        let toks = lex("\"\"\"line1\\nline2\"\"\"");
        assert_eq!(toks[0].kind, TokenKind::StrLit(StrLitKind::Raw));
    }

    #[test]
    fn triple_quoted_multiline() {
        let src = "\"\"\"\nfoo\nbar\n\"\"\"";
        let toks = lex(src);
        assert_eq!(toks[0].kind, TokenKind::StrLit(StrLitKind::Raw));
    }

    #[test]
    fn plain_with_spaces() {
        let toks = lex("\"hello world\"");
        assert_eq!(toks[0].kind, TokenKind::StrLit(StrLitKind::Plain));
    }
}

mod comments {
    use super::*;
    use cb_frontend::CommentKind;

    #[test]
    fn line_comment_trivia() {
        let toks = lex_trivia("// to end of line\nfoo");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Comment(CommentKind::Line),
                TokenKind::Newline,
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn line_comment_without_trivia_discarded() {
        let toks = lex("// to end of line\nfoo");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Newline,
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn rem_comment_uppercase() {
        let toks = lex_trivia("REM stuff\nfoo");
        assert_eq!(toks[0].kind, TokenKind::Comment(CommentKind::Line));
    }

    #[test]
    fn rem_comment_lowercase() {
        let toks = lex_trivia("rem stuff\nfoo");
        assert_eq!(toks[0].kind, TokenKind::Comment(CommentKind::Line));
    }

    #[test]
    fn rem_comment_mixed_case() {
        let toks = lex_trivia("Rem stuff\nfoo");
        assert_eq!(toks[0].kind, TokenKind::Comment(CommentKind::Line));
    }

    #[test]
    fn simple_block_comment() {
        let toks = lex_trivia("/* simple */");
        // One Block comment, then Eof.
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Comment(CommentKind::Block), TokenKind::Eof]
        );
    }

    #[test]
    fn nested_block_comment_one_token() {
        // `/* outer /* inner */ still */ ok` — ONE block comment then Ident("ok").
        let toks = lex_trivia("/* outer /* inner */ still */ ok");
        assert_eq!(toks[0].kind, TokenKind::Comment(CommentKind::Block));
        // After the block comment, we have whitespace (trivia mode) then ident.
        // The trailing token before Eof must be the ident.
        let non_ws: Vec<_> = toks
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Whitespace))
            .collect();
        assert_eq!(
            non_ws.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![
                TokenKind::Comment(CommentKind::Block),
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unterminated_block_comment_produces_error_and_diag() {
        use cb_frontend::LexErrorKind;
        let (toks, diags) = lex_with_diags("/* unclosed");
        let last_real = toks
            .iter()
            .rev()
            .find(|t| !matches!(t.kind, TokenKind::Eof));
        assert!(
            matches!(
                last_real.map(|t| t.kind),
                Some(TokenKind::Error(LexErrorKind::UnterminatedBlockComment))
            ),
            "expected Error(UnterminatedBlockComment), got tokens={toks:?}"
        );
        assert!(
            diags.iter().any(|d| d.code == Some("E0103")),
            "expected at least one E0103 diagnostic, got {diags:?}"
        );
    }
}

mod operators {
    use super::*;
    use cb_frontend::{Op, Punct};

    fn expect_kinds(src: &str, expected: &[TokenKind]) {
        let toks = lex(src);
        let got = kinds(&toks);
        let mut want: Vec<TokenKind> = expected.to_vec();
        want.push(TokenKind::Eof);
        assert_eq!(got, want, "for source {src:?}");
    }

    #[test]
    fn single_char_arithmetic() {
        expect_kinds("+", &[TokenKind::Op(Op::Plus)]);
        expect_kinds("-", &[TokenKind::Op(Op::Minus)]);
        expect_kinds("*", &[TokenKind::Op(Op::Star)]);
        expect_kinds("/", &[TokenKind::Op(Op::Slash)]);
    }

    #[test]
    fn star_star_is_one_token() {
        expect_kinds("**", &[TokenKind::Op(Op::StarStar)]);
    }

    #[test]
    fn equal_and_relational() {
        expect_kinds("=", &[TokenKind::Op(Op::Eq)]);
        expect_kinds("<", &[TokenKind::Op(Op::Lt)]);
        expect_kinds(">", &[TokenKind::Op(Op::Gt)]);
        expect_kinds("<=", &[TokenKind::Op(Op::LtEq)]);
        expect_kinds(">=", &[TokenKind::Op(Op::GtEq)]);
        expect_kinds("<>", &[TokenKind::Op(Op::NotEq)]);
    }

    #[test]
    fn punctuation() {
        expect_kinds("(", &[TokenKind::Punct(Punct::LParen)]);
        expect_kinds(")", &[TokenKind::Punct(Punct::RParen)]);
        expect_kinds("[", &[TokenKind::Punct(Punct::LBracket)]);
        expect_kinds("]", &[TokenKind::Punct(Punct::RBracket)]);
        expect_kinds(",", &[TokenKind::Punct(Punct::Comma)]);
        expect_kinds(":", &[TokenKind::Punct(Punct::Colon)]);
        expect_kinds(";", &[TokenKind::Punct(Punct::Semicolon)]);
        expect_kinds(".", &[TokenKind::Punct(Punct::Dot)]);
    }

    #[test]
    fn lone_backslash_in_expression_context() {
        // `a \ b` — the `\` is not followed by a line terminator, so it should
        // be Op::BackSlash, not a continuation.
        let toks = lex("a \\ b");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident { sigil: None },
                TokenKind::Op(Op::BackSlash),
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lone_backslash_at_eof_is_op_backslash() {
        // Pin down the EOF case: spec is silent, current behaviour is to emit
        // a lone Op::BackSlash. If this ever changes (e.g. becomes an error),
        // it should be a deliberate spec update, not a silent regression.
        let toks = lex("\\");
        let kinds = kinds(&toks);
        assert!(matches!(kinds[0], TokenKind::Op(Op::BackSlash)));
        assert!(matches!(kinds[1], TokenKind::Eof));
    }
}

mod trivia {
    use super::*;

    #[test]
    fn whitespace_preserved() {
        let toks = lex_trivia("  foo");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Whitespace,
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tab_whitespace_preserved_between_idents() {
        let toks = lex_trivia("foo\tbar");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident { sigil: None },
                TokenKind::Whitespace,
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn line_continuation_in_trivia_mode() {
        // `a = 1 + \\\n  2` (one backslash + newline + indented `2`).
        // Expect a Continuation token, NO Newline for that line.
        let toks = lex_trivia("a = 1 + \\\n  2");
        let kinds = kinds(&toks);
        assert!(
            kinds.contains(&TokenKind::Continuation),
            "expected Continuation in {kinds:?}"
        );
        // No Newline tokens — the continuation swallowed the line ending.
        assert!(
            !kinds.contains(&TokenKind::Newline),
            "expected no Newline in continuation case, got {kinds:?}"
        );
    }

    #[test]
    fn line_continuation_without_trivia() {
        // Same input, no trivia: also no Newline.
        let toks = lex("a = 1 + \\\n  2");
        let kinds = kinds(&toks);
        assert!(
            !kinds.contains(&TokenKind::Newline),
            "expected no Newline in continuation case, got {kinds:?}"
        );
        // And no Continuation either (suppressed).
        assert!(
            !kinds.contains(&TokenKind::Continuation),
            "Continuation should be suppressed without preserve_trivia"
        );
    }

    #[test]
    fn whitespace_suppressed_without_trivia() {
        let toks = lex("  foo");
        assert_eq!(
            kinds(&toks),
            vec![TokenKind::Ident { sigil: None }, TokenKind::Eof]
        );
    }
}

mod newlines {
    use super::*;

    #[test]
    fn lf_newline_one_byte() {
        let toks = lex("\n");
        assert_eq!(toks[0].kind, TokenKind::Newline);
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 1);
    }

    #[test]
    fn crlf_newline_two_bytes() {
        let toks = lex("\r\n");
        assert_eq!(toks[0].kind, TokenKind::Newline);
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 2);
    }

    #[test]
    fn bare_cr_newline_one_byte() {
        let toks = lex("\r");
        assert_eq!(toks[0].kind, TokenKind::Newline);
        assert_eq!(toks[0].span.start, 0);
        assert_eq!(toks[0].span.end, 1);
    }

    #[test]
    fn ident_newline_ident() {
        let toks = lex("a\nb");
        assert_eq!(
            kinds(&toks),
            vec![
                TokenKind::Ident { sigil: None },
                TokenKind::Newline,
                TokenKind::Ident { sigil: None },
                TokenKind::Eof,
            ]
        );
    }
}

mod bom {
    use super::*;

    #[test]
    fn bom_consumed_before_first_token() {
        // BOM is U+FEFF, encoded as 3 bytes in UTF-8: EF BB BF.
        let src = "\u{FEFF}foo";
        let toks = lex(src);
        assert_eq!(toks[0].kind, TokenKind::Ident { sigil: None });
        assert_eq!(
            toks[0].span.start, 3,
            "first token should start after the 3-byte BOM"
        );
        assert_eq!(toks[0].span.end, 6);
    }
}

mod errors {
    use super::*;
    use cb_frontend::LexErrorKind;

    fn has_error_kind(toks: &[Token], kind: LexErrorKind) -> bool {
        toks.iter().any(|t| t.kind == TokenKind::Error(kind))
    }

    fn has_diag_code(diags: &[Diagnostic], code: &str) -> bool {
        diags.iter().any(|d| d.code == Some(code))
    }

    #[test]
    fn newline_in_string_e0101() {
        let (toks, diags) = lex_with_diags("\"hello\nworld\"");
        assert!(
            has_error_kind(&toks, LexErrorKind::NewlineInString),
            "expected NewlineInString token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0101"),
            "expected E0101 diagnostic; got {diags:?}"
        );
    }

    #[test]
    fn unterminated_string_e0102() {
        let (toks, diags) = lex_with_diags("\"oops");
        assert!(
            has_error_kind(&toks, LexErrorKind::UnterminatedString),
            "expected UnterminatedString token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0102"),
            "expected E0102 diagnostic; got {diags:?}"
        );
    }

    #[test]
    fn unterminated_block_comment_e0103() {
        let (toks, diags) = lex_with_diags("/* /*");
        assert!(
            has_error_kind(&toks, LexErrorKind::UnterminatedBlockComment),
            "expected UnterminatedBlockComment token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0103"),
            "expected E0103 diagnostic; got {diags:?}"
        );
    }

    #[test]
    fn number_overflow_e0104() {
        let (toks, diags) = lex_with_diags("99999999999999999999");
        assert!(
            has_error_kind(&toks, LexErrorKind::NumberOverflow),
            "expected NumberOverflow token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0104"),
            "expected E0104 diagnostic; got {diags:?}"
        );
    }

    #[test]
    fn invalid_digit_separator_decimal_doubled() {
        let (toks, diags) = lex_with_diags("1__000");
        assert!(
            has_error_kind(&toks, LexErrorKind::InvalidDigitSeparator),
            "expected InvalidDigitSeparator token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0105"),
            "expected E0105 diagnostic; got {diags:?}"
        );
    }

    #[test]
    fn invalid_digit_separator_hex_leading() {
        let (toks, diags) = lex_with_diags("$_ff");
        assert!(
            has_error_kind(&toks, LexErrorKind::InvalidDigitSeparator),
            "expected InvalidDigitSeparator token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0105"),
            "expected E0105 diagnostic; got {diags:?}"
        );
        // Regression: `$_ff` must produce exactly ONE E0105 diagnostic, not
        // two. Previously both the hex-prefix pre-check and `scan_digit_run`
        // diagnosed the same leading `_`.
        let e0105_count = diags.iter().filter(|d| d.code == Some("E0105")).count();
        assert_eq!(
            e0105_count, 1,
            "expected exactly one E0105 diagnostic for `$_ff`, got {diags:?}"
        );
    }

    #[test]
    fn invalid_digit_separator_binary_leading() {
        let (toks, diags) = lex_with_diags("%_10");
        assert!(
            has_error_kind(&toks, LexErrorKind::InvalidDigitSeparator),
            "expected InvalidDigitSeparator token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0105"),
            "expected E0105 diagnostic; got {diags:?}"
        );
        // Regression: `%_10` must produce exactly ONE E0105 diagnostic.
        let e0105_count = diags.iter().filter(|d| d.code == Some("E0105")).count();
        assert_eq!(
            e0105_count, 1,
            "expected exactly one E0105 diagnostic for `%_10`, got {diags:?}"
        );
    }

    #[test]
    fn malformed_exponent_e0107() {
        // `1e` — float with no exponent digits is a malformed number,
        // not an unexpected-character error. Produces E0107.
        let (toks, diags) = lex_with_diags("1e");
        assert!(
            has_error_kind(&toks, LexErrorKind::MalformedNumber),
            "expected MalformedNumber token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0107"),
            "expected E0107 diagnostic; got {diags:?}"
        );
        let e0107 = diags.iter().find(|d| d.code == Some("E0107")).unwrap();
        assert!(
            e0107.message.contains("exponent"),
            "expected E0107 message to mention exponent; got {:?}",
            e0107.message
        );
    }

    #[test]
    fn distinct_messages_for_newline_in_string_variants() {
        // Plain newline inside `"..."` and `\` immediately before newline
        // should produce different diagnostic messages, even though both
        // use code E0101 / NewlineInString.
        let (_, diags_plain) = lex_with_diags("\"hello\nworld\"");
        let (_, diags_escaped) = lex_with_diags("\"hello\\\nworld\"");
        let plain_msg = diags_plain
            .iter()
            .find(|d| d.code == Some("E0101"))
            .map(|d| d.message.clone())
            .expect("expected E0101 for plain newline");
        let escaped_msg = diags_escaped
            .iter()
            .find(|d| d.code == Some("E0101"))
            .map(|d| d.message.clone())
            .expect("expected E0101 for backslash-newline");
        assert_ne!(
            plain_msg, escaped_msg,
            "the two newline-in-string variants should have distinct messages"
        );
        assert!(
            escaped_msg.contains("backslash"),
            "backslash-newline message should mention backslash; got {escaped_msg:?}"
        );
    }

    #[test]
    fn invalid_digit_separator_trailing() {
        let (toks, diags) = lex_with_diags("1_000_");
        assert!(
            has_error_kind(&toks, LexErrorKind::InvalidDigitSeparator),
            "expected InvalidDigitSeparator token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0105"),
            "expected E0105 diagnostic; got {diags:?}"
        );
    }

    #[test]
    fn invalid_char_at_top_level_e0106() {
        let (toks, diags) = lex_with_diags("@");
        // Either InvalidChar or UnexpectedChar.
        let has_err = toks.iter().any(|t| {
            matches!(
                t.kind,
                TokenKind::Error(LexErrorKind::InvalidChar)
                    | TokenKind::Error(LexErrorKind::UnexpectedChar)
            )
        });
        assert!(
            has_err,
            "expected InvalidChar or UnexpectedChar token; got {toks:?}"
        );
        assert!(
            has_diag_code(&diags, "E0106"),
            "expected E0106 diagnostic; got {diags:?}"
        );
        // Cursor must advance — total tokens (including Eof) should be at least 2.
        assert!(
            toks.len() >= 2,
            "lexer must continue past invalid char; got {toks:?}"
        );
    }
}
