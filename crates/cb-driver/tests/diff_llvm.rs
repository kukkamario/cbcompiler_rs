//! Differential AOT tests (FD-049): the interpreter is the oracle, the LLVM
//! backend is the unit under test. For each fixture we run `cb --backend interp`
//! (its stdout + exit code), then `cb --backend llvm -o <tmp> <file>`, run the
//! produced native exe, and assert the two agree on **both** newline-normalized
//! stdout and exit code. The driver only *emits* the exe — this harness runs it.
//!
//! Gated on both backends being present (`--features llvm` keeps `interp`). The
//! fixtures are the Phase-1 scalar core + control flow + user functions +
//! Allegro-free runtime calls + strings, plus the Phase-2 array surface
//! (Dim/New/index/Redim/Len/For Each); user Types / graphics remain out of
//! scope and excluded.
#![cfg(all(feature = "llvm", feature = "interp"))]

use std::path::PathBuf;
use std::process::Command;

/// The built `cb` binary (Cargo sets `CARGO_BIN_EXE_cb` for integration tests).
fn cb() -> &'static str {
    env!("CARGO_BIN_EXE_cb")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/programs")
}

/// Normalize line endings so a text-mode C `stdout` (`\r\n` on Windows) compares
/// equal to the interpreter's raw `\n`.
fn normalise(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// Captured outputs from running a fixture through both backends.
struct Outcome {
    want_stdout: String,
    want_code: Option<i32>,
    got_stdout: String,
    got_code: Option<i32>,
    /// The produced native exe's raw stderr (for trap-message assertions).
    got_stderr: Vec<u8>,
}

/// Run a fixture through both backends: the interpreter (oracle) and the
/// llvm-compiled native exe. Returns their stdout/exit-code/stderr for the
/// caller to assert on.
fn run_both(name: &str) -> Outcome {
    let src = fixtures_dir().join(format!("{name}.cb"));

    // Oracle: the interpreter's stdout + exit code.
    let oracle = Command::new(cb())
        .arg(&src)
        .output()
        .unwrap_or_else(|e| panic!("run interp on {name}: {e}"));

    // Compile via the llvm backend to a throwaway native exe.
    let tmp = tempfile::tempdir().expect("temp dir");
    let exe = tmp
        .path()
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let compile = Command::new(cb())
        .args(["--backend", "llvm", "-o"])
        .arg(&exe)
        .arg(&src)
        .output()
        .unwrap_or_else(|e| panic!("compile {name} via llvm: {e}"));
    assert!(
        compile.status.success(),
        "llvm compile of {name} failed (exit {:?}):\n{}",
        compile.status.code(),
        String::from_utf8_lossy(&compile.stderr)
    );

    // Run the produced exe.
    let run = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("run produced exe for {name}: {e}"));

    Outcome {
        want_stdout: normalise(&oracle.stdout),
        want_code: oracle.status.code(),
        got_stdout: normalise(&run.stdout),
        got_code: run.status.code(),
        got_stderr: run.stderr,
    }
}

/// Assert stdout + exit code agree between the two backends. Both exit codes
/// must be `Some`: on Unix a signal-killed process yields `None`, so a
/// `SIGSEGV`/`SIGABRT` could otherwise compare equal-by-absence (F18).
fn assert_agree(name: &str, o: &Outcome) {
    assert_eq!(
        o.want_stdout, o.got_stdout,
        "stdout mismatch for {name}: interp vs llvm"
    );
    assert!(
        o.want_code.is_some(),
        "{name}: interp produced no exit code (signal-killed?)"
    );
    assert!(
        o.got_code.is_some(),
        "{name}: produced exe produced no exit code (signal-killed?)"
    );
    assert_eq!(
        o.want_code, o.got_code,
        "exit-code mismatch for {name}: interp vs llvm"
    );
}

fn run_diff(name: &str) {
    let o = run_both(name);
    assert_agree(name, &o);
}

/// A fault fixture: in addition to stdout/exit-code parity, the produced exe
/// must have written a (non-empty) trap message to stderr — guarding against a
/// silent or signal-kill divergence on the LLVM side (F18). The exact text is
/// not compared (it differs between the Rust interp and the C runtime).
fn run_diff_trap(name: &str) {
    let o = run_both(name);
    assert_agree(name, &o);
    assert!(
        !o.got_stderr.is_empty(),
        "trap fixture {name}: produced exe wrote no stderr trap message"
    );
}

macro_rules! diff_tests {
    ($($name:ident),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_diff(stringify!($name));
            }
        )+
    };
}

/// Like `diff_tests!` but for fault fixtures, routed through `run_diff_trap`.
macro_rules! diff_trap_tests {
    ($($name:ident),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_diff_trap(stringify!($name));
            }
        )+
    };
}

// Scalar core + control flow + user functions (pure Phase-1, no runtime calls
// beyond Print).
diff_tests! {
    int_arithmetic,
    shift_mixed_width,
    byte_short_overflow,
    float_formatting,
    mixed_arithmetic,
    string_ops,
    if_elseif_else,
    float_condition,
    nested_for_loops,
    select_case,
    function_multi_param,
    recursion_factorial,
    recursion_fibonacci,
    mutual_recursion,
    sigil_optional,
}

// Allegro-free runtime calls (Math / String library / System). These link the
// SDK-free runtime closure on CI and the full closure locally.
diff_tests! {
    runtime_math,
    runtime_sqrt,
    runtime_string,
    runtime_string_fd017,
}

// Arrays (FD-049 Phase 2): Dim/New/index/Redim/Len/For Each over native heap
// arrays. Fault fixtures (out-of-bounds / empty / negative-index traps) live in
// the `diff_trap_tests!` block below.
diff_tests! {
    array_1d,
    array_multidim,
    array_redim,
    array_redim_grow,
    array_redim_shrink,
    array_redim_multidim,
    array_redim_global,
    array_len,
    array_foreach,
    array_string,
    array_param,
}

// User Types (FD-049 Phase 3a): New/field access, the type-instance linked list
// (First/Last/Next/Previous + For Each), Delete (lvalue rewind + rvalue),
// field-projection StorePlace, and reference equality. These link the new
// cb_type.cpp heap helpers.
diff_tests! {
    type_list_sum,
    type_multi_field,
    type_modify_in_function,
    type_pass_to_function,
    type_inference_fd042,
    type_first_each_delete,
    type_previous,
    type_ref_equality,
}

// Value structs (FD-049 Phase 3b): inline LLVM aggregates with value semantics —
// scalar field read/write, value-copy independence, nested `a.b.c`, struct array
// elements, String-field copy/reassign refcount, and by-value param copy.
diff_tests! {
    struct_field_rw,
    struct_copy_value,
    struct_nested,
    struct_array_elem,
    struct_array_string,
    struct_string_field,
    struct_param_value,
    struct_return_string,
}

// Function pointers (FD-049 Phase 3c): FuncAddr (bare-name address-of) and
// CallIndirect through a `Function(...)` value. The null-call fault fixture
// lives in the `diff_trap_tests!` block below.
diff_tests! {
    fnptr_call,
    fnptr_param,
    fnptr_null_equality,
}

// Fault fixtures: programs that trap at runtime (out-of-bounds / empty /
// negative array index, null fn-ptr call). Beyond stdout + exit-code parity,
// these assert the produced exe wrote a non-empty stderr trap message and that
// neither side was signal-killed (F18). Both backends print their pre-trap
// output, then trap and exit 1.
diff_trap_tests! {
    array_oob,
    array_empty,
    array_negative_index,
    fnptr_null,
}
