//! Build script for the LLVM/AOT backend (FD-048).
//!
//! Under the `codegen` feature it re-exports the runtime link metadata that
//! `cb-runtime-sys` publishes via its `links = "cb_runtime"` key
//! (`DEP_CB_RUNTIME_*`) as compile-time environment variables, so `link.rs` can
//! read the runtime closure location with `env!` at the AOT link site. Without
//! `codegen` the script is a no-op: neither `link.rs` nor the metadata exist.

use std::env;
use std::path::PathBuf;

/// Metadata keys published by `cb-runtime-sys` (FD-048). Re-exported verbatim as
/// `CB_RT_<KEY>` env vars. `CLOSURE_LIST` is empty for the SDK-free flavor.
const RUNTIME_KEYS: &[&str] = &["FLAVOR", "LIB_DIR", "RUNTIME_LIBS", "CLOSURE_LIST"];

fn main() {
    // The build script always runs; only do work for the codegen build, where
    // `cb-runtime-sys` is a dependency (so `DEP_CB_RUNTIME_*` exist) and
    // `link.rs`/`emit.rs` — the only readers of these env vars — are compiled.
    if env::var_os("CARGO_FEATURE_CODEGEN").is_none() {
        return;
    }

    // Re-export the runtime closure metadata published by cb-runtime-sys's build
    // script. Emit every key unconditionally — possibly empty — so `env!(...)` in
    // link.rs always resolves, and track each so this script reruns when the
    // runtime is rebuilt to a different location.
    for key in RUNTIME_KEYS {
        let dep_var = format!("DEP_CB_RUNTIME_{key}");
        println!("cargo:rerun-if-env-changed={dep_var}");
        let val = env::var(&dep_var).unwrap_or_default();
        println!("cargo:rustc-env=CB_RT_{key}={val}");
    }

    // The vcpkg LLVM 18 prefix; `link.rs` probes `<prefix>/bin/clang.exe` then
    // `<prefix>/tools/llvm/clang.exe` first, so it uses the pinned LLVM 18 clang
    // rather than any stray clang on PATH.
    println!("cargo:rerun-if-env-changed=LLVM_SYS_181_PREFIX");
    let prefix = env::var("LLVM_SYS_181_PREFIX").unwrap_or_default();
    println!("cargo:rustc-env=CB_LLVM_SYS_PREFIX={prefix}");

    // Host C/C++ compiler path as the non-Windows link-driver fallback (gcc/cc).
    // Discovered here because the `cc` crate is a build-dependency, unavailable
    // to runtime code. Best-effort: empty when discovery fails (link.rs then
    // falls back to `cc`/`clang` on PATH).
    let cc_path: PathBuf = cc::Build::new()
        .try_get_compiler()
        .map(|c| c.path().to_path_buf())
        .unwrap_or_default();
    println!("cargo:rustc-env=CB_RT_CC={}", cc_path.display());
}
