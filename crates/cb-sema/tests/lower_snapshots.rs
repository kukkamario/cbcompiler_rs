//! Snapshot tests for the AST→IR lowering pass.
//!
//! Each test parses CoolBasic source, runs sema, lowers to IR, and snapshots
//! the printed IR output via `insta`.

use cb_diagnostics::{FileId, SourceMap};
use cb_frontend::lexer::{LexerOptions, tokenize};
use cb_frontend::parser::parse;

fn empty_catalog() -> cb_sema::RuntimeCatalog {
    cb_sema::RuntimeCatalog {
        types: Vec::new(),
        functions: Vec::new(),
        constants: Vec::new(),
    }
}

/// A single-file `SourceMap` holding `src` at `FileId(0)`, matching the spans.
fn sources_of(src: &str) -> SourceMap {
    let mut sources = SourceMap::new();
    sources.add("<test>".to_string(), src.to_string());
    sources
}

fn lower_src(src: &str) -> String {
    let file = FileId(0);
    let (tokens, _) = tokenize(src, file, LexerOptions::default());
    let parsed = parse(&tokens, src, file);
    let sources = sources_of(src);
    let mut sema = cb_sema::analyze(&parsed.arena, &parsed.program, &sources, &empty_catalog());
    assert!(!sema.has_errors(), "sema errors: {:?}", sema.diagnostics);
    let ir = cb_sema::lower::lower(&parsed.arena, &parsed.program, &sources, &mut sema);
    cb_ir::verify::verify(&ir);
    cb_ir::print::print_program(&ir, &sema.interner)
}

fn sema_diags(src: &str) -> Vec<cb_diagnostics::Diagnostic> {
    let file = FileId(0);
    let (tokens, _) = tokenize(src, file, LexerOptions::default());
    let parsed = parse(&tokens, src, file);
    let sema = cb_sema::analyze(
        &parsed.arena,
        &parsed.program,
        &sources_of(src),
        &empty_catalog(),
    );
    sema.diagnostics
}

/// Assigning to a non-variable-rooted lvalue (e.g. a function-call
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
    let ir = lower_src("Dim x As Int\nIf x = 0 Then\n  x = 1\nElse\n  x = 2\nEndIf\n");
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
    let ir =
        lower_src("Dim i As Int\nDim j As Int\nFor i = 0 To 3\n  For j = 0 To 5\n  Next\nNext\n");
    insta::assert_snapshot!(ir);
}

// Descending integer loop — direction-test constants are integer-typed
// and the negative step drives the `>=` (descending) branch.
#[test]
fn for_loop_descending() {
    let ir = lower_src("Dim i As Int\nFor i = 10 To 0 Step -1\nNext\n");
    insta::assert_snapshot!(ir);
}

// Float loop — default/zero direction constants and all compare/step
// operands are Float, matching the loop variable (no Int/Float operand mismatch).
#[test]
fn for_loop_float_step() {
    let ir = lower_src("Dim x As Float\nFor x = 10.0 To 0.0 Step -0.5\nNext\n");
    insta::assert_snapshot!(ir);
}

// Mixed loop — `To 10.5` (Float) is coerced to the Int loop variable,
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
fn select_default_in_middle() {
    // §6.2: `Default` may appear in any position. With `Default` between two
    // `Case`s, the dispatch chain must still test both cases (the second one's
    // "else" pointing at the default body), and the default must run only on a
    // no-match — not drop the trailing `Case`. (S-H2)
    let ir = lower_src(
        "Dim x As Int\nx = 2\nSelect x\n  Case 1\n    x = 10\n  Default\n    x = 99\n  Case 2\n    x = 20\nEnd Select\n",
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
    let ir = lower_src("Function Foo() As Int\n  Const X = 42\n  Return X\nEnd Function\n");
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
    let sources = sources_of(src);
    let mut sema = cb_sema::analyze(&parsed.arena, &parsed.program, &sources, catalog);
    assert!(!sema.has_errors(), "sema errors: {:?}", sema.diagnostics);
    let ir = cb_sema::lower::lower(&parsed.arena, &parsed.program, &sources, &mut sema);
    cb_ir::verify::verify(&ir);
    cb_ir::print::print_program(&ir, &sema.interner)
}

#[test]
fn runtime_function_call() {
    let catalog = cb_sema::RuntimeCatalog {
        types: Vec::new(),
        functions: vec![cb_sema::FuncDesc {
            name: "print".to_string(),
            c_symbol: "cb_rt_print".to_string(),
            params: vec![cb_sema::FuncParamDesc {
                name: Some("text".to_string()),
                ty: cb_ir::types::IrType::String,
            }],
            return_ty: cb_ir::types::IrType::Void,
        }],
        constants: Vec::new(),
    };
    let ir = lower_with_catalog("print(\"hello world\")\n", &catalog);
    insta::assert_snapshot!(ir);
}

// Regression: a bare (no-paren, no-arg) call to an *overloaded* command must
// lower to a 0-arg call. `DrawScreen` gained a 2-arg overload, turning
// it into an OverloadSet; lowering previously only handled Function/RuntimeFn
// for bare statements, so `DrawScreen` was silently dropped — the window never
// flipped or pumped events (black, unclosable). See lower.rs ExprStmt handling.
#[test]
fn bare_overloaded_command_lowers_to_call() {
    let two_param = || {
        vec![
            cb_sema::FuncParamDesc {
                name: None,
                ty: cb_ir::types::IrType::Int,
            },
            cb_sema::FuncParamDesc {
                name: None,
                ty: cb_ir::types::IrType::Int,
            },
        ]
    };
    let catalog = cb_sema::RuntimeCatalog {
        types: Vec::new(),
        functions: vec![
            // 0-arg variant
            cb_sema::FuncDesc {
                name: "drawscreen".to_string(),
                c_symbol: "cb_rt_drawscreen".to_string(),
                params: Vec::new(),
                return_ty: cb_ir::types::IrType::Void,
            },
            // 2-arg variant — makes `drawscreen` an overload set
            cb_sema::FuncDesc {
                name: "drawscreen".to_string(),
                c_symbol: "cb_rt_drawscreen_args".to_string(),
                params: two_param(),
                return_ty: cb_ir::types::IrType::Void,
            },
        ],
        constants: Vec::new(),
    };
    let ir = lower_with_catalog("DrawScreen\n", &catalog);
    assert!(
        ir.contains("call drawscreen"),
        "bare overloaded command did not lower to a call:\n{ir}"
    );
}

// Loops, Break and Continue. Continue's target differs per loop kind
// (Forever → body, Repeat-While/Repeat-Until/While → condition, For → step
// block), so each kind gets its own snapshot.

#[test]
fn repeat_forever_break_continue() {
    let ir = lower_src(
        "Dim i As Int\nRepeat\n  i = i + 1\n  If i = 3 Then Continue\n  If i = 5 Then Break\nForever\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn repeat_while_break_continue() {
    let ir = lower_src(
        "Dim i As Int\nRepeat\n  i = i + 1\n  If i = 3 Then Continue\n  If i = 9 Then Break\nWhile i < 10\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn repeat_until_break_continue() {
    let ir = lower_src(
        "Dim i As Int\nRepeat\n  i = i + 1\n  If i = 3 Then Continue\n  If i = 9 Then Break\nUntil i >= 10\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn while_break_continue() {
    let ir = lower_src(
        "Dim i As Int\nWhile i < 10\n  i = i + 1\n  If i = 3 Then Continue\n  If i = 5 Then Break\nWend\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn for_loop_break_continue() {
    let ir = lower_src(
        "Dim i As Int\nFor i = 1 To 10\n  If i = 3 Then Continue\n  If i = 5 Then Break\nNext\n",
    );
    insta::assert_snapshot!(ir);
}

// `Break 2` must jump to the *outer* loop's exit block, not the inner one's.
#[test]
fn break_count_exits_nested_loops() {
    let ir = lower_src(
        "Dim i As Int\nDim j As Int\nFor i = 0 To 9\n  For j = 0 To 9\n    If j = 5 Then Break 2\n  Next j\nNext i\n",
    );
    insta::assert_snapshot!(ir);
}

// For Each desugaring — index+Len walk for arrays, First/Next walk for
// Type linked lists.

#[test]
fn for_each_array() {
    let ir = lower_src(
        "Dim scores As Float[] = New Float[3]\nDim total As Float\nFor v = Each scores\n  total = total + v\nNext v\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn for_each_type() {
    let ir = lower_src(
        "Type Mob\n  Field hp As Int\nEndType\nDim count As Int\nFor n = Each Mob\n  count = count + n.hp\nNext n\n",
    );
    insta::assert_snapshot!(ir);
}

// Arrays — allocation, element store/load, Len with a dimension
// argument, and Redim in both local and global form.

#[test]
fn array_new_index_store_load() {
    let ir = lower_src("Dim a As Integer[]\na = New Integer[10]\na[3] = 42\nDim x As Int = a[3]\n");
    insta::assert_snapshot!(ir);
}

#[test]
fn array_multidim_and_len_dim() {
    let ir = lower_src(
        "Dim b As Float[,]\nb = New Float[4, 8]\nb[1, 2] = 1.5\nDim n As Int\nn = Len(b, 1)\n",
    );
    insta::assert_snapshot!(ir);
}

// A bare function name in value position takes the function's address
// (cb_syntax.md §7.4), lowering to `func_addr <name>` — the sole producer of a
// non-null function pointer, consumed by `call_indirect`.
#[test]
fn function_address_lowers_to_func_addr() {
    let ir = lower_src(
        "Function add(a As Integer, b As Integer) As Integer\n  Return a + b\nEndFunction\n\
         Dim fp As Function(Integer, Integer) As Integer\nfp = add\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn redim_local_and_global() {
    let ir = lower_src(
        "Global garr As Float[]\nDim larr As Int[]\nRedim larr As Int[5]\nRedim garr As Float[8]\n",
    );
    insta::assert_snapshot!(ir);
}

// Value structs — the StorePlace projection paths.

#[test]
fn struct_field_write_read() {
    let ir = lower_src(
        "Struct Vec2\n  Field x As Float\n  Field y As Float\nEndStruct\n\
         Dim p As Vec2\np.x = 1.5\nDim s As Float = p.x + p.y\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn struct_nested_field_write() {
    let ir = lower_src(
        "Struct Inner\n  Field v As Int\nEndStruct\n\
         Struct Outer\n  Field inner As Inner\n  Field w As Int\nEndStruct\n\
         Dim o As Outer\no.inner.v = 5\nDim r As Int = o.inner.v\n",
    );
    insta::assert_snapshot!(ir);
}

// Whole-struct assignment is a value copy (load + store), never aliasing.
#[test]
fn struct_copy_assignment() {
    let ir = lower_src(
        "Struct Vec2\n  Field x As Int\nEndStruct\n\
         Dim p As Vec2\np.x = 1\nDim q As Vec2 = p\nq.x = 99\n",
    );
    insta::assert_snapshot!(ir);
}

// Mixed [Index, Field] projection chain — `arr[1].x = 42` must be a single
// store_place addressing the owning local (regression surface).
#[test]
fn array_of_structs_element_field() {
    let ir = lower_src(
        "Struct P\n  Field x As Int\nEndStruct\n\
         Dim arr As P[] = New P[3]\narr[1].x = 42\nDim r As Int = arr[1].x\n",
    );
    insta::assert_snapshot!(ir);
}

// Type (heap) instances — New, field access, list intrinsics, Delete.

#[test]
fn type_new_and_field_assign() {
    let ir = lower_src(
        "Type Mob\n  Field hp As Int\nEndType\n\
         Dim m As Mob = New Mob\nm.hp = 5\nDim h As Int = m.hp\n",
    );
    insta::assert_snapshot!(ir);
}

#[test]
fn type_list_intrinsics() {
    let ir = lower_src(
        "Type Mob\n  Field hp As Int\nEndType\n\
         Dim a As Mob = New Mob\nDim b As Mob = New Mob\n\
         Dim n As Mob = First(Mob)\nn = Next(n)\nn = Previous(n)\nDim l As Mob = Last(Mob)\n",
    );
    insta::assert_snapshot!(ir);
}

// All three Delete lowerings: lvalue on a local (rewind+mark), lvalue on a
// global, and rvalue (no rewind) on a call result.
#[test]
fn type_delete_lvalue_rvalue_global() {
    let ir = lower_src(
        "Type Mob\n  Field hp As Int\nEndType\n\
         Global gm As Mob\ngm = New Mob\nDim n As Mob = New Mob\n\
         Delete n\nDelete gm\nDelete First(Mob)\n",
    );
    insta::assert_snapshot!(ir);
}

// String comparisons lower to the str_* ops and Len(String) to
// str_len, not their numeric/array counterparts.
#[test]
fn string_compare_and_strlen() {
    let ir = lower_src(
        "Dim a As String = \"abc\"\nDim b As String = \"abd\"\nDim x As Int\n\
         If a < b Then x = 1\nIf a = b Then x = 2\nIf a <> b Then x = 3\nIf a >= b Then x = 4\n\
         Dim n As Int = Len(a)\n",
    );
    insta::assert_snapshot!(ir);
}

// Mirror of short_circuit_and: rhs must only be evaluated on the else edge.
#[test]
fn short_circuit_or() {
    let ir = lower_src("Dim a As Int\nDim b As Int\nIf a Or b Then\n  a = 1\nEndIf\n");
    insta::assert_snapshot!(ir);
}

// Continue inside Select is explicit fall-through (§6.2) — the arm
// body jumps straight into the *next* arm's body, skipping its test.
#[test]
fn select_continue_fallthrough() {
    let ir = lower_src(
        "Dim x As Int\nx = 30\nSelect x\n  Case 30\n    x = 1\n    Continue\n  Case 40\n    x = 2\n  Default\n    x = 0\nEnd Select\n",
    );
    insta::assert_snapshot!(ir);
}

// An array of value structs must carry `StructVal(p)` elements
// consistently (the declared array type is refined recursively, not just at the
// top level), so For Each over it types the loop variable as the struct value
// — not a heap `TypeRef`. The element is read via get_element_flat and its
// field accessed by value.
#[test]
fn for_each_struct_array() {
    let ir = lower_src(
        "Struct P\n  Field x As Int\nEndStruct\n\
         Dim arr As P[] = New P[3]\nDim total As Int\n\
         For e = Each arr\n  total = total + e.x\nNext e\n",
    );
    insta::assert_snapshot!(ir);
}

// For Each over a rank ≥ 2 array walks every element in row-major order
// (§6.3), not just dimension 0 — the bound is `array_total_len` and elements are
// read with a single flat `get_element_flat` index. Previously this emitted an
// axis-0 `len` plus a single-index `get_element`, which trapped at runtime.
#[test]
fn for_each_multidim_array() {
    let ir = lower_src(
        "Dim grid As Int[,] = New Int[2, 3]\nDim sum As Int\n\
         For v = Each grid\n  sum = sum + v\nNext v\n",
    );
    insta::assert_snapshot!(ir);
}

// `Delete <field>` and `Delete <index>` are rvalue deletes (§3.3): the
// node is freed with no slot rewind, exactly like `Delete First(T)`. They must
// emit `delete_rvalue` over the loaded reference — previously they were
// classified lvalue and the lowerer dropped them silently (no IR at all).
#[test]
fn delete_field_and_index_are_rvalue() {
    let ir = lower_src(
        "Type Node\n  Field link As Node\nEndType\n\
         Dim a As Node = New Node\na.link = New Node\n\
         Dim arr As Node[] = New Node[3]\narr[0] = New Node\n\
         Delete a.link\nDelete arr[0]\n",
    );
    insta::assert_snapshot!(ir);
}
