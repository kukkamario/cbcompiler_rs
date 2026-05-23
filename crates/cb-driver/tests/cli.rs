//! End-to-end tests for the `cb` driver binary.
//!
//! Spawns the built binary via `assert_cmd` (which uses Cargo's
//! `CARGO_BIN_EXE_cb` env var) and asserts on exit codes, stderr
//! patterns, and a snapshot of the AST dump. Tempfile inputs keep the
//! fixtures next to the test bodies.

use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::{TempDir, tempdir};

fn write_cb(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    fs::write(&p, body).expect("write fixture");
    p
}

#[test]
fn valid_file_exits_zero_and_dumps_ast() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    let out = Command::cargo_bin("cb")
        .unwrap()
        .arg("--dump-ast")
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    // Path is included in the program header? No — the driver does not
    // echo the input path on stdout. The dump is path-independent, so
    // it snapshots cleanly.
    insta::assert_snapshot!(stdout, @r"
    Program (1 top-level statements):
      Stmt::Dim @ 0..16
        TypeExpr::Primitive @ 9..12
        Expr::IntLit @ 15..16
    ");
}

#[test]
fn lex_error_exits_one() {
    let dir = tempdir().unwrap();
    // `@` is not a recognised token — lexer emits E0106 ("unexpected
    // character"). Confirm both the exit code and the code reaches
    // stderr through the renderer.
    let path = write_cb(&dir, "bad.cb", "@\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stderr(contains("E0106"));
}

#[test]
fn parse_error_exits_one() {
    let dir = tempdir().unwrap();
    // `If` with no condition / `Then` clause — parser surfaces the
    // missing expression as `error[E…]:` on stderr.
    let path = write_cb(&dir, "bad.cb", "If\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stderr(contains("error["));
}

#[test]
fn missing_arg_exits_two() {
    Command::cargo_bin("cb")
        .unwrap()
        .assert()
        .code(2)
        .stderr(contains("usage: cb"));
}

#[test]
fn missing_file_exits_two() {
    Command::cargo_bin("cb")
        .unwrap()
        .arg("definitely-does-not-exist-cb-test-fixture.cb")
        .assert()
        .code(2)
        .stderr(contains("failed to read"));
}

#[test]
fn errors_dominate_warnings() {
    // A narrowing conversion (E0318 warning) plus a lex error (@) → exit 1.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "mixed.cb", "Dim x As Integer\nx = 1.5\n@\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1);
}

#[test]
fn sema_error_exits_one() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "sema.cb", "Dim y As Integer\ny = x + 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stderr(contains("E0300"));
}

#[test]
fn sema_narrowing_warning_exits_zero() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "warn.cb", "Dim x As Integer\nx = 1.5\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .success()
        .stderr(contains("E0318"));
}

#[test]
fn backend_flag_accepts_interp() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .args(["--backend", "interp"])
        .arg(&path)
        .assert()
        .success();
}

#[cfg(not(feature = "llvm"))]
#[test]
fn backend_flag_rejects_uncompiled_llvm() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .args(["--backend", "llvm"])
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("not compiled in"));
}

#[test]
fn backend_flag_rejects_unknown_name() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .args(["--backend", "bogus"])
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("unknown backend"));
}

#[test]
fn backend_flag_missing_value_exits_two() {
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--backend")
        .assert()
        .code(2)
        .stderr(contains("--backend requires a value"));
}
