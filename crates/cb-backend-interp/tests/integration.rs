use std::io::Write;

use cb_backend_interp::error::{InterpError, InterpErrorKind};
use cb_backend_interp::interp::Frame;
use cb_backend_interp::observer::Observer;
use cb_diagnostics::{SourceMap, Span};
use cb_frontend::{LexerOptions, parse, tokenize};
use cb_ir::FuncId;
use cb_ir::inst::{InstKind, TrapKind};

fn run(src: &str) -> String {
    let mut sources = SourceMap::new();
    let file = sources.add("test.cb".into(), src.into());

    let (tokens, lex_diags) = tokenize(src, file, LexerOptions::default());
    assert!(
        lex_diags
            .iter()
            .all(|d| !matches!(d.severity, cb_diagnostics::Severity::Error)),
        "lex errors: {lex_diags:?}"
    );

    let parse_result = parse(&tokens, src, file);
    assert!(
        parse_result
            .diagnostics
            .iter()
            .all(|d| !matches!(d.severity, cb_diagnostics::Severity::Error)),
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

    let sema_errors: Vec<_> = sema
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, cb_diagnostics::Severity::Error))
        .collect();
    assert!(sema_errors.is_empty(), "sema errors: {sema_errors:?}");

    let ir = cb_sema::lower::lower(&parse_result.arena, &parse_result.program, src, &mut sema);

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

// FD-020: Int(Float) rounds to nearest, ties away from zero (cb_runtime.md
// §Math). Positive ties round up, negative ties round down (away from zero),
// and non-tie values round to the nearest — not truncation toward zero.
#[test]
fn int_conversion_rounds_half_away_from_zero() {
    let cases = [
        ("10.5", "11"),
        ("1.5", "2"),
        ("2.5", "3"),
        ("0.5", "1"),
        ("-0.5", "-1"),
        ("-1.5", "-2"),
        ("-2.5", "-3"),
        ("-1.4", "-1"),
        ("-1.6", "-2"),
    ];
    for (input, expected) in cases {
        let out = run(&format!("Dim i As Int = Int({input})\nPrint Str(i)"));
        assert_eq!(out, format!("{expected}\n"), "Int({input})");
    }
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
    let out = run("Dim i As Int = 0\n\
         While i < 3\n\
           i = i + 1\n\
         Wend\n\
         Print Str(i)");
    assert_eq!(out, "3\n");
}

#[test]
fn for_loop() {
    let out = run("Dim total As Int = 0\n\
         For i = 1 To 5\n\
           total = total + i\n\
         Next i\n\
         Print Str(total)");
    assert_eq!(out, "15\n");
}

#[test]
fn function_call() {
    let out = run("Function double(x As Integer) As Integer\n\
           Return x * 2\n\
         EndFunction\n\
         Print Str(double(21))");
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
fn unary_plus_is_abs_int() {
    // FD-028: unary `+` is absolute value, identical to Abs.
    let out = run("Print Str(+(-5))");
    assert_eq!(out, "5\n");
}

#[test]
fn unary_plus_is_abs_float() {
    let out = run("Print Str(+(-3.14))");
    assert_eq!(out, "3.14\n");
}

#[test]
fn unary_plus_const_folds_to_abs() {
    // FD-028: `+` in a constant expression folds to the absolute value.
    let out = run("Const c = +(-7)\n\
         Print Str(c)");
    assert_eq!(out, "7\n");
}

#[test]
fn unary_plus_matches_abs() {
    // `+x` and `Abs(x)` must agree; their difference is 0.
    let out = run("Dim x As Int\n\
         x = -42\n\
         Print Str(+x - Abs(x))");
    assert_eq!(out, "0\n");
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

// ── Heap type tests ────────────────────────────────────────────────

#[test]
fn type_new_and_field_access() {
    let out = run("Type Enemy\n\
           Field hp As Int\n\
         EndType\n\
         Dim e As Enemy = New Enemy\n\
         e.hp = 100\n\
         Print Str(e.hp)");
    assert_eq!(out, "100\n");
}

#[test]
fn type_linked_list_first_next() {
    let out = run("Type Node\n\
           Field val As Int\n\
         EndType\n\
         Dim a As Node = New Node\n\
         a.val = 1\n\
         Dim b As Node = New Node\n\
         b.val = 2\n\
         Dim c As Node = New Node\n\
         c.val = 3\n\
         Dim n As Node = First(Node)\n\
         While n <> Null\n\
           Print Str(n.val)\n\
           n = Next(n)\n\
         Wend");
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn type_for_each() {
    let out = run("Type Item\n\
           Field x As Int\n\
         EndType\n\
         Dim a As Item = New Item\n\
         a.x = 10\n\
         Dim b As Item = New Item\n\
         b.x = 20\n\
         For n = Each Item\n\
           Print Str(n.x)\n\
         Next n");
    assert_eq!(out, "10\n20\n");
}

#[test]
fn type_delete_and_continue_iteration() {
    let out = run("Type Obj\n\
           Field v As Int\n\
         EndType\n\
         Dim a As Obj = New Obj\n\
         a.v = 1\n\
         Dim b As Obj = New Obj\n\
         b.v = 2\n\
         Dim c As Obj = New Obj\n\
         c.v = 3\n\
         For n = Each Obj\n\
           If n.v = 2 Then\n\
             Delete n\n\
           EndIf\n\
         Next n\n\
         Dim total As Int = 0\n\
         For m = Each Obj\n\
           total = total + m.v\n\
         Next m\n\
         Print Str(total)");
    assert_eq!(out, "4\n");
}

// FD-034 item 3: For Each over a rank-2 array visits every element in row-major
// order (last index varies fastest), not just dimension 0. Before the fix this
// trapped on the first iteration (single flat index into a 2-D array).
#[test]
fn for_each_multidim_array_row_major() {
    let out = run("Dim grid As Int[,] = New Int[2, 3]\n\
         grid[0, 0] = 1\n\
         grid[0, 1] = 2\n\
         grid[0, 2] = 3\n\
         grid[1, 0] = 4\n\
         grid[1, 1] = 5\n\
         grid[1, 2] = 6\n\
         For v = Each grid\n\
           Print Str(v)\n\
         Next v");
    assert_eq!(out, "1\n2\n3\n4\n5\n6\n");
}

// FD-034 item 2: `Delete <field>` is an rvalue delete that actually frees the
// node — it is no longer silently dropped. The freed node is unlinked from the
// Type list, so a later For Each sees only the survivor. Before the fix the
// statement emitted no IR and the node stayed in the list (total would be 3).
#[test]
fn delete_field_frees_node() {
    let out = run("Type Node\n\
           Field link As Node\n\
           Field v As Int\n\
         EndType\n\
         Dim a As Node = New Node\n\
         a.v = 1\n\
         a.link = New Node\n\
         a.link.v = 2\n\
         Delete a.link\n\
         Dim total As Int = 0\n\
         For n = Each Node\n\
           total = total + n.v\n\
         Next n\n\
         Print Str(total)");
    assert_eq!(out, "1\n");
}

// FD-034 item 2: the same for `Delete <array element>` — the referenced node is
// freed, leaving only the survivor in the Type list.
#[test]
fn delete_array_element_frees_node() {
    let out = run("Type Node\n\
           Field v As Int\n\
         EndType\n\
         Dim arr As Node[] = New Node[2]\n\
         arr[0] = New Node\n\
         arr[0].v = 1\n\
         arr[1] = New Node\n\
         arr[1].v = 2\n\
         Delete arr[0]\n\
         Dim total As Int = 0\n\
         For n = Each Node\n\
           total = total + n.v\n\
         Next n\n\
         Print Str(total)");
    assert_eq!(out, "2\n");
}

#[test]
fn array_new_and_index() {
    let out = run("Dim arr As Int[] = New Int[3]\n\
         arr[0] = 10\n\
         arr[1] = 20\n\
         arr[2] = 30\n\
         Print Str(arr[1])");
    assert_eq!(out, "20\n");
}

#[test]
fn array_len() {
    let out = run("Dim arr As Int[] = New Int[5]\n\
         Print Str(Len(arr))");
    assert_eq!(out, "5\n");
}

#[test]
fn null_comparison() {
    let out = run("Type MyObj\n\
           Field x As Int\n\
         EndType\n\
         Dim obj As MyObj\n\
         If obj = Null Then\n\
           Print \"null\"\n\
         Else\n\
           Print \"not null\"\n\
         EndIf");
    assert_eq!(out, "null\n");
}

// ── Observer tests ─────────────────────────────────────────────────

struct CountingObserver {
    before_count: usize,
    after_count: usize,
    call_count: usize,
    return_count: usize,
}

impl CountingObserver {
    fn new() -> Self {
        Self {
            before_count: 0,
            after_count: 0,
            call_count: 0,
            return_count: 0,
        }
    }
}

impl Observer for CountingObserver {
    fn before_inst(&mut self, _frame: &Frame, _inst: &InstKind, _span: Span) {
        self.before_count += 1;
    }
    fn after_inst(
        &mut self,
        _frame: &Frame,
        _inst: &InstKind,
        _result: &cb_backend_interp::value::Value,
        _span: Span,
    ) {
        self.after_count += 1;
    }
    fn on_call(
        &mut self,
        _caller: &Frame,
        _callee: FuncId,
        _args: &[cb_backend_interp::value::Value],
    ) {
        self.call_count += 1;
    }
    fn on_return(&mut self, _frame: &Frame, _value: &cb_backend_interp::value::Value) {
        self.return_count += 1;
    }
}

fn compile_program(src: &str) -> (cb_ir::Program, cb_diagnostics::Interner) {
    let mut sources = SourceMap::new();
    let file = sources.add("test.cb".into(), src.into());

    let (tokens, lex_diags) = tokenize(src, file, LexerOptions::default());
    assert!(
        lex_diags
            .iter()
            .all(|d| !matches!(d.severity, cb_diagnostics::Severity::Error)),
        "lex errors: {lex_diags:?}"
    );

    let parse_result = parse(&tokens, src, file);
    assert!(
        parse_result
            .diagnostics
            .iter()
            .all(|d| !matches!(d.severity, cb_diagnostics::Severity::Error)),
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

    let sema_errors: Vec<_> = sema
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, cb_diagnostics::Severity::Error))
        .collect();
    assert!(sema_errors.is_empty(), "sema errors: {sema_errors:?}");

    let ir = cb_sema::lower::lower(&parse_result.arena, &parse_result.program, src, &mut sema);

    #[cfg(debug_assertions)]
    cb_ir::verify::verify(&ir);

    (ir, sema.interner)
}

#[test]
fn observer_counts_instructions() {
    let (ir, interner) = compile_program(r#"Print "hello""#);

    let mut output = Vec::new();
    {
        let mut interp = cb_backend_interp::Interpreter::new(&ir, &interner)
            .with_stdout(Box::new(&mut output as &mut dyn Write))
            .with_observer(CountingObserver::new());
        interp.run().expect("should succeed");
    }
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, "hello\n");
}

#[test]
fn observer_sees_function_calls() {
    let (ir, interner) = compile_program(
        "Function double(x As Integer) As Integer\n\
         Return x * 2\n\
         EndFunction\n\
         Print Str(double(21))",
    );

    let mut output = Vec::new();
    {
        let mut interp = cb_backend_interp::Interpreter::new(&ir, &interner)
            .with_stdout(Box::new(&mut output as &mut dyn Write))
            .with_observer(CountingObserver::new());
        interp.run().expect("should succeed");
    }
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(output_str, "42\n");
}

// FD-019: a user `Call`'s result is delivered to `after_inst`. When the Call
// pushes a frame the result isn't known yet, so the hook is deferred until the
// callee returns and fired against the call site — previously it was skipped
// entirely, leaving a debugger watching the call site blind to the result.
struct CallResultRecorder {
    call_results: std::rc::Rc<std::cell::RefCell<Vec<i32>>>,
}

impl Observer for CallResultRecorder {
    fn after_inst(
        &mut self,
        _frame: &Frame,
        inst: &InstKind,
        result: &cb_backend_interp::value::Value,
        _span: Span,
    ) {
        if let InstKind::Call { .. } = inst
            && let cb_backend_interp::value::Value::Int(v) = result
        {
            self.call_results.borrow_mut().push(*v);
        }
    }
}

#[test]
fn observer_sees_call_result() {
    let (ir, interner) = compile_program(
        "Function double(x As Integer) As Integer\n\
         Return x * 2\n\
         EndFunction\n\
         Print Str(double(21))",
    );

    let recorder = CallResultRecorder {
        call_results: Default::default(),
    };
    let seen = recorder.call_results.clone();
    let mut output = Vec::new();
    {
        let mut interp = cb_backend_interp::Interpreter::new(&ir, &interner)
            .with_stdout(Box::new(&mut output as &mut dyn Write))
            .with_observer(recorder);
        interp.run().expect("should succeed");
    }
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
    assert_eq!(
        *seen.borrow(),
        vec![42],
        "after_inst should observe the user call's result (42)"
    );
}

// FD-015: a runtime function that raises an error via the trap channel
// surfaces as an `Err(RuntimeError)` from `run` AND fires `on_runtime_error`.
struct ErrorRecorder {
    errors: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl Observer for ErrorRecorder {
    fn on_runtime_error(&mut self, _frame: &Frame, msg: &str, _span: Span) {
        self.errors.borrow_mut().push(msg.to_string());
    }
}

#[test]
fn observer_sees_runtime_error() {
    let (ir, interner) = compile_program("TestRaiseError(\"boom\")\n");

    let recorder = ErrorRecorder {
        errors: Default::default(),
    };
    let seen = recorder.errors.clone();

    let mut output = Vec::new();
    let result = {
        let mut interp = cb_backend_interp::Interpreter::new(&ir, &interner)
            .with_stdout(Box::new(&mut output as &mut dyn Write))
            .with_observer(recorder);
        interp.run()
    };

    let err = result.expect_err("raise_error should produce an Err");
    assert!(
        matches!(err.kind, InterpErrorKind::RuntimeError(ref m) if m == "boom"),
        "expected RuntimeError(\"boom\"), got {err:?}",
    );
    assert_eq!(*seen.borrow(), vec!["boom".to_string()]);
}

// ── Trap / error tests ────────────────────────────────────────────

fn run_err(src: &str) -> InterpError {
    let (ir, interner) = compile_program(src);
    let mut output = Vec::new();
    let mut interp = cb_backend_interp::Interpreter::new(&ir, &interner)
        .with_stdout(Box::new(&mut output as &mut dyn Write));
    interp.run().expect_err("expected interpreter error")
}

#[test]
fn trap_division_by_zero() {
    let err = run_err("Print Str(1 / 0)");
    assert!(matches!(
        err.kind,
        InterpErrorKind::Trap(TrapKind::DivisionByZero)
    ));
}

#[test]
fn trap_null_deref_field_access() {
    let err = run_err(
        "Type Obj\n\
           Field x As Int\n\
         EndType\n\
         Dim o As Obj\n\
         Print Str(o.x)",
    );
    assert!(matches!(
        err.kind,
        InterpErrorKind::Trap(TrapKind::NullDeref)
    ));
}

#[test]
fn trap_deleted_access_field() {
    let err = run_err(
        "Type Obj\n\
           Field x As Int\n\
         EndType\n\
         Dim o As Obj = New Obj\n\
         o.x = 42\n\
         Dim p As Obj = o\n\
         Delete p\n\
         Print Str(o.x)",
    );
    assert!(matches!(
        err.kind,
        InterpErrorKind::Trap(TrapKind::DeletedAccess)
    ));
}

#[test]
fn trap_index_out_of_bounds() {
    let err = run_err(
        "Dim arr As Int[] = New Int[3]\n\
         arr[5] = 10",
    );
    assert!(matches!(
        err.kind,
        InterpErrorKind::Trap(TrapKind::IndexOutOfBounds)
    ));
}

#[test]
fn trap_stack_overflow() {
    let err = run_err(
        "Function recurse() As Integer\n\
           Return recurse()\n\
         EndFunction\n\
         Print Str(recurse())",
    );
    assert!(
        matches!(err.kind, InterpErrorKind::RuntimeError(ref msg) if msg.contains("stack overflow"))
    );
}

#[test]
fn trap_double_delete() {
    let err = run_err(
        "Type Obj\n\
           Field x As Int\n\
         EndType\n\
         Dim o As Obj = New Obj\n\
         Delete o\n\
         Delete o",
    );
    assert!(matches!(
        err.kind,
        InterpErrorKind::Trap(TrapKind::DoubleDelete)
    ));
}

// ── FD-019: interpreter correctness & memory-safety regressions ─────────

#[test]
fn shift_right_logical_on_negative() {
    // `Shr` is a logical right shift: the sign bit must NOT be replicated.
    // (-1) Shr 1 == 0x7FFFFFFF, not -1.
    let out = run("Dim r As Int = -1\nPrint Str(r Shr 1)");
    assert_eq!(out, "2147483647\n");
}

#[test]
fn shift_arithmetic_preserves_sign() {
    let out = run("Dim r As Int = -8\nPrint Str(r Sar 1)");
    assert_eq!(out, "-4\n");
}

#[test]
fn shift_left_count_reduced_to_width() {
    // A shift count >= the operand width is reduced modulo it (33 -> 1).
    let out = run("Dim r As Int = 1\nPrint Str(r Shl 33)");
    assert_eq!(out, "2\n");
}

#[test]
fn shift_right_basic() {
    let out = run("Dim r As Int = 8\nPrint Str(r Shr 2)");
    assert_eq!(out, "2\n");
}

#[test]
fn value_struct_field_write_then_read() {
    // The defining FD-019 bug #2 case: a write to a value-struct field must
    // persist (it previously updated a throwaway register copy).
    let out = run("Struct Vec2\n\
           Field x As Int\n\
           Field y As Int\n\
         EndStruct\n\
         Dim p As Vec2\n\
         p.x = 7\n\
         p.y = 9\n\
         Print Str(p.x + p.y)");
    assert_eq!(out, "16\n");
}

#[test]
fn value_struct_nested_field_write() {
    let out = run("Struct Inner\n\
           Field v As Int\n\
         EndStruct\n\
         Struct Outer\n\
           Field inner As Inner\n\
           Field w As Int\n\
         EndStruct\n\
         Dim o As Outer\n\
         o.inner.v = 5\n\
         o.w = 3\n\
         Print Str(o.inner.v + o.w)");
    assert_eq!(out, "8\n");
}

#[test]
fn value_struct_copy_semantics() {
    // Assigning a struct copies it; mutating the copy must not affect the
    // original.
    let out = run("Struct Vec2\n\
           Field x As Int\n\
         EndStruct\n\
         Dim p As Vec2\n\
         p.x = 1\n\
         Dim q As Vec2 = p\n\
         q.x = 99\n\
         Print Str(p.x)");
    assert_eq!(out, "1\n");
}

#[test]
fn array_of_structs_field_write_read() {
    let out = run("Struct P\n\
           Field x As Int\n\
         EndStruct\n\
         Dim arr As P[] = New P[3]\n\
         arr[1].x = 42\n\
         Print Str(arr[1].x)");
    assert_eq!(out, "42\n");
}

#[test]
fn array_of_structs_defaults_to_zero_struct() {
    // FD-019 bug #4: array-of-struct elements default to a zero-initialised
    // struct, not Null — field access must not trap.
    let out = run("Struct P\n\
           Field x As Int\n\
         EndStruct\n\
         Dim arr As P[] = New P[2]\n\
         Print Str(arr[0].x)");
    assert_eq!(out, "0\n");
}

#[test]
fn array_negative_dimension_is_clean_error() {
    // FD-019 bug #3: a negative dimension must be a clean RuntimeError, not a
    // multi-exabyte allocation that aborts the process.
    let err = run_err(
        "Dim n As Int = -1\n\
         Dim arr As Int[] = New Int[n]",
    );
    assert!(
        matches!(err.kind, InterpErrorKind::RuntimeError(ref m) if m.contains("negative array dimension")),
        "unexpected error: {err:?}"
    );
}

#[test]
fn redim_negative_dimension_is_clean_error() {
    let err = run_err(
        "Dim arr As Int[] = New Int[2]\n\
         Dim n As Int = -5\n\
         Redim arr As Int[n]",
    );
    assert!(
        matches!(err.kind, InterpErrorKind::RuntimeError(ref m) if m.contains("negative array dimension")),
        "unexpected error: {err:?}"
    );
}

// ── FD-032: first-class functions (address-of + indirect call) ─────────

#[test]
fn function_pointer_roundtrip() {
    // Take a function's address with its bare name (cb_syntax.md §7.4), store it
    // in a Function(...) variable, and call through it. Exercises the new
    // FuncAddr instruction plus the CallIndirect success arm end-to-end.
    let out = run("Function add(a As Integer, b As Integer) As Integer\n\
           Return a + b\n\
         EndFunction\n\
         Dim fp As Function(Integer, Integer) As Integer\n\
         fp = add\n\
         Print Str(fp(2, 3))");
    assert_eq!(out, "5\n");
}

#[test]
fn function_pointer_as_argument() {
    // A function name passed as an argument lowers to FuncAddr; the callee then
    // invokes the fn-pointer parameter via CallIndirect (higher-order call).
    let out = run("Function apply(f As Function(Integer) As Integer, x As Integer) As Integer\n\
           Return f(x)\n\
         EndFunction\n\
         Function inc(n As Integer) As Integer\n\
           Return n + 1\n\
         EndFunction\n\
         Print Str(apply(inc, 41))");
    assert_eq!(out, "42\n");
}

#[test]
fn function_pointer_reassigned_picks_right_target() {
    // Two addresses through one variable must dispatch to the right function.
    let out = run("Function pick_a(x As Integer) As Integer\n\
           Return x + 10\n\
         EndFunction\n\
         Function pick_b(x As Integer) As Integer\n\
           Return x + 20\n\
         EndFunction\n\
         Dim fp As Function(Integer) As Integer\n\
         fp = pick_a\n\
         Print Str(fp(5))\n\
         fp = pick_b\n\
         Print Str(fp(5))");
    assert_eq!(out, "15\n25\n");
}

#[test]
fn trap_null_function_pointer() {
    // The sixth TrapKind, previously never fired in a test: calling a null
    // fn-pointer traps NullFnPtr (cb_syntax.md §9.2).
    let err = run_err(
        "Dim fp As Function(Integer) As Integer\n\
         fp = Null\n\
         Print Str(fp(0))",
    );
    assert!(matches!(err.kind, InterpErrorKind::Trap(TrapKind::NullFnPtr)));
}

// ── FD-032: multi-dimensional arrays ───────────────────────────────────

#[test]
fn array_len_by_dimension() {
    // Len(arr) is axis 0; Len(arr, n) selects dimension n — the Len{dim:Some}
    // path, previously unexercised for a multi-dim array.
    let out = run("Dim grid As Int[,] = New Int[2, 3]\n\
         Print Str(Len(grid))\n\
         Print Str(Len(grid, 0))\n\
         Print Str(Len(grid, 1))");
    assert_eq!(out, "2\n2\n3\n");
}

// ── FD-032: heap lifecycle under iteration ─────────────────────────────

#[test]
fn for_each_multiple_mid_list_deletes() {
    // Delete-with-rewind must survive *several* deletions in one pass, not just
    // one (extends type_delete_and_continue_iteration). Delete the 2nd and 4th
    // of five siblings; the survivors (1, 3, 5) remain iterable -> total 9.
    let out = run("Type Node\n\
           Field v As Int\n\
         EndType\n\
         Dim a As Node = New Node\n\
         a.v = 1\n\
         Dim b As Node = New Node\n\
         b.v = 2\n\
         Dim c As Node = New Node\n\
         c.v = 3\n\
         Dim d As Node = New Node\n\
         d.v = 4\n\
         Dim e As Node = New Node\n\
         e.v = 5\n\
         For n = Each Node\n\
           If n.v = 2 Or n.v = 4 Then\n\
             Delete n\n\
           EndIf\n\
         Next n\n\
         Dim total As Int = 0\n\
         For m = Each Node\n\
           total = total + m.v\n\
         Next m\n\
         Print Str(total)");
    assert_eq!(out, "9\n");
}

// ── FD-032: narrow integer widths (cb_syntax.md §3.1/§3.4) ─────────────

// NOTE: the following three tests are #[ignore]d because FD-032 surfaced a
// pre-existing numeric bug now tracked as FD-035: `Dim x As <narrow/unsigned> =
// <int literal>` is not coerced to the declared type (`check_dim` only checks
// the init, never `coerce`s it), so the variable holds a plain `Int`. A shift on
// it then dispatches as 32-bit signed (`eval_binop` also needs LHS-type dispatch
// for shift counts). Un-ignore these as part of FD-035.

#[test]
#[ignore = "FD-035: Dim-init not coerced to declared narrow type; UInt holds Int -> signed shift"]
fn uint_shift_stays_unsigned_32bit() {
    // A UInt shift stays unsigned: 1 Shl 31 = 2147483648, which a signed Int
    // could not represent (it would be -2147483648). Exercises uint_binop.
    let out = run("Dim u As UInt = 1\nPrint Str(u Shl 31)");
    assert_eq!(out, "2147483648\n");
}

#[test]
#[ignore = "FD-035: Dim-init not coerced to declared narrow type; ULong holds Int -> signed 32-bit shift"]
fn ulong_shift_stays_unsigned_64bit() {
    // A ULong shift stays unsigned 64-bit: 1 Shl 63 overflows a signed Long.
    let out = run("Dim u As ULong = 1\nPrint Str(u Shl 63)");
    assert_eq!(out, "9223372036854775808\n");
}

#[test]
#[ignore = "FD-035: Dim-init not coerced to declared type; `s` stays Int, so this would not actually test Short storage (also Value::Short(i16) vs documented unsigned)"]
fn short_holds_documented_unsigned_range() {
    // cb_syntax.md §3.1: Short is 16-bit UNSIGNED, so 40000 is in range. (Also
    // probes the Value::Short(i16) signed-vs-documented-unsigned mismatch once
    // the Dim-init coercion lands.)
    let out = run("Dim s As Short = 40000\nPrint Str(s)");
    assert_eq!(out, "40000\n");
}

#[test]
fn byte_wraps_modulo_256_on_assignment() {
    // Byte is 8-bit unsigned: `b + 100` (= 300) narrows to 44 (300 mod 256) on
    // the store-back `Convert`. This exercises the assignment-narrowing path
    // (which DOES coerce, unlike a Dim init), so it is unaffected by the Dim-init
    // gap above. A non-literal value avoids the E0326 literal-overflow error.
    let out = run("Dim b As Byte = 200\nb = b + 100\nPrint Str(b)");
    assert_eq!(out, "44\n");
}

// ── FD-032: observer across nested calls ───────────────────────────────

#[test]
fn observer_sees_nested_call_results() {
    // Deferred call-result delivery must work across nested calls, not just one
    // level deep (extends observer_sees_call_result). inner returns first (21),
    // then outer (42).
    let (ir, interner) = compile_program(
        "Function inner(x As Integer) As Integer\n\
           Return x + 1\n\
         EndFunction\n\
         Function outer(y As Integer) As Integer\n\
           Return inner(y) * 2\n\
         EndFunction\n\
         Print Str(outer(20))",
    );
    let recorder = CallResultRecorder {
        call_results: Default::default(),
    };
    let seen = recorder.call_results.clone();
    let mut output = Vec::new();
    {
        let mut interp = cb_backend_interp::Interpreter::new(&ir, &interner)
            .with_stdout(Box::new(&mut output as &mut dyn Write))
            .with_observer(recorder);
        interp.run().expect("should succeed");
    }
    assert_eq!(String::from_utf8(output).unwrap(), "42\n");
    assert_eq!(
        *seen.borrow(),
        vec![21, 42],
        "after_inst should observe both nested call results, innermost first"
    );
}
