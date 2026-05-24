//! Snapshot tests for the AST→IR lowering pass.
//!
//! Each test parses CoolBasic source, runs sema, lowers to IR, and snapshots
//! the printed IR output via `insta`.

use cb_diagnostics::FileId;
use cb_frontend::lexer::{tokenize, LexerOptions};
use cb_frontend::parser::parse;

fn lower_src(src: &str) -> String {
    let file = FileId(0);
    let (tokens, _) = tokenize(src, file, LexerOptions::default());
    let parsed = parse(&tokens, src, file);
    let mut sema = cb_sema::analyze(&parsed.arena, &parsed.program, src, file, &[]);
    assert!(
        !sema.has_errors(),
        "sema errors: {:?}",
        sema.diagnostics
    );
    let ir = cb_sema::lower::lower(&parsed.arena, &parsed.program, src, &mut sema);
    cb_ir::verify::verify(&ir);
    cb_ir::print::print_program(&ir, &sema.interner)
}

#[test]
fn simple_assignment() {
    let ir = lower_src("Dim x As Int\nx = 42\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn mixed_type_arithmetic() {
    let ir = lower_src("Dim x As Float\nx = 1 + 1.5\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn string_concat_mixed() {
    let ir = lower_src("Dim x As String\nx = \"count: \" + 42\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn if_else() {
    let ir = lower_src(
        "Dim x As Int\nIf x = 0 Then\n  x = 1\nElse\n  x = 2\nEndIf\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn while_loop() {
    let ir = lower_src("Dim i As Int\nWhile i < 10\n  i = i + 1\nWend\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn for_loop() {
    let ir = lower_src("Dim i As Int\nFor i = 1 To 10\nNext\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn nested_for_loops() {
    let ir = lower_src(
        "Dim i As Int\nDim j As Int\nFor i = 0 To 3\n  For j = 0 To 5\n  Next\nNext\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn select_case() {
    let ir = lower_src(
        "Dim x As Int\nx = 2\nSelect x\n  Case 1\n    x = 10\n  Case 2\n    x = 20\n  Default\n    x = 0\nEnd Select\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn function_call() {
    let ir = lower_src(
        "Function Add(a As Int, b As Int) As Int\n  Return a + b\nEnd Function\nDim result As Int\nresult = Add(3, 4)\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn implicit_conversion_in_assignment() {
    let ir = lower_src("Dim x As Float\nDim y As Int\ny = 5\nx = y\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn goto_label() {
    let ir = lower_src("Goto done\nDim x As Int\nx = 1\ndone:\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn short_circuit_and() {
    let ir = lower_src("Dim a As Int\nDim b As Int\nIf a And b Then\n  a = 1\nEndIf\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn function_local_const() {
    let ir = lower_src(
        "Function Foo() As Int\n  Const X = 42\n  Return X\nEnd Function\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn comparison_with_promotion() {
    let ir = lower_src("Dim x As Int\nDim y As Float\nIf x < y Then\n  x = 1\nEndIf\n");
    insta::assert_snapshot!(ir);
}

fn lower_with_catalog(src: &str, catalog: &[cb_sema::FuncDesc]) -> String {
    let file = FileId(0);
    let (tokens, _) = tokenize(src, file, LexerOptions::default());
    let parsed = parse(&tokens, src, file);
    let mut sema = cb_sema::analyze(&parsed.arena, &parsed.program, src, file, catalog);
    assert!(
        !sema.has_errors(),
        "sema errors: {:?}",
        sema.diagnostics
    );
    let ir = cb_sema::lower::lower(&parsed.arena, &parsed.program, src, &mut sema);
    cb_ir::verify::verify(&ir);
    cb_ir::print::print_program(&ir, &sema.interner)
}

#[test]
fn runtime_function_call() {
    let catalog = vec![cb_sema::FuncDesc {
        name: "print".to_string(),
        c_symbol: "cb_rt_print".to_string(),
        params: vec![cb_sema::FuncParamDesc {
            name: Some("text".to_string()),
            ty: cb_sema::Type::String,
        }],
        return_ty: cb_sema::Type::Void,
    }];
    let ir = lower_with_catalog("print(\"hello world\")\n", &catalog);
    insta::assert_snapshot!(ir);
}
