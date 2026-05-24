use std::env;
use std::path::PathBuf;
use std::process::Command;

fn strip_unc(path: PathBuf) -> PathBuf {
    // Strip \\?\ UNC prefix that confuses MSVC on Windows
    PathBuf::from(
        path.to_str()
            .unwrap()
            .strip_prefix(r"\\?\")
            .unwrap_or(path.to_str().unwrap()),
    )
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let runtime_src = strip_unc(
        manifest_dir
            .join("../../runtime")
            .canonicalize()
            .expect("runtime/ directory not found"),
    );

    let vcpkg_toolchain = runtime_src.join("vcpkg/scripts/buildsystems/vcpkg.cmake");

    let build_dir = out_dir.join("cmake-build");
    std::fs::create_dir_all(&build_dir).unwrap();

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
        cmake_args.push(format!(
            "-DVCPKG_MANIFEST_DIR={}",
            runtime_src.display()
        ));
    }

    let status = Command::new("cmake")
        .args(&cmake_args)
        .current_dir(&build_dir)
        .status()
        .expect("failed to run cmake — is CMake installed and in PATH?");
    assert!(status.success(), "cmake configure failed");

    // Build
    let status = Command::new("cmake")
        .args(["--build", ".", "--config", "Release"])
        .current_dir(&build_dir)
        .status()
        .expect("cmake build invocation failed");
    assert!(status.success(), "cmake build failed");

    // Link
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // MSVC puts the lib in a Release/ subdirectory
    println!(
        "cargo:rustc-link-search=native={}",
        out_dir.join("Release").display()
    );
    println!("cargo:rustc-link-lib=static=cb_runtime");

    // Rebuild if runtime sources change
    println!(
        "cargo:rerun-if-changed={}",
        runtime_src.join("catalog.c").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        runtime_src.join("cb_runtime.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        runtime_src.join("CMakeLists.txt").display()
    );
}
