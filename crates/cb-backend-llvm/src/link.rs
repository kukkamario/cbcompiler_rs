//! Link step for the AOT pipeline.
//!
//! Takes the emitted object file and drives a compiler *driver* (`clang` on
//! Windows, `cc` on Unix) to produce a runnable executable: CRT startup glue +
//! the CoolBasic runtime closure that `cb-runtime-sys` built. We use a compiler
//! driver rather than a bare `ld`/`lld` so the toolchain (CRT, SDK lib paths) is
//! auto-discovered — no `vcvars`/`-libpath:` wrangling.
//!
//! The runtime location is not hardcoded: it comes from the `CB_RT_*` env vars
//! the build script re-exported from `cb-runtime-sys`'s `DEP_CB_RUNTIME_*`
//! metadata, so we link *whatever* runtime was built — the full Allegro closure
//! locally, or the SDK-free core on a machine/CI without Allegro.

use std::path::{Path, PathBuf};
use std::process::Command;

// Runtime link metadata, re-exported by build.rs from cb-runtime-sys.
const FLAVOR: &str = env!("CB_RT_FLAVOR"); // "full" | "sdkfree"
const LIB_DIR: &str = env!("CB_RT_LIB_DIR"); // cb-runtime-sys OUT_DIR
const RUNTIME_LIBS: &str = env!("CB_RT_RUNTIME_LIBS"); // comma-separated archive names
const CLOSURE_LIST: &str = env!("CB_RT_CLOSURE_LIST"); // closure file path ("" for sdkfree)
const LLVM_SYS_PREFIX: &str = env!("CB_LLVM_SYS_PREFIX"); // vcpkg LLVM 18 prefix ("" if unset)
const HOST_CC: &str = env!("CB_RT_CC"); // host cc path discovered at build time ("" if none)

/// Whether the CoolBasic runtime archives are force-included.
///
/// Production links lazily (`No`): an empty `main` references no runtime symbol,
/// so nothing is pulled in and the exe stays small — the closure is merely
/// *available* for later codegen. The gated test links `Yes` to force every
/// runtime object in, proving the closure actually *resolves* rather than that
/// the link args merely parse.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum WholeArchive {
    No,
    // Constructed only by the gated resolution test; dead in the lib build.
    #[cfg_attr(not(test), allow(dead_code))]
    Yes,
}

/// Link `obj` into the executable `exe`, pulling in the runtime closure.
pub fn link(obj: &Path, exe: &Path, whole: WholeArchive) -> Result<(), String> {
    if FLAVOR != "full" && FLAVOR != "sdkfree" {
        return Err(format!(
            "runtime link metadata missing (flavor {FLAVOR:?}); \
             was cb-runtime-sys built under the codegen feature?"
        ));
    }

    let driver = find_driver()?;
    let mut cmd = Command::new(&driver);
    cmd.arg(obj);
    cmd.arg("-o").arg(exe);

    // Windows: force the dynamic (/MD) CRT to match Rust and the plugin-DLL
    // model (CLAUDE.md). Three things are needed:
    //   * `/nodefaultlib:libcmt` neutralizes the static-CRT default the clang gnu
    //     driver hardcodes on the link line;
    //   * the dynamic CRT import libs are named *explicitly* — `-fms-runtime-lib=dll`
    //     only sets a compile-phase `--dependent-lib=msvcrt`, and no compile phase
    //     runs here (we feed a pre-built object). Without this the CRT — and
    //     crucially `mainCRTStartup`, which calls `main` — is absent (LNK2001).
    //     The runtime archives are /MD too and their objects carry the
    //     `/DEFAULTLIB:msvcrt` directive, but a lazily-linked empty `main` pulls
    //     none of them in, so we cannot rely on that.
    // Harmless no-op on non-Windows (the block is skipped).
    if cfg!(target_os = "windows") {
        cmd.arg("-fms-runtime-lib=dll");
        cmd.arg("-Xlinker").arg("/nodefaultlib:libcmt");
        for lib in ["msvcrt", "vcruntime", "ucrt"] {
            cmd.arg("-Xlinker").arg(format!("/defaultlib:{lib}"));
        }
    }

    // The CoolBasic runtime archives (cb_runtime[/_core] or cb_runtime_sdkfree).
    let archives = resolve_runtime_archives()?;
    add_runtime_archives(&mut cmd, &archives, whole);

    // The transitive Allegro/system closure (full flavor only): absolute lib
    // paths go to the linker verbatim; bare system names become `-l<name>`.
    if FLAVOR == "full" && !CLOSURE_LIST.is_empty() {
        let content = std::fs::read_to_string(CLOSURE_LIST)
            .map_err(|e| format!("read runtime closure list {CLOSURE_LIST}: {e}"))?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if Path::new(line).is_absolute() {
                cmd.arg(line);
            } else {
                let name = line.trim_start_matches("-l").trim_end_matches(".lib");
                if !name.is_empty() {
                    cmd.arg(format!("-l{name}"));
                }
            }
        }
    }

    // The runtime is C++; name the C++ standard library explicitly and last (the
    // C driver does not pull it in). MSVC links its CRT automatically — nothing
    // needed on Windows. Mirrors cb-runtime-sys/build.rs.
    if cfg!(target_os = "macos") {
        cmd.arg("-lc++");
    } else if cfg!(not(target_os = "windows")) {
        cmd.arg("-lstdc++");
    }

    // libm, after the archives that use it (GNU ld resolves left-to-right). The
    // runtime's math TUs reference `floor`/`pow`/…; the C driver does not pull in
    // libm, so a whole-archived `cb_math.o` fails to link without it ("DSO
    // missing from command line") — and real codegen calling runtime math will
    // need it even on the lazy path. cb-runtime-sys never names libm itself: the
    // interp binary gets it transitively from Rust's std, but this standalone
    // link drives its own libs. Skipped on Windows (the CRT supplies math); a
    // harmless no-op on macOS, where libm lives in libSystem.
    if cfg!(not(target_os = "windows")) {
        cmd.arg("-lm");
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run link driver {}: {e}", driver.display()))?;
    if !output.status.success() {
        return Err(format!(
            "link step failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    Ok(())
}

/// Add the CoolBasic runtime archives, force-including them under `Yes`.
fn add_runtime_archives(cmd: &mut Command, archives: &[PathBuf], whole: WholeArchive) {
    match whole {
        WholeArchive::No => {
            for a in archives {
                cmd.arg(a);
            }
        }
        WholeArchive::Yes if cfg!(target_os = "windows") => {
            // MSVC: `/WHOLEARCHIVE:<path>` per archive; also list it as input.
            for a in archives {
                cmd.arg("-Xlinker")
                    .arg(format!("/WHOLEARCHIVE:{}", a.display()));
                cmd.arg(a);
            }
        }
        WholeArchive::Yes => {
            // GNU ld / ld64: bracket the archives with --whole-archive.
            cmd.arg("-Wl,--whole-archive");
            for a in archives {
                cmd.arg(a);
            }
            cmd.arg("-Wl,--no-whole-archive");
        }
    }
}

/// Resolve each runtime archive name (from `CB_RT_RUNTIME_LIBS`) to a file on
/// disk under the runtime's lib dir. Handles the MSVC multi-config `Release/`
/// subdir and the `lib<name>.a` Unix naming.
fn resolve_runtime_archives() -> Result<Vec<PathBuf>, String> {
    let lib_dir = Path::new(LIB_DIR);
    RUNTIME_LIBS
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|name| resolve_lib(lib_dir, name))
        .collect()
}

fn resolve_lib(lib_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let candidates = [
        lib_dir.join(format!("{name}.lib")),
        lib_dir.join("Release").join(format!("{name}.lib")),
        lib_dir.join(format!("lib{name}.a")),
        lib_dir.join("Release").join(format!("lib{name}.a")),
    ];
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "runtime archive {name:?} not found under {} (looked for \
                 {name}.lib / lib{name}.a, incl. a Release/ subdir)",
                lib_dir.display()
            )
        })
}

/// Locate the link driver. On Windows, anchor on the vcpkg LLVM 18 prefix so we
/// use the pinned clang and not any stray clang on PATH;
/// vcpkg surfaces `clang.exe` under `bin/` only via the junction prefix, else at
/// `tools/llvm/`. On Unix, prefer the build-time discovered `cc`, then PATH.
fn find_driver() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        if !LLVM_SYS_PREFIX.is_empty() {
            for sub in ["bin/clang.exe", "tools/llvm/clang.exe"] {
                let p = Path::new(LLVM_SYS_PREFIX).join(sub);
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
        if probe("clang") {
            return Ok(PathBuf::from("clang"));
        }
        Err(
            "no clang link driver found: set LLVM_SYS_181_PREFIX to the vcpkg \
             LLVM 18 prefix, or put clang on PATH"
                .to_string(),
        )
    } else {
        if !HOST_CC.is_empty() && Path::new(HOST_CC).is_file() {
            return Ok(PathBuf::from(HOST_CC));
        }
        for cand in ["cc", "clang", "gcc"] {
            if probe(cand) {
                return Ok(PathBuf::from(cand));
            }
        }
        Err("no C link driver (cc/clang/gcc) found on PATH".to_string())
    }
}

/// True when `<cmd> --version` runs successfully — used to confirm a driver on
/// PATH before committing to it.
fn probe(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
