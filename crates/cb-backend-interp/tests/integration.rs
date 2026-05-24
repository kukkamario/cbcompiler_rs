use std::io::Write;

use cb_diagnostics::SourceMap;
use cb_frontend::{LexerOptions, parse, tokenize};

fn run(src: &str) -> String {
    let mut sources = SourceMap::new();
    let file = sources.add("test.cb".into(), src.into());

    let (tokens, lex_diags) = tokenize(src, file, LexerOptions::default());
    assert!(
        lex_diags.iter().all(|d| !matches!(d.severity, cb_diagnostics::Severity::Error)),
        "lex errors: {lex_diags:?}"
    );

    let parse_result = parse(&tokens, src, file);
    assert!(
        parse_result.diagnostics.iter().all(|d| !matches!(d.severity, cb_diagnostics::Severity::Error)),
        "parse errors: {:?}",
        parse_result.diagnostics
    );

    let runtime_catalog = cb_runtime_sys::load_catalog().expect("load catalog");
    let mut sema = cb_sema::analyze(
        &parse_result.arena,
        &parse_result.program,
        src,
        file,
        &runtime_catalog,
    );

    let sema_errors: Vec<_> = sema.diagnostics.iter()
        .filter(|d| matches!(d.severity, cb_diagnostics::Severity::Error))
        .collect();
    assert!(sema_errors.is_empty(), "sema errors: {sema_errors:?}");

    let ir = cb_sema::lower::lower(
        &parse_result.arena,
        &parse_result.program,
        src,
        &mut sema,
    );

    #[cfg(debug_assertions)]
    cb_ir::verify::verify(&ir);

    let mut output = Vec::new();
    {
        let mut interp = cb_backend_interp::Interpreter::new(&ir, &sema.interner)
            .with_stdout(Box::new(&mut output as &mut dyn Write));
        interp.run().expect("interpreter should succeed");
    }
    String::from_utf8(output).expect("output should be valid UTF-8")
}

#[test]
fn hello_world() {
    let out = run(r#"Print "Hello, World!""#);
    assert_eq!(out, "Hello, World!\n");
}

#[test]
fn print_integer() {
    let out = run("Dim x As Int = 42\nPrint Str(x)");
    assert_eq!(out, "42\n");
}

#[test]
fn arithmetic_add() {
    let out = run("Dim x As Int = 2 + 3\nPrint Str(x)");
    assert_eq!(out, "5\n");
}

#[test]
fn arithmetic_mul() {
    let out = run("Dim x As Int = 6 * 7\nPrint Str(x)");
    assert_eq!(out, "42\n");
}

#[test]
fn float_arithmetic() {
    let out = run("Dim x As Float = 1.5 + 2.5\nPrint Str(x)");
    assert_eq!(out, "4\n");
}

#[test]
fn if_else_true() {
    let out = run("Dim x As Int = 1\nIf x = 1 Then\nPrint \"yes\"\nElse\nPrint \"no\"\nEndIf");
    assert_eq!(out, "yes\n");
}

#[test]
fn if_else_false() {
    let out = run("Dim x As Int = 0\nIf x = 1 Then\nPrint \"yes\"\nElse\nPrint \"no\"\nEndIf");
    assert_eq!(out, "no\n");
}

#[test]
fn while_loop() {
    let out = run(
        "Dim i As Int = 0\n\
         While i < 3\n\
           i = i + 1\n\
         Wend\n\
         Print Str(i)"
    );
    assert_eq!(out, "3\n");
}

#[test]
fn for_loop() {
    let out = run(
        "Dim total As Int = 0\n\
         For i = 1 To 5\n\
           total = total + i\n\
         Next i\n\
         Print Str(total)"
    );
    assert_eq!(out, "15\n");
}

#[test]
fn function_call() {
    let out = run(
        "Function double(x As Integer) As Integer\n\
           Return x * 2\n\
         EndFunction\n\
         Print Str(double(21))"
    );
    assert_eq!(out, "42\n");
}

#[test]
fn abs_int() {
    let out = run("Print Str(Abs(-42))");
    assert_eq!(out, "42\n");
}

#[test]
fn abs_float() {
    let out = run("Print Str(Abs(-3.14))");
    assert_eq!(out, "3.14\n");
}

#[test]
fn string_concatenation() {
    let out = run("Dim a As String = \"Hello, \"\nDim b As String = \"World!\"\nPrint a + b");
    assert_eq!(out, "Hello, World!\n");
}

#[test]
fn multiple_prints() {
    let out = run("Print \"a\"\nPrint \"b\"\nPrint \"c\"");
    assert_eq!(out, "a\nb\nc\n");
}
