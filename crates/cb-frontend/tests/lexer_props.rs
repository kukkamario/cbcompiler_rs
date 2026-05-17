//! `proptest` properties for the lexer.

use cb_frontend::span::FileId;
use cb_frontend::{LexerOptions, TokenKind, tokenize};
use proptest::prelude::*;

fn safe_source() -> impl Strategy<Value = String> {
    // Generate tokens chosen from a fixed alphabet that lex unambiguously and
    // can't trigger lex errors. Concatenate. This is the "well-formed ASCII"
    // strategy needed for round-trip.
    let pieces = prop::collection::vec(
        prop_oneof![
            Just("foo".to_string()),
            Just("bar123".to_string()),
            Just("_x".to_string()),
            Just("If".to_string()),
            Just("EndIf".to_string()),
            Just("123".to_string()),
            Just("1_000".to_string()),
            Just("0.5".to_string()),
            Just("\"hello\"".to_string()),
            Just("// comment\n".to_string()),
            Just("/* nested /* */ */".to_string()),
            Just("  ".to_string()),
            Just("\t".to_string()),
            Just("\n".to_string()),
            Just(":".to_string()),
            Just("+".to_string()),
            Just("(".to_string()),
            Just(")".to_string()),
        ],
        0..40,
    );
    pieces.prop_map(|parts| {
        // Separator between pieces avoids fusing idents/numbers.
        let mut s = parts.join(" ");
        // Ensure a trailing newline so any trailing comment is closed.
        s.push('\n');
        s
    })
}

proptest! {
    #[test]
    fn round_trip_preserve_trivia(src in safe_source()) {
        let (tokens, _diags) = tokenize(
            &src,
            FileId(0),
            LexerOptions { preserve_trivia: true },
        );
        let reconstructed: String = tokens
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Eof))
            .map(|t| &src[t.span.start as usize..t.span.end as usize])
            .collect();
        prop_assert_eq!(reconstructed, src);
    }

    #[test]
    fn no_panic_on_arbitrary_utf8(src in any::<String>()) {
        let _ = tokenize(&src, FileId(0), LexerOptions::default());
    }

    #[test]
    fn always_ends_with_one_eof(src in any::<String>()) {
        let (tokens, _) = tokenize(&src, FileId(0), LexerOptions::default());
        prop_assert!(matches!(tokens.last().map(|t| t.kind), Some(TokenKind::Eof)));
        let eof_count = tokens.iter().filter(|t| matches!(t.kind, TokenKind::Eof)).count();
        prop_assert_eq!(eof_count, 1);
    }

    #[test]
    fn determinism(src in any::<String>()) {
        let a = tokenize(&src, FileId(0), LexerOptions::default());
        let b = tokenize(&src, FileId(0), LexerOptions::default());
        let to_summary = |(toks, diags): &(Vec<cb_frontend::Token>, Vec<cb_diagnostics::Diagnostic>)| {
            let t: Vec<_> = toks
                .iter()
                .map(|t| (format!("{:?}", t.kind), t.span.start, t.span.end))
                .collect();
            (t, diags.clone())
        };
        prop_assert_eq!(to_summary(&a), to_summary(&b));
    }
}
