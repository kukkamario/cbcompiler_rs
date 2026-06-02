//! Snapshot tests for the AST→IR lowering pass.
//!
//! Each test parses CoolBasic source, runs sema, lowers to IR, and snapshots
//! the printed IR output via `insta`.

use cb_diagnostics::FileId;
use cb_frontend::lexer::{tokenize, LexerOptions};
use cb_frontend::parser::parse;

fn empty_catalog() -> cb_sema::RuntimeCatalog {
    cb_sema::RuntimeCatalog {
        types: Vec::new(),
        functions: Vec::new(),
    }
}

fn lower_src(src: &str) -> String {
    let file = FileId(0);
    let (tokens, _) = tokenize(src, file, LexerOptions::default());
    let parsed = parse(&tokens, src, file);
    let mut sema = cb_sema::analyze(&parsed.arena, &parsed.program, src, file, &empty_catalog());
    assert!(
        !sema.has_errors(),
        "sema errors: {:?}",
        sema.diagnostics
    );
    let ir = cb_sema::lower::lower(&parsed.arena, &parsed.program, src, &mut sema);
    cb_ir::verify::verify(&ir);
    cb_ir::print::print_program(&ir, &sema.interner)
}

fn sema_diags(src: &str) -> Vec<cb_diagnostics::Diagnostic> {
    let file = FileId(0);
    let (tokens, _) = tokenize(src, file, LexerOptions::default());
    let parsed = parse(&tokens, src, file);
    let sema = cb_sema::analyze(&parsed.arena, &parsed.program, src, file, &empty_catalog());
    sema.diagnostics
}

/// FD-019: assigning to a non-variable-rooted lvalue (e.g. a function-call
/// result's field) is rejected, not silently dropped by lowering.
#[test]
fn invalid_assign_target_call_field() {
    let diags = sema_diags(
        "Type Mob\n  Field hp As Int\nEndType\n\
         Function getit() As Mob\n  Return Null\nEndFunction\n\
         getit().hp = 99\n",
    );
    assert!(
        diags.iter().any(|d| d.code_is("E0325")),
        "expected E0325, got: {diags:?}"
    );
}

/// A normal variable-rooted field assignment must NOT trip the new check.
#[test]
fn valid_field_assign_not_rejected() {
    let diags = sema_diags(
        "Type Mob\n  Field hp As Int\nEndType\n\
         Dim m As Mob = New Mob\nm.hp = 5\n",
    );
    assert!(
        !diags.iter().any(|d| d.code_is("E0325")),
        "unexpected E0325: {diags:?}"
    );
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

// FD-020: descending integer loop — direction-test constants are integer-typed
// and the negative step drives the `>=` (descending) branch.
#[test]
fn for_loop_descending() {
    let ir = lower_src("Dim i As Int\nFor i = 10 To 0 Step -1\nNext\n");
    insta::assert_snapshot!(ir);
}

// FD-020: float loop — default/zero direction constants and all compare/step
// operands are Float, matching the loop variable (no Int/Float operand mismatch).
#[test]
fn for_loop_float_step() {
    let ir = lower_src("Dim x As Float\nFor x = 10.0 To 0.0 Step -0.5\nNext\n");
    insta::assert_snapshot!(ir);
}

// FD-020: mixed loop — `To 10.5` (Float) is coerced to the Int loop variable,
// so the lowered `to` register is Int (a narrowing E0318 warning also fires).
#[test]
fn for_loop_mixed_narrowing() {
    let ir = lower_src("Dim i As Int\nFor i = 1 To 10.5\nNext\n");
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

fn lower_with_catalog(src: &str, catalog: &cb_sema::RuntimeCatalog) -> String {
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

extern "C" fn dummy_runtime_fn() {}

#[test]
fn runtime_function_call() {
    let catalog = cb_sema::RuntimeCatalog {
        types: Vec::new(),
        functions: vec![cb_sema::FuncDesc {
            name: "print".to_string(),
            c_symbol: "cb_rt_print".to_string(),
            fn_ptr: dummy_runtime_fn,
            params: vec![cb_sema::FuncParamDesc {
                name: Some("text".to_string()),
                ty: cb_ir::types::IrType::String,
            }],
            return_ty: cb_ir::types::IrType::Void,
        }],
    };
    let ir = lower_with_catalog("print(\"hello world\")\n", &catalog);
    insta::assert_snapshot!(ir);
}

// Regression: a bare (no-paren, no-arg) call to an *overloaded* command must
// lower to a 0-arg call. `DrawScreen` gained a 2-arg overload (FD-017), turning
// it into an OverloadSet; lowering previously only handled Function/RuntimeFn
// for bare statements, so `DrawScreen` was silently dropped — the window never
// flipped or pumped events (black, unclosable). See lower.rs ExprStmt handling.
#[test]
fn bare_overloaded_command_lowers_to_call() {
    let two_param = || {
        vec![
            cb_sema::FuncParamDesc { name: None, ty: cb_ir::types::IrType::Int },
            cb_sema::FuncParamDesc { name: None, ty: cb_ir::types::IrType::Int },
        ]
    };
    let catalog = cb_sema::RuntimeCatalog {
        types: Vec::new(),
        functions: vec![
            // 0-arg variant
            cb_sema::FuncDesc {
                name: "drawscreen".to_string(),
                c_symbol: "cb_rt_drawscreen".to_string(),
                fn_ptr: dummy_runtime_fn,
                params: Vec::new(),
                return_ty: cb_ir::types::IrType::Void,
            },
            // 2-arg variant — makes `drawscreen` an overload set
            cb_sema::FuncDesc {
                name: "drawscreen".to_string(),
                c_symbol: "cb_rt_drawscreen_args".to_string(),
                fn_ptr: dummy_runtime_fn,
                params: two_param(),
                return_ty: cb_ir::types::IrType::Void,
            },
        ],
    };
    let ir = lower_with_catalog("DrawScreen\n", &catalog);
    assert!(
        ir.contains("call drawscreen"),
        "bare overloaded command did not lower to a call:\n{ir}"
    );
}
