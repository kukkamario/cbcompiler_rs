//! End-to-end tests: spawn `cb.exe` on a `.cb` fixture and assert its
//! stdout matches the sibling `.out` file byte-for-byte (after line-ending
//! normalisation). Each fixture lives in `tests/fixtures/programs/` as a
//! pair `<name>.cb` + `<name>.out`. Adding a new test = write the pair +
//! one `#[test] fn name() { run("name") }`.
//!
//! Every fixture runs the program, so the whole suite needs a backend; it is
//! gated on `interp` (the reference backend) and is empty in backend-less
//! builds (`--no-default-features`).
#![cfg(feature = "interp")]

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

/// Like [`run`], but skips when the linked runtime has no graphics/input — the
/// SDK-free build (FD-033), whose language-core catalog omits the Allegro-
/// backed functions these fixtures call (sema would reject them as unknown).
fn run_graphics(name: &str) {
    if !cb_runtime_sys::HAS_GRAPHICS {
        eprintln!("skipping {name}: SDK-free runtime build has no graphics/input");
        return;
    }
    run(name);
}

/// Like [`run_graphics`], but runs the program in a throwaway working directory.
/// Fixtures that write files (e.g. `SaveImage` to a relative path) resolve them
/// against the cwd; isolating it keeps those temp files out of the crate root.
fn run_graphics_isolated(name: &str) {
    if !cb_runtime_sys::HAS_GRAPHICS {
        eprintln!("skipping {name}: SDK-free runtime build has no graphics/input");
        return;
    }
    let dir = fixtures_dir();
    let cb_path = dir.join(format!("{name}.cb"));
    let out_path = dir.join(format!("{name}.out"));
    let expected = std::fs::read_to_string(&out_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", out_path.display()));
    let work = tempfile::tempdir().expect("create temp working dir");

    let output = Command::cargo_bin("cb")
        .unwrap()
        .arg(&cb_path)
        .current_dir(work.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");

    let normalise = |s: &str| s.replace("\r\n", "\n");
    assert_eq!(
        normalise(&stdout),
        normalise(&expected),
        "stdout mismatch for {name}.cb",
    );
}

/// Like [`run_graphics_isolated`], but first stages the named committed assets
/// (from `tests/fixtures/assets/`) into the throwaway cwd, so the program can
/// load them by bare filename. Used for fixtures that consume real binary assets
/// (e.g. LoadMap of a CoolBasic `.til` + tileset).
fn run_graphics_with_assets(name: &str, assets: &[&str]) {
    if !cb_runtime_sys::HAS_GRAPHICS {
        eprintln!("skipping {name}: SDK-free runtime build has no graphics/input");
        return;
    }
    let dir = fixtures_dir();
    let cb_path = dir.join(format!("{name}.cb"));
    let out_path = dir.join(format!("{name}.out"));
    let expected = std::fs::read_to_string(&out_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", out_path.display()));
    let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets");
    let work = tempfile::tempdir().expect("create temp working dir");
    for asset in assets {
        std::fs::copy(assets_dir.join(asset), work.path().join(asset))
            .unwrap_or_else(|e| panic!("stage asset {asset}: {e}"));
    }

    let output = Command::cargo_bin("cb")
        .unwrap()
        .arg(&cb_path)
        .current_dir(work.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");

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
fn runtime_memblock_fd039() {
    // Memory blocks are Allegro-free (FD-039), so this runs in BOTH the full and
    // SDK-free builds — plain `run`, not `run_graphics`. Covers alloc/zero-fill,
    // unsigned Byte/Short + signed Int + 32-bit Float round-trips, little-endian
    // byte order, resize preserve/zero-fill, MemCopy, and Null default. The
    // out-of-bounds trap path is in cli.rs; the C++ edge cases in
    // runtime/tests/test_memblock.cpp.
    run("runtime_memblock_fd039");
}

#[test]
fn runtime_image() {
    run_graphics("runtime_image");
}

#[test]
fn runtime_gfx_fd017() {
    run_graphics("runtime_gfx_fd017");
}

#[test]
fn runtime_image_fd017() {
    run_graphics("runtime_image_fd017");
}

#[test]
fn collide_images() {
    run_graphics("collide_images");
}

#[test]
fn runtime_image_fd036() {
    // Writes a sprite sheet via SaveImage and reloads it with LoadAnimImage, so
    // it needs an isolated working directory.
    run_graphics_isolated("runtime_image_fd036");
}

#[test]
fn runtime_camera_fd036() {
    // Asserts deterministic camera state (CameraX/Y/Angle); the world<->screen
    // affine math is unit-tested headlessly in runtime/tests/test_camera.cpp.
    run_graphics("runtime_camera_fd036");
}

#[test]
fn runtime_map_fd036() {
    // MakeMap dims + GetMap2/EditMap round-trips, and LoadMap of the real
    // CoolBasic asset testmap.til + tileset.bmp (byte-verified format). The
    // world<->tile math is unit-tested headlessly in runtime/tests/test_map.cpp.
    run_graphics_with_assets("runtime_map_fd036", &["testmap.til", "tileset.bmp"]);
}

#[test]
fn runtime_object_fd036() {
    // MakeObject/Position/Rotate/Turn/slot/life round-trips, GetAngle2/Distance2,
    // CloneObject pos+angle reset, a built sprite-sheet object (Play/Stop/Loop/
    // Size/Frame), PaintObject, and InitObjectList/NextObject enumeration. Writes
    // the sheet via SaveImage, so it needs an isolated working directory. The pure
    // object math is unit-tested headlessly in runtime/tests/test_object.cpp.
    run_graphics_isolated("runtime_object_fd036");
}

#[test]
fn runtime_collision_fd036() {
    // ObjectsOverlap (box/circle/pixel-stub/invalid-type), the 1-based collision
    // query surface, and SetupCollision registration (object-object + the type-4
    // Map overload). Deterministic state, no display. The persistent per-tick
    // collision path is covered by runtime_gameloop_fd036 (Phase 5c); the
    // resolution geometry by runtime/tests/test_collision.cpp.
    run_graphics("runtime_collision_fd036");
}

#[test]
fn runtime_pick_fd036() {
    // ObjectPick raycast + PickedObject/X/Y/Angle, ObjectSight (map-wall DDA), and
    // the object-aware camera funcs (PointCamera/CameraFollow/CloneCamera*/
    // CameraPick/ScreenPositionObject). Deterministic state, no display: the
    // screen<->world transform uses the 400x300 design size via cb_camera_math.
    // CameraFollow's per-frame motion is covered by runtime_gameloop_fd036 (5c).
    run_graphics("runtime_pick_fd036");
}

#[test]
fn runtime_emitter_fd038() {
    // Particle emitters: MakeEmitter returns the Object handle (no distinct type),
    // so object commands drive it and it enumerates; the Particle* commands wire
    // up (3-/4-arg movement, emission, animation); and the emitter is excluded
    // from both ObjectsOverlap/SetupCollision and ObjectPickable/ObjectPick (real
    // CB — see FD-038). Driven through UpdateGame. The particle simulation math is
    // unit-tested headlessly in runtime/tests/test_particle.cpp.
    run_graphics("runtime_emitter_fd038");
}

#[test]
fn runtime_gameloop_fd036() {
    // UpdateGame drives the per-tick advancement: ObjectLife decrement + auto-
    // delete, animation frame advance, and the persistent SetupCollision checks
    // (report records-but-doesn't-move; circle slide applies the resolved
    // position). Builds a sprite sheet via SaveImage, so it needs an isolated cwd.
    // The UpdateGame/DrawGame/DrawScreen dedup flags need a real display and are a
    // deferred visual smoke; the per-tick math is unit-tested in test_object.cpp /
    // test_collision.cpp.
    run_graphics_isolated("runtime_gameloop_fd036");
}

#[test]
fn runtime_text_fd018() {
    run_graphics("runtime_text_fd018");
}

#[test]
fn runtime_input() {
    run_graphics("runtime_input");
}

#[test]
fn runtime_constants_fd029() {
    // The cbKey* constants resolve SDK-free, but this fixture also calls
    // KeyDown (an input function), so it needs the full runtime. Constant
    // decoding itself is covered by the cb-runtime-sys unit tests in both modes.
    run_graphics("runtime_constants_fd029");
}
