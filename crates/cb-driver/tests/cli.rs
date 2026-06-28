//! End-to-end tests for the `cb` driver binary.
//!
//! Spawns the built binary via `assert_cmd` (which uses Cargo's
//! `CARGO_BIN_EXE_cb` env var) and asserts on exit codes, stderr
//! patterns, and a snapshot of the AST dump. Tempfile inputs keep the
//! fixtures next to the test bodies.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::{contains, is_empty};
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
    // No positional `<FILE>` — clap reports the missing required argument and
    // exits 2 (its default for usage errors, which matches the driver's own
    // usage exit code).
    Command::cargo_bin("cb")
        .unwrap()
        .assert()
        .code(2)
        .stderr(contains("Usage: cb"));
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
fn infer_from_null_exits_one_e0331() {
    // An implicit declaration cannot infer a type from `Null` — E0331.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "infer_null.cb", "x = Null\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stderr(contains("E0331"));
}

#[cfg(feature = "interp")]
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

#[cfg(feature = "interp")]
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
    // `--backend` with no following value — clap reports the missing value and
    // exits 2.
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--backend")
        .assert()
        .code(2)
        .stderr(contains("--backend").and(contains("value is required")));
}

#[test]
fn runtime_print_typechecks_and_lowers() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "hello.cb", "Print(\"hello world\")\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--dump-ir")
        .arg(&path)
        .assert()
        .success()
        .stdout(contains("call print"))
        .stdout(contains("const_string \"hello world\""));
}

#[cfg(feature = "interp")]
#[test]
fn make_error_writes_stderr_and_exits_one() {
    // `MakeError(msg)` writes the message to stderr and terminates with a
    // non-zero exit code; the statement after it must not run.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "err.cb",
        "Print \"before\"\nMakeError(\"boom happened\")\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stdout(contains("before"))
        .stdout(contains("after").not())
        .stderr(contains("boom happened"));
}

#[cfg(feature = "interp")]
#[test]
fn end_statement_exits_zero() {
    // A bare `End` terminates cleanly (exit 0); code after it does not run.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "end.cb", "Print \"hi\"\nEnd\nPrint \"unreachable\"\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .success()
        .stdout(contains("hi"))
        .stdout(contains("unreachable").not());
}

#[cfg(feature = "interp")]
#[test]
fn runtime_request_exit_sets_exit_code() {
    // Trap channel: a runtime function calling the host's
    // `request_exit(code)` terminates cleanly with that code (via the
    // Exit→Ok(code) path), and the statement after it must not run.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "req_exit.cb",
        "Print \"before\"\nTestRequestExit(7)\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(7)
        .stdout(contains("before"))
        .stdout(contains("after").not());
}

#[cfg(feature = "interp")]
#[test]
fn runtime_raise_error_writes_stderr_and_exits_one() {
    // Trap channel: a runtime function calling the host's
    // `raise_error(msg)` aborts with exit 1, renders "runtime error: <msg>"
    // to stderr, and the statement after it must not run.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "raise_err.cb",
        "Print \"before\"\nTestRaiseError(\"boom\")\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stdout(contains("before"))
        .stdout(contains("after").not())
        .stderr(contains("runtime error: boom"));
}

#[cfg(feature = "interp")]
#[test]
fn particle_command_on_non_emitter_traps() {
    // The Particle* commands are typed to take an Object, so the checker
    // can't distinguish a plain object from an emitter. A plain object reaching
    // ParticleMovement traps via the trap channel (classic CB blind-casts → UB;
    // we trap rather than silently no-op). Graphics-gated — MakeObject /
    // ParticleMovement only exist in the full Allegro build.
    if !cb_runtime_sys::HAS_GRAPHICS {
        eprintln!("skipping: SDK-free runtime build has no graphics");
        return;
    }
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "emit_trap.cb",
        "Dim o As Object\no = MakeObject()\nParticleMovement(o, 1.0, 0.1)\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stdout(contains("after").not())
        .stderr(contains("runtime error: ParticleMovement"))
        .stderr(contains("not a particle emitter"));
}

#[cfg(feature = "interp")]
#[test]
fn memblock_out_of_bounds_traps() {
    // An out-of-range Peek/Poke traps via the trap channel instead of
    // corrupting memory (classic CB blind-casts → UB; we trap). A
    // 4-byte block can't hold a 4-byte int at offset 2, so PokeInt aborts with
    // exit 1 and the statement after it does not run. Not graphics-gated —
    // memory blocks are Allegro-free and present in the SDK-free build too.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "mb_oob.cb",
        "Dim m As Memblock\nm = MakeMEMBlock(4)\nPrint \"before\"\nPokeInt(m, 2, 123)\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stdout(contains("before"))
        .stdout(contains("after").not())
        .stderr(contains("runtime error: PokeInt"))
        .stderr(contains("out of bounds"));
}

#[cfg(feature = "interp")]
#[test]
fn file_op_on_null_handle_traps() {
    // A read/write on a null (never-opened) File handle traps via the
    // trap channel — exit 1, and the statement after it does not run. Classic
    // CB is permissive; we refuse. Allegro-free, so present
    // in the SDK-free build too.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "null_file.cb",
        "Dim f As File\nPrint \"before\"\nPrint Str(ReadByte(f))\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(1)
        .stdout(contains("before"))
        .stdout(contains("after").not())
        .stderr(contains("runtime error: ReadByte"))
        .stderr(contains("invalid file handle"));
}

#[cfg(feature = "interp")]
#[test]
fn delete_sound_on_null_handle_traps_when_audio_available() {
    // DeleteSound on a null (never-loaded) `Sound` raises the trap
    // ("invalid sound handle") — exit 1,
    // and "after" is not reached. But we deliberately SUPPRESS that trap on
    // an audio-less host (LoadSound would have returned Null there through no fault
    // of the program), so on a silent box the op is a no-op and the program runs
    // to completion. Graphics-gated (the Sound funcs exist only in the full Allegro
    // build); within that, tolerant of whether a real audio device is present so
    // CI never flakes either way.
    if !cb_runtime_sys::HAS_GRAPHICS {
        eprintln!("skipping: SDK-free runtime build has no audio");
        return;
    }
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "null_sound.cb",
        "Dim s As Sound\nPrint \"before\"\nDeleteSound s\nPrint \"after\"\n",
    );
    let output = Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("before"), "stdout: {stdout}");
    match output.status.code() {
        // Audio available: the trap fired, halting before "after".
        Some(1) => {
            assert!(!stdout.contains("after"), "trap should halt before 'after'");
            assert!(
                stderr.contains("runtime error: DeleteSound"),
                "stderr: {stderr}"
            );
            assert!(stderr.contains("invalid sound handle"), "stderr: {stderr}");
        }
        // Audio unavailable: the trap is suppressed and the program runs on.
        Some(0) => assert!(
            stdout.contains("after"),
            "suppressed trap should run to completion; stdout: {stdout}"
        ),
        other => panic!("unexpected exit code {other:?}; stderr: {stderr}"),
    }
}

#[cfg(feature = "interp")]
#[test]
fn copy_file_over_existing_traps() {
    // CopyFile refuses to overwrite an existing destination — it traps,
    // matching classic CB's "operation fails". Runs in the
    // temp dir so the relative files land there.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "copy.cb",
        "Dim a As File\na = OpenToWrite(\"a.dat\")\nWriteInt(a, 1)\nCloseFile(a)\n\
         Dim b As File\nb = OpenToWrite(\"b.dat\")\nWriteInt(b, 2)\nCloseFile(b)\n\
         Print \"before\"\nCopyFile(\"a.dat\", \"b.dat\")\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stdout(contains("before"))
        .stdout(contains("after").not())
        .stderr(contains("runtime error: CopyFile"))
        .stderr(contains("destination already exists"));
}

#[test]
fn runtime_abs_overload_resolves() {
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "abs.cb", "Dim x As Int\nx = Abs(-5)\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--dump-ir")
        .arg(&path)
        .assert()
        .success()
        .stdout(contains("call abs"));
}

// --- CLI parsing: help, version, and argument errors ---

#[test]
fn help_flag_prints_usage_and_exits_zero() {
    // `--help` is handled by clap: prints usage to stdout and exits 0, even
    // though `<FILE>` is otherwise required.
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(
            contains("Usage: cb")
                .and(contains("--backend"))
                .and(contains("--dump-ir")),
        );
}

#[test]
fn short_help_flag_exits_zero() {
    Command::cargo_bin("cb")
        .unwrap()
        .arg("-h")
        .assert()
        .success()
        .stdout(contains("Usage: cb"));
}

#[test]
fn version_flag_exits_zero() {
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("cb").and(contains(env!("CARGO_PKG_VERSION"))));
}

#[test]
fn unknown_flag_exits_two() {
    // An unrecognised flag is a clap usage error → exit 2.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--not-a-real-flag")
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("--not-a-real-flag").and(contains("unexpected argument")));
}

#[cfg(feature = "interp")]
#[test]
fn backend_equals_form_accepts_interp() {
    // The `--backend=interp` equals form is handled by clap identically to the
    // space-separated form.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--backend=interp")
        .arg(&path)
        .assert()
        .success();
}

#[test]
fn backend_empty_equals_rejected() {
    // `--backend=` supplies an empty backend name, which is not a known
    // backend → the driver's own "unknown backend" error, exit 2.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--backend=")
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("unknown backend"));
}

// --- Dump flags vs. the error gate ---

#[test]
fn dump_ir_on_erroring_input_exits_one_with_empty_stdout() {
    // IR lowering is gated behind `if !had_error`, so `--dump-ir` on an
    // erroring program prints nothing to stdout, reports the diagnostic on
    // stderr, and exits 1.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "bad.cb", "@\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--dump-ir")
        .arg(&path)
        .assert()
        .code(1)
        .stdout(is_empty())
        .stderr(contains("E0106"));
}

#[test]
fn dump_ast_on_erroring_input_still_emits() {
    // `--dump-ast` runs *before* the error gate, so a program with a semantic
    // error still gets its AST dumped to stdout while exiting 1 — the
    // deliberate asymmetry with `--dump-ir` above.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "sema.cb", "Dim y As Integer\ny = x + 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg("--dump-ast")
        .arg(&path)
        .assert()
        .code(1)
        .stdout(contains("Program ("))
        .stderr(contains("E0300"));
}

// --- Exit-code clamping ---

#[cfg(feature = "interp")]
#[test]
fn request_exit_above_255_clamps_to_255() {
    // `request_exit(256)` would wrap to 0 under the old `as u8` cast, hiding a
    // failure as success. The clamp policy saturates it to 255 instead.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "big_exit.cb",
        "Print \"before\"\nTestRequestExit(256)\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(255)
        .stdout(contains("before"))
        .stdout(contains("after").not());
}

#[cfg(feature = "interp")]
#[test]
fn request_exit_negative_clamps_to_zero() {
    // Negative codes are out of the OS `0..=255` range; the clamp policy maps
    // them to 0. Pinned so the choice is intentional rather than incidental.
    let dir = tempdir().unwrap();
    let path = write_cb(
        &dir,
        "neg_exit.cb",
        "Print \"before\"\nTestRequestExit(-1)\nPrint \"after\"\n",
    );
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(0)
        .stdout(contains("before"))
        .stdout(contains("after").not());
}

// --- Backend selection across feature builds ---

#[cfg(feature = "llvm")]
#[test]
fn backend_llvm_compiles_and_runs() {
    // The llvm backend now lowers the IR: it emits a native exe,
    // reports `cb: wrote …`, and exits 0. The produced exe runs and prints the
    // program's output. (The exhaustive interp==llvm parity check is the
    // `diff_llvm` suite; this is the CLI-contract smoke.)
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Print \"hi from llvm\"\n");
    let exe = dir
        .path()
        .join(format!("ok{}", std::env::consts::EXE_SUFFIX));
    Command::cargo_bin("cb")
        .unwrap()
        .args(["--backend", "llvm", "-o"])
        .arg(&exe)
        .arg(&path)
        .assert()
        .success()
        .stdout(contains("cb: wrote"));

    let out = std::process::Command::new(&exe)
        .output()
        .expect("run produced exe");
    assert!(out.status.success(), "produced exe should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hi from llvm"),
        "produced exe stdout missing program output: {stdout:?}"
    );
}

#[cfg(not(any(feature = "interp", feature = "llvm")))]
#[test]
fn no_backend_compiled_in_exits_two() {
    // A dump-only build (`--no-default-features`) has no backend to run, so a
    // plain compile-and-run invocation reports that and exits 2.
    let dir = tempdir().unwrap();
    let path = write_cb(&dir, "ok.cb", "Dim x As Int = 1\n");
    Command::cargo_bin("cb")
        .unwrap()
        .arg(&path)
        .assert()
        .code(2)
        .stderr(contains("no backend compiled in"));
}
