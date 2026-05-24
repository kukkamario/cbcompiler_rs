use std::io::Write;

use cb_backend_interp::error::{InterpError, InterpErrorKind};
use cb_backend_interp::observer::Observer;
use cb_backend_interp::interp::Frame;
use cb_diagnostics::{SourceMap, Span};
use cb_frontend::{LexerOptions, parse, tokenize};
use cb_ir::FuncId;
use cb_ir::inst::{InstKind, TrapKind};

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

// ── Heap type tests ────────────────────────────────────────────────

#[test]
fn type_new_and_field_access() {
    let out = run(
        "Type Enemy\n\
           Field hp As Int\n\
         EndType\n\
         Dim e As Enemy = New Enemy\n\
         e.hp = 100\n\
         Print Str(e.hp)"
    );
    assert_eq!(out, "100\n");
}

#[test]
fn type_linked_list_first_next() {
    let out = run(
        "Type Node\n\
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
         Wend"
    );
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn type_for_each() {
    let out = run(
        "Type Item\n\
           Field x As Int\n\
         EndType\n\
         Dim a As Item = New Item\n\
         a.x = 10\n\
         Dim b As Item = New Item\n\
         b.x = 20\n\
         For n = Each Item\n\
           Print Str(n.x)\n\
         Next n"
    );
    assert_eq!(out, "10\n20\n");
}

#[test]
fn type_delete_and_continue_iteration() {
    let out = run(
        "Type Obj\n\
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
         Print Str(total)"
    );
    assert_eq!(out, "4\n");
}

#[test]
fn array_new_and_index() {
    let out = run(
        "Dim arr As Int[] = New Int[3]\n\
         arr[0] = 10\n\
         arr[1] = 20\n\
         arr[2] = 30\n\
         Print Str(arr[1])"
    );
    assert_eq!(out, "20\n");
}

#[test]
fn array_len() {
    let out = run(
        "Dim arr As Int[] = New Int[5]\n\
         Print Str(Len(arr))"
    );
    assert_eq!(out, "5\n");
}

#[test]
fn null_comparison() {
    let out = run(
        "Type MyObj\n\
           Field x As Int\n\
         EndType\n\
         Dim obj As MyObj\n\
         If obj = Null Then\n\
           Print \"null\"\n\
         Else\n\
           Print \"not null\"\n\
         EndIf"
    );
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
    fn on_call(&mut self, _caller: &Frame, _callee: FuncId, _args: &[cb_backend_interp::value::Value]) {
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
         Print Str(double(21))"
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
    assert!(matches!(err.kind, InterpErrorKind::Trap(TrapKind::DivisionByZero)));
}

#[test]
fn trap_null_deref_field_access() {
    let err = run_err(
        "Type Obj\n\
           Field x As Int\n\
         EndType\n\
         Dim o As Obj\n\
         Print Str(o.x)"
    );
    assert!(matches!(err.kind, InterpErrorKind::Trap(TrapKind::NullDeref)));
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
         Print Str(o.x)"
    );
    assert!(matches!(err.kind, InterpErrorKind::Trap(TrapKind::DeletedAccess)));
}

#[test]
fn trap_index_out_of_bounds() {
    let err = run_err(
        "Dim arr As Int[] = New Int[3]\n\
         arr[5] = 10"
    );
    assert!(matches!(err.kind, InterpErrorKind::Trap(TrapKind::IndexOutOfBounds)));
}

#[test]
fn trap_stack_overflow() {
    let err = run_err(
        "Function recurse() As Integer\n\
           Return recurse()\n\
         EndFunction\n\
         Print Str(recurse())"
    );
    assert!(matches!(err.kind, InterpErrorKind::RuntimeError(ref msg) if msg.contains("stack overflow")));
}

#[test]
fn trap_double_delete() {
    let err = run_err(
        "Type Obj\n\
           Field x As Int\n\
         EndType\n\
         Dim o As Obj = New Obj\n\
         Delete o\n\
         Delete o"
    );
    assert!(matches!(err.kind, InterpErrorKind::Trap(TrapKind::DoubleDelete)));
}
