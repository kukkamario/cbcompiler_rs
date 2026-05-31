//! End-to-end tests: spawn `cb.exe` on a `.cb` fixture and assert its
//! stdout matches the sibling `.out` file byte-for-byte (after line-ending
//! normalisation). Each fixture lives in `tests/fixtures/programs/` as a
//! pair `<name>.cb` + `<name>.out`. Adding a new test = write the pair +
//! one `#[test] fn name() { run("name") }`.

use std::path::PathBuf;

use assert_cmd::Command;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/programs")
}

fn run(name: &str) {
    let dir = fixtures_dir();
    let cb_path = dir.join(format!("{name}.cb"));
    let out_path = dir.join(format!("{name}.out"));
    let expected = std::fs::read_to_string(&out_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", out_path.display()));

    let output = Command::cargo_bin("cb")
        .unwrap()
        .arg(&cb_path)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");

    // Rust's writeln! emits \n; git on Windows may rewrite checked-out .out
    // files to \r\n. Normalise both sides so the test is portable.
    let normalise = |s: &str| s.replace("\r\n", "\n");
    assert_eq!(
        normalise(&stdout),
        normalise(&expected),
        "stdout mismatch for {name}.cb",
    );
}

// Type system ------------------------------------------------------------

#[test]
fn int_arithmetic() {
    run("int_arithmetic");
}

#[test]
fn float_formatting() {
    run("float_formatting");
}

#[test]
fn mixed_arithmetic() {
    run("mixed_arithmetic");
}

#[test]
fn string_ops() {
    run("string_ops");
}

// Functions and recursion ------------------------------------------------

#[test]
fn function_multi_param() {
    run("function_multi_param");
}

#[test]
fn recursion_factorial() {
    run("recursion_factorial");
}

#[test]
fn recursion_fibonacci() {
    run("recursion_fibonacci");
}

#[test]
fn mutual_recursion() {
    run("mutual_recursion");
}

// User-defined Type ------------------------------------------------------

#[test]
fn type_multi_field() {
    run("type_multi_field");
}

#[test]
fn type_pass_to_function() {
    run("type_pass_to_function");
}

#[test]
fn type_modify_in_function() {
    run("type_modify_in_function");
}

#[test]
fn type_list_sum() {
    run("type_list_sum");
}

// Control flow -----------------------------------------------------------

#[test]
fn nested_for_loops() {
    run("nested_for_loops");
}

#[test]
fn select_case() {
    run("select_case");
}

#[test]
fn sigil_optional() {
    run("sigil_optional");
}

#[test]
fn if_elseif_else() {
    run("if_elseif_else");
}

// Runtime functions ------------------------------------------------------

#[test]
fn runtime_sqrt() {
    run("runtime_sqrt");
}

#[test]
fn runtime_math() {
    run("runtime_math");
}

#[test]
fn runtime_string() {
    run("runtime_string");
}

#[test]
fn runtime_string_fd017() {
    run("runtime_string_fd017");
}

#[test]
fn runtime_system() {
    run("runtime_system");
}

#[test]
fn runtime_image() {
    run("runtime_image");
}

#[test]
fn runtime_gfx_fd017() {
    run("runtime_gfx_fd017");
}

#[test]
fn runtime_image_fd017() {
    run("runtime_image_fd017");
}

#[test]
fn runtime_text_fd018() {
    run("runtime_text_fd018");
}

#[test]
fn runtime_input() {
    run("runtime_input");
}
