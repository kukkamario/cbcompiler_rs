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

fn run_diff(name: &str) {
    let src = fixtures_dir().join(format!("{name}.cb"));

    // Oracle: the interpreter's stdout + exit code.
    let oracle = Command::new(cb())
        .arg(&src)
        .output()
        .unwrap_or_else(|e| panic!("run interp on {name}: {e}"));
    let want_stdout = normalise(&oracle.stdout);
    let want_code = oracle.status.code();

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

    // Run the produced exe and compare.
    let run = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("run produced exe for {name}: {e}"));
    let got_stdout = normalise(&run.stdout);
    let got_code = run.status.code();

    assert_eq!(
        want_stdout, got_stdout,
        "stdout mismatch for {name}: interp vs llvm"
    );
    assert_eq!(
        want_code, got_code,
        "exit-code mismatch for {name}: interp vs llvm"
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

// Scalar core + control flow + user functions (pure Phase-1, no runtime calls
// beyond Print).
diff_tests! {
    int_arithmetic,
    float_formatting,
    mixed_arithmetic,
    string_ops,
    if_elseif_else,
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
// arrays. `array_oob` is a fault fixture — both backends print `before`, then
// the out-of-bounds index traps and exits 1.
diff_tests! {
    array_1d,
    array_multidim,
    array_redim,
    array_len,
    array_foreach,
    array_string,
    array_param,
    array_oob,
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
    type_ref_equality,
}
