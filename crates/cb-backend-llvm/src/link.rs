//! Link step for the AOT pipeline.
//!
//! Takes the emitted object file and drives a compiler *driver* (`clang` on
//! Windows, `cc` on Unix) to produce a runnable executable: CRT startup glue +
//! the CoolBasic runtime closure that `cb-runtime-sys` built. We use a compiler
//! driver rather than a bare `ld`/`lld` so the toolchain (CRT, SDK lib paths) is
//! auto-discovered — no `vcvars`/`-libpath:` wrangling.
//!
//! The runtime location is resolved at *use* time, not hardcoded, so a relocated
//! `cb` (a published release moved off the build machine) finds its runtime next
//! to the executable. Resolution order — see [`resolve_runtime_dir`]:
//!   1. `CB_RUNTIME_DIR` env var (explicit override);
//!   2. `<exe-dir>/lib` (the layout [`stage_runtime_bundle`] / the release
//!      archive produces), when it actually holds the runtime archives;
//!   3. the build-time `CB_RT_*` metadata `cb-runtime-sys` published — the
//!      unchanged dev / `cargo test` path.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

// Runtime link metadata, re-exported by build.rs from cb-runtime-sys. These are
// the *build-time* locations; at use time they serve only as the dev fallback
// (see [`resolve_runtime_dir`]).
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

/// The resolved location of the runtime archives + (full flavor) the Allegro
/// closure for *this* invocation of `cb`.
struct RuntimeDir {
    /// Directory holding the `cb_runtime[_core]` (or `cb_runtime_sdkfree`)
    /// archives.
    lib_dir: PathBuf,
    /// `true` for a relocated/bundled dir: the closure is read from
    /// `<lib_dir>/closure.txt` with lib entries relative to `lib_dir`. `false`
    /// for the build-time fallback: the closure is read from the absolute
    /// `CB_RT_CLOSURE_LIST` file.
    bundled: bool,
}

/// Link `obj` into the executable `exe`, pulling in the runtime closure.
pub fn link(obj: &Path, exe: &Path, whole: WholeArchive) -> Result<(), String> {
    if FLAVOR != "full" && FLAVOR != "sdkfree" {
        return Err(format!(
            "runtime link metadata missing (flavor {FLAVOR:?}); \
             was cb-runtime-sys built under the codegen feature?"
        ));
    }

    let rt = resolve_runtime_dir();
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
        // When using the bundled clang from a release (next to the exe), prefer
        // the bundled lld-link so no Visual Studio link.exe is needed — only the
        // Windows SDK/CRT import libs, which clang/lld discover themselves.
        for a in bundled_lld_args(&driver) {
            cmd.arg(a);
        }
    }

    // The CoolBasic runtime archives (cb_runtime[/_core] or cb_runtime_sdkfree).
    let archives = resolve_runtime_archives(&rt.lib_dir)?;
    add_runtime_archives(&mut cmd, &archives, whole);

    // The transitive Allegro/system closure (full flavor only).
    for arg in closure_args(&rt)? {
        cmd.arg(arg);
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

/// Resolve the runtime directory for this invocation (see the module docs).
fn resolve_runtime_dir() -> RuntimeDir {
    if let Some(dir) = std::env::var_os("CB_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return RuntimeDir {
            lib_dir: PathBuf::from(dir),
            bundled: true,
        };
    }
    if let Some(exe_d) = exe_dir() {
        let lib = exe_d.join("lib");
        if has_runtime_archives(&lib) {
            return RuntimeDir {
                lib_dir: lib,
                bundled: true,
            };
        }
    }
    RuntimeDir {
        lib_dir: PathBuf::from(LIB_DIR),
        bundled: false,
    }
}

/// The directory containing the running `cb` executable.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// True when every runtime archive resolves under `dir` — the signal that an
/// `<exe-dir>/lib` is a real bundle rather than an empty/absent directory.
fn has_runtime_archives(dir: &Path) -> bool {
    RUNTIME_LIBS
        .split(',')
        .filter(|s| !s.is_empty())
        .all(|name| resolve_lib(dir, name).is_ok())
}

/// The transitive Allegro/system closure as ready-to-pass linker args (full
/// flavor only; empty for sdkfree). Bundled dirs read `<lib_dir>/closure.txt`
/// (lib filenames relative to the dir, `-lNAME` system libs verbatim); the
/// build-time fallback reads the absolute `CB_RT_CLOSURE_LIST` (absolute lib
/// paths verbatim, bare names → `-lNAME`).
fn closure_args(rt: &RuntimeDir) -> Result<Vec<OsString>, String> {
    if FLAVOR != "full" {
        return Ok(Vec::new());
    }

    let (list_path, base): (PathBuf, Option<&Path>) = if rt.bundled {
        let p = rt.lib_dir.join("closure.txt");
        if !p.is_file() {
            return Err(format!(
                "bundled runtime at {} is missing closure.txt (malformed bundle)",
                rt.lib_dir.display()
            ));
        }
        (p, Some(rt.lib_dir.as_path()))
    } else {
        if CLOSURE_LIST.is_empty() {
            return Ok(Vec::new());
        }
        (PathBuf::from(CLOSURE_LIST), None)
    };

    let content = std::fs::read_to_string(&list_path)
        .map_err(|e| format!("read runtime closure list {}: {e}", list_path.display()))?;
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match base {
            // Bundled: `-lNAME` is a system lib; anything else is a lib filename
            // relative to the bundle dir.
            Some(base) => {
                if let Some(name) = line.strip_prefix("-l") {
                    out.push(OsString::from(format!("-l{name}")));
                } else {
                    out.push(base.join(line).into_os_string());
                }
            }
            // Build-time list: absolute path → verbatim; bare name → -lNAME.
            None => {
                if Path::new(line).is_absolute() {
                    out.push(OsString::from(line));
                } else {
                    let name = line.trim_start_matches("-l").trim_end_matches(".lib");
                    if !name.is_empty() {
                        out.push(OsString::from(format!("-l{name}")));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Resolve each runtime archive name (from `CB_RT_RUNTIME_LIBS`) to a file on
/// disk under `lib_dir`.
fn resolve_runtime_archives(lib_dir: &Path) -> Result<Vec<PathBuf>, String> {
    RUNTIME_LIBS
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|name| resolve_lib(lib_dir, name))
        .collect()
}

/// Resolve one archive `name` to a file under `lib_dir`, handling the MSVC
/// multi-config `Release/` subdir and the `lib<name>.a` Unix naming.
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

/// Locate the link driver. Order on every platform: the `CB_LINK_DRIVER`
/// override, then a bundled driver next to the exe (release), then the
/// platform-specific dev/system search.
///
/// Windows anchors on the bundled/vcpkg LLVM 18 `clang.exe`; Unix prefers the
/// build-time discovered `cc`, then PATH.
fn find_driver() -> Result<PathBuf, String> {
    if let Some(d) = env_driver() {
        return Ok(d);
    }

    if cfg!(target_os = "windows") {
        // Bundled clang next to the exe (release layout).
        if let Some(exe_d) = exe_dir() {
            for sub in ["bin/clang.exe", "clang.exe"] {
                let p = exe_d.join(sub);
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
        // Dev: the vcpkg LLVM 18 prefix (`bin/` via the junction, else `tools/llvm/`).
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
            "no clang link driver found: set CB_LINK_DRIVER, ship a bundled \
             clang next to cb, set LLVM_SYS_181_PREFIX, or put clang on PATH"
                .to_string(),
        )
    } else {
        // Bundled clang next to the exe (release layout), if any.
        if let Some(exe_d) = exe_dir() {
            for sub in ["bin/clang", "clang"] {
                let p = exe_d.join(sub);
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
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

/// The `CB_LINK_DRIVER` override, when set to a non-empty value.
fn env_driver() -> Option<PathBuf> {
    let v = std::env::var_os("CB_LINK_DRIVER")?;
    if v.is_empty() {
        None
    } else {
        Some(PathBuf::from(v))
    }
}

/// Linker args that steer clang at a bundled `lld-link.exe`. Returned only when
/// `driver` is the bundled clang under the exe dir *and* an `lld-link.exe` sits
/// beside it — i.e. a Windows release. Dev builds (clang from the vcpkg prefix
/// or PATH) get an empty list and keep using the MSVC `link.exe`, unchanged.
fn bundled_lld_args(driver: &Path) -> Vec<String> {
    let (Some(dir), Some(exe_d)) = (driver.parent(), exe_dir()) else {
        return Vec::new();
    };
    if !dir.starts_with(&exe_d) {
        return Vec::new();
    }
    if dir.join("lld-link.exe").is_file() {
        vec!["-fuse-ld=lld".to_string(), format!("-B{}", dir.display())]
    } else {
        Vec::new()
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

/// Summary of a staged runtime bundle, for the driver to report.
#[derive(Debug)]
pub struct BundleReport {
    /// Directory the bundle was written to.
    pub dest: PathBuf,
    /// The runtime flavor staged: `"full"` or `"sdkfree"`.
    pub flavor: &'static str,
    /// Number of CoolBasic runtime archives copied.
    pub archives: usize,
    /// Number of closure lib *files* copied into the bundle (Allegro + its
    /// transitive static deps; 0 for sdkfree and for the dynamic/system flavor).
    pub closure_libs: usize,
    /// Number of bare system libs recorded as `-lNAME` (resolved from the user's
    /// system at AOT link time, not copied).
    pub system_libs: usize,
}

/// Stage a relocatable copy of the runtime under `dest` so a `cb` placed beside
/// it (with `dest` as `<exe-dir>/lib`) links AOT output without the build
/// machine's paths.
///
/// Copies the runtime archives and, for the full flavor, every absolute closure
/// lib file, then writes a `closure.txt` whose lib entries are *relative* to
/// `dest` (bare system libs are recorded as `-lNAME` and resolved from the
/// user's system at link time). Always reads the *build-time* metadata — the
/// runtime this `cb` was built against — so it is meant to run from the same
/// build that produced the binary being packaged.
pub fn stage_runtime_bundle(dest: &Path) -> Result<BundleReport, String> {
    if FLAVOR != "full" && FLAVOR != "sdkfree" {
        return Err(format!(
            "runtime link metadata missing (flavor {FLAVOR:?}); \
             was cb-runtime-sys built under the codegen feature?"
        ));
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;

    // Runtime archives, from the build-time lib dir.
    let archives = resolve_runtime_archives(Path::new(LIB_DIR))?;
    for a in &archives {
        let name = a
            .file_name()
            .ok_or_else(|| format!("archive path has no file name: {}", a.display()))?;
        let to = dest.join(name);
        std::fs::copy(a, &to)
            .map_err(|e| format!("copy {} -> {}: {e}", a.display(), to.display()))?;
    }

    // Closure (full flavor): copy each absolute lib file, record it relatively;
    // pass bare system names through as `-lNAME`.
    let mut closure_lines: Vec<String> = Vec::new();
    let mut closure_libs = 0usize;
    let mut system_libs = 0usize;
    if FLAVOR == "full" && !CLOSURE_LIST.is_empty() {
        let content = std::fs::read_to_string(CLOSURE_LIST)
            .map_err(|e| format!("read closure list {CLOSURE_LIST}: {e}"))?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if Path::new(line).is_absolute() {
                let src = Path::new(line);
                let name = src
                    .file_name()
                    .ok_or_else(|| format!("closure lib path has no file name: {line}"))?;
                let to = dest.join(name);
                std::fs::copy(src, &to)
                    .map_err(|e| format!("copy {} -> {}: {e}", src.display(), to.display()))?;
                closure_lines.push(name.to_string_lossy().into_owned());
                closure_libs += 1;
            } else {
                let nm = line.trim_start_matches("-l").trim_end_matches(".lib");
                if !nm.is_empty() {
                    closure_lines.push(format!("-l{nm}"));
                    system_libs += 1;
                }
            }
        }
    }
    if FLAVOR == "full" {
        let closure_path = dest.join("closure.txt");
        let mut body = closure_lines.join("\n");
        body.push('\n');
        std::fs::write(&closure_path, body)
            .map_err(|e| format!("write {}: {e}", closure_path.display()))?;
    }

    Ok(BundleReport {
        dest: dest.to_path_buf(),
        flavor: FLAVOR,
        archives: archives.len(),
        closure_libs,
        system_libs,
    })
}
