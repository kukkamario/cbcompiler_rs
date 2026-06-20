use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn strip_unc(path: PathBuf) -> PathBuf {
    // Strip the \\?\ UNC prefix that confuses MSVC on Windows. Only relevant
    // on Windows; on a non-UTF-8 path we can't string-manipulate it, so leave
    // it untouched rather than panicking (FD-024).
    if !cfg!(windows) {
        return path;
    }
    match path.to_str() {
        Some(s) => PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(s)),
        None => path,
    }
}

/// Translation units with zero Allegro dependency: the FD-016 "core" (string
/// primitives in `cb_string.cpp` + the `cb_runtime_init` host handshake in
/// `cb_host.cpp`), the Allegro-free functionality (`cb_math.cpp`, the string
/// library `cb_strfuncs.cpp`, `cb_system.cpp`), and the catalog assembly
/// compiled with `-DCB_NO_ALLEGRO` so its graphics/input/text `CB_FN` rows —
/// the only things that would otherwise force a link against Allegro — are
/// guarded out. This is enough to build a real catalog of every language-core
/// runtime function (FD-033).
const SDK_FREE_TUS: &[&str] = &[
    "cb_string.cpp",
    "cb_host.cpp",
    "cb_math.cpp",
    "cb_strfuncs.cpp",
    "cb_system.cpp",
    "catalog.cpp",
];

/// Source files that should trigger a rebuild when changed, regardless of
/// which build path runs.
const RERUN_SOURCES: &[&str] = &[
    "catalog.cpp",
    "cb_host.cpp",
    "cb_math.cpp",
    "cb_string.cpp",
    "cb_strfuncs.cpp",
    "cb_system.cpp",
    "cb_gfx.cpp",
    "cb_geom.h",
    "cb_camera.cpp",
    "cb_camera.h",
    "cb_camera_math.h",
    "cb_map.cpp",
    "cb_map.h",
    "cb_map_data.h",
    "cb_object.cpp",
    "cb_object.h",
    "cb_object_data.h",
    "cb_font.cpp",
    "cb_font.h",
    "cb_input.cpp",
    "cb_input.h",
    "cb_keys.def",
    "cb_utf8.h",
    "cb_runtime.h",
    "cb_runtime_core.h",
    "cb_runtime_func.h",
    "CMakeLists.txt",
];

fn main() {
    // We may emit `--cfg cb_no_allegro`; declare it so the unexpected-cfgs lint
    // stays quiet on every downstream crate.
    println!("cargo:rustc-check-cfg=cfg(cb_no_allegro)");
    println!("cargo:rerun-if-env-changed=CB_RUNTIME_FORCE_SDK_FREE");
    println!("cargo:rerun-if-env-changed=CB_RUNTIME_REQUIRE_ALLEGRO");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let runtime_src = strip_unc(
        manifest_dir
            .join("../../runtime")
            .canonicalize()
            .expect("runtime/ directory not found"),
    );

    for f in RERUN_SOURCES {
        println!("cargo:rerun-if-changed={}", runtime_src.join(f).display());
    }

    let force_sdk_free = env::var_os("CB_RUNTIME_FORCE_SDK_FREE").is_some();
    let require_allegro = env::var_os("CB_RUNTIME_REQUIRE_ALLEGRO").is_some();
    assert!(
        !(force_sdk_free && require_allegro),
        "CB_RUNTIME_FORCE_SDK_FREE and CB_RUNTIME_REQUIRE_ALLEGRO are both set — pick one"
    );

    // Path selection (FD-033):
    //   • forced SDK-free            → cc build, no probing
    //   • required Allegro           → CMake build, fatal on failure
    //   • auto (default)             → CMake when the toolchain is present and
    //                                  configures cleanly; otherwise fall back
    //                                  to the SDK-free cc build so a plain
    //                                  `cargo test` works on any machine.
    if force_sdk_free {
        build_sdk_free(&runtime_src);
        return;
    }

    if require_allegro {
        build_full(&out_dir, &runtime_src)
            .expect("CB_RUNTIME_REQUIRE_ALLEGRO is set but the full Allegro build failed");
        return;
    }

    if cmake_available() {
        match build_full(&out_dir, &runtime_src) {
            Ok(()) => return,
            Err(e) => {
                println!(
                    "cargo:warning=full Allegro runtime build failed ({e}); falling back to the \
                     SDK-free core runtime (no graphics/input). Set CB_RUNTIME_REQUIRE_ALLEGRO=1 \
                     to make this fatal."
                );
            }
        }
    } else {
        println!(
            "cargo:warning=cmake not found; building the SDK-free core runtime (no \
             graphics/input). Install CMake + the Allegro SDK for the full runtime, or set \
             CB_RUNTIME_REQUIRE_ALLEGRO=1 to require it."
        );
    }

    build_sdk_free(&runtime_src);
}

/// True when a `cmake` executable is callable. Cheap probe so the auto path can
/// skip the full build (and its slow configure) on a Rust-only machine.
fn cmake_available() -> bool {
    Command::new("cmake")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Compile only the Allegro-free TUs via the `cc` crate, producing a static lib
/// with a real `cb_runtime_get_catalog` (language-core functions only) and the
/// string primitives. `cc` emits the `rustc-link-lib` for the archive and links
/// the C++ standard library itself. Signals `cb_no_allegro` to dependents so
/// graphics-dependent tests can skip cleanly.
fn build_sdk_free(runtime_src: &Path) {
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++20")
        .define("CB_NO_ALLEGRO", None)
        .include(runtime_src);
    for tu in SDK_FREE_TUS {
        build.file(runtime_src.join(tu));
    }
    build.compile("cb_runtime_sdkfree");

    println!("cargo:rustc-cfg=cb_no_allegro");
}

/// Build the full runtime (core + functionality + Allegro) through CMake and
/// emit the static-link directives, including the transitive Allegro closure
/// parsed from the list `runtime/CMakeLists.txt` generates. Returns `Err` on
/// any configure/build/parse failure so the caller can fall back; all
/// `cargo:` directives are emitted only after every fallible step succeeds, so
/// a failure leaves no half-applied link state behind.
fn build_full(out_dir: &Path, runtime_src: &Path) -> Result<(), String> {
    let vcpkg_toolchain = runtime_src.join("vcpkg/scripts/buildsystems/vcpkg.cmake");

    let build_dir = out_dir.join("cmake-build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("create build dir: {e}"))?;

    // Configure
    let mut cmake_args = vec![
        runtime_src.to_str().unwrap().to_string(),
        format!("-DCMAKE_ARCHIVE_OUTPUT_DIRECTORY={}", out_dir.display()),
    ];
    if vcpkg_toolchain.exists() {
        cmake_args.push(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}",
            vcpkg_toolchain.display()
        ));
        cmake_args.push(format!("-DVCPKG_MANIFEST_DIR={}", runtime_src.display()));
        // x64-windows-static-md: static Allegro + transitive deps, dynamic CRT
        // (matches Rust's default /MD CRT linkage on MSVC). Produces a
        // single-file cb.exe with no runtime DLL dependencies beyond the
        // standard Windows redistributables.
        if cfg!(target_os = "windows") {
            cmake_args.push("-DVCPKG_TARGET_TRIPLET=x64-windows-static-md".to_string());
        }
    }

    let status = Command::new("cmake")
        .args(&cmake_args)
        .current_dir(&build_dir)
        .status()
        .map_err(|e| format!("failed to run cmake: {e}"))?;
    if !status.success() {
        return Err("cmake configure failed".to_string());
    }

    // Build
    let status = Command::new("cmake")
        .args(["--build", ".", "--config", "Release"])
        .current_dir(&build_dir)
        .status()
        .map_err(|e| format!("cmake build invocation failed: {e}"))?;
    if !status.success() {
        return Err("cmake build failed".to_string());
    }

    // Read the transitive link list emitted by runtime/CMakeLists.txt.
    // Multi-config generators (MSVC) produce per-config files; we only ever
    // build Release here. Single-config generators (Ninja) write the
    // un-suffixed name when CMAKE_BUILD_TYPE is empty.
    let link_list_candidates = [
        build_dir.join("cb_runtime_link_libs_Release.txt"),
        build_dir.join("cb_runtime_link_libs_.txt"),
        build_dir.join("cb_runtime_link_libs.txt"),
    ];
    let link_list = link_list_candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            format!(
                "cb_runtime_link_libs_*.txt not generated by CMake under {}",
                build_dir.display()
            )
        })?;
    let content =
        std::fs::read_to_string(link_list).map_err(|e| format!("read generated link list: {e}"))?;

    // All fallible work done — now emit the cargo directives.
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // MSVC puts the lib in a Release/ subdirectory
    println!(
        "cargo:rustc-link-search=native={}",
        out_dir.join("Release").display()
    );
    // Functionality lib before core lib: cb_runtime references core symbols,
    // and GNU ld resolves left-to-right (a dependent must precede its
    // dependency). MSVC is order-insensitive for static libs. The Allegro
    // transitive closure (parsed below) needs only the functionality lib.
    println!("cargo:rustc-link-lib=static=cb_runtime");
    println!("cargo:rustc-link-lib=static=cb_runtime_core");

    let mut seen_dirs = BTreeSet::<String>::new();
    let mut seen_libs = BTreeSet::<String>::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = Path::new(line);
        if path.is_absolute() {
            // Absolute path to a .lib produced by CMake's $<TARGET_LINKER_FILE:tgt>.
            if let Some(parent) = path.parent() {
                let dir = parent.display().to_string();
                if seen_dirs.insert(dir.clone()) {
                    println!("cargo:rustc-link-search=native={dir}");
                }
            }
            if let Some(stem) = path.file_stem() {
                let stem = stem.to_string_lossy();
                // Determine link kind + the bare name rustc expects.
                // MSVC: `name.lib` → static, the stem IS the name.
                // Unix: `lib<name>.a` → static, `lib<name>.so` → dylib; in both
                // cases the leading `lib` prefix must be stripped (the linker
                // re-adds it when resolving `-l<name>`).
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase());
                let (kind, name) = match ext.as_deref() {
                    Some("a") => ("static", stem.strip_prefix("lib").unwrap_or(&stem)),
                    Some("so") | Some("dylib") => {
                        ("dylib", stem.strip_prefix("lib").unwrap_or(&stem))
                    }
                    // MSVC `.lib` (and anything else): use the stem verbatim.
                    _ => ("static", stem.as_ref()),
                };
                if seen_libs.insert(name.to_string()) {
                    println!("cargo:rustc-link-lib={kind}={name}");
                }
            }
        } else {
            // Bare library name from INTERFACE_LINK_LIBRARIES (typically a
            // Windows system lib like "opengl32" or "dwmapi"). Strip Unix-style
            // -l prefix and any .lib suffix CMake may have included.
            let name = line.trim_start_matches("-l").trim_end_matches(".lib");
            if !name.is_empty() && seen_libs.insert(name.to_string()) {
                println!("cargo:rustc-link-lib={name}");
            }
        }
    }

    // The runtime is C++, and the Allegro/openal static archives reference
    // C++ standard-library symbols (std::runtime_error typeinfo, throw
    // helpers, …). rustc links the final binary through the C compiler
    // driver, which does NOT pull in the C++ runtime — so name it explicitly,
    // last, after the C++ archives that need it. MSVC links its CRT
    // automatically and needs nothing here.
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if cfg!(not(target_os = "windows")) {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }

    Ok(())
}
