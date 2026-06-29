# FD-055: Self-Contained Windows AOT Toolchain — No Visual Studio / Windows SDK Install

**Status:** In Progress
**Priority:** Medium
**Effort:** Medium (1-4 hours)
**Impact:** A released `cb` can AOT-compile on a Windows machine with **no Visual Studio / Windows SDK installed**. Today the release bundles everything except the Microsoft CRT + Windows SDK *import libraries*, so the link step still requires a developer SDK. After this FD, the only Microsoft component a user needs is the Visual C++ Redistributable to *run* produced exes (in-scope-acceptable), fetched per-user.

## Problem

The Windows release archive already bundles `cb.exe`, the CoolBasic runtime + Allegro static libs (via `cb --bundle-runtime`), and the link driver (`clang.exe` + `lld-link.exe` in `bin/`). The single remaining external requirement is the **Microsoft CRT + Windows SDK import libraries** — `.lib` files only, since the AOT step only *links* a pre-built object (no compile phase, so no headers needed):

- the CRT import libs named explicitly in `link.rs` (`msvcrt.lib`, `vcruntime.lib`, `ucrt.lib`);
- the Windows SDK "um" import libs for the bare `-lNAME` system libs in the bundle's `closure.txt` (kernel32, user32, gdi32, shell32, ole32, comdlg32, winmm, psapi, shlwapi, opengl32, glu32, gdiplus, wsock32, avrt).

Today clang/lld auto-discover these from an installed VS / Windows SDK — exactly what `packaging/RELEASE-README-windows.md` tells the user to install. That install requirement is what makes the release *not* self-contained.

Out of scope: making the *produced* exe free of the VC++ Redistributable (that would require either a static CRT — rejected in `CLAUDE.md` for the `CallDLL` plugin-DLL model — or a full MinGW/UCRT toolchain migration). Requiring the redist to *run* output is acceptable.

## Solution

An **xwin-style per-user fetch**: a one-time download of the MSVC CRT + Windows SDK import libs into a per-user cache on the user's machine, then link against that cache. This is legally clean — each user fetches Microsoft content themselves under the MS license; we never redistribute it in our archive (only the MIT/Apache `xwin` tool ships).

**Integration — ship a prebuilt `xwin.exe` in `bin/` and shell out to it** (not the `xwin` crate as a library). Keeps `cb-backend-llvm`'s deps as-is (`inkwell`, `cb-runtime-sys`, `tempfile`) instead of dragging `reqwest`/`tokio`/TLS into the `llvm` feature; matches the established "tools live in `bin/`, shell out via `Command`" convention (`clang.exe`, `lld-link.exe`); depends only on xwin's stable CLI (`xwin splat`), not its unstable library API.

**Sysroot discovery + libpath injection (`link.rs`).** A new `win_sdk_dir()` mirrors `find_driver()`'s ordering: `CB_WIN_SDK` env → per-user cache `%LOCALAPPDATA%\cb\winsdk` → `<exe-dir>/sdk` → `None`. A companion `is_complete_sysroot()` validates sentinel libs exist before accepting a dir (mirrors `has_runtime_archives()`), so an interrupted splat is treated as absent. When a sysroot is found, the Windows link block adds `-Xlinker /libpath:<crt/lib/x64>`, `<sdk/lib/um/x64>`, `<sdk/lib/ucrt/x64>` (the `x64` leaf dirs come from `--preserve-ms-arch-notation`). **Discovery is the sole gate** — independent of the bundled-lld signal — so a dev box with a real SDK and no `CB_WIN_SDK`/cache produces a byte-for-byte-unchanged link command. lld-link searches `%LIB%` + `/libpath:` and does not probe the registry, so the explicit `/libpath:` is the natural match for the xwin splat layout (vs `/winsysroot`/`-vctoolsdir`, which expect a VS-style tree).

**Fetch action (`link.rs` + driver).** A new `cb --setup-toolchain` flag mirrors the `--bundle-runtime` utility-action pattern: it short-circuits, runs `setup_windows_toolchain()`, and exits; gated so a non-llvm build reports the gap. `setup_windows_toolchain()` resolves the splat target (`CB_WIN_SDK` else `%LOCALAPPDATA%\cb\winsdk`), locates `xwin.exe` via `find_xwin()` (mirroring `find_driver`'s bundled lookup: `CB_XWIN` env → `<exe-dir>/bin` → PATH), prints a one-line Microsoft-license notice (license auto-accepted), then shells out:

```
xwin --arch x86_64 --accept-license splat --output <dest> --preserve-ms-arch-notation --disable-symlinks
```

(omitting `--include-debug-libs`). Non-Windows is a friendly no-op + success so scripts stay portable.

**Release/CI.** The Windows release job builds the tool (`cargo install xwin --version <PIN> --locked`) and copies `xwin.exe` next to clang/lld-link in `bin/`. CI **does not** run `xwin splat` — only the MIT/Apache binary ships; MS libs are fetched on the user's machine. A new **Windows Server Core** job (no VS/SDK) runs `--setup-toolchain` + an AOT compile on every release to guard the no-SDK path continuously.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `crates/cb-backend-llvm/src/link.rs` | MODIFY | `win_sdk_dir()` + `is_complete_sysroot()`; `/libpath:` injection in the Windows block; `setup_windows_toolchain()` + `find_xwin()` + `ToolchainReport`. |
| `crates/cb-backend-llvm/src/lib.rs` | MODIFY | Re-export `ToolchainReport` + `setup_windows_toolchain`. |
| `crates/cb-driver/src/main.rs` | MODIFY | `--setup-toolchain` flag, `required_unless_present_any`, `setup_toolchain_cmd` gated like `bundle_runtime_cmd`. |
| `.github/workflows/release.yml` | MODIFY | Build/copy `xwin.exe` into `bin/` (no splat in CI); new Server Core no-SDK smoke job; update Windows-job comment. |
| `packaging/RELEASE-README-windows.md` | MODIFY | Replace the VS/SDK install instructions with the one-time `cb --setup-toolchain` step; add `xwin.exe` to the `bin/` table; keep the VC++ Redist line. |

## Primary risk + spike (do first)

When the bundled `clang.exe` (gnu-style driver, `*-pc-windows-msvc`) runs with `-fuse-ld=lld` on a machine with **no** MSVC toolchain: (a) does clang's toolchain auto-detection hard-fail before reaching lld-link, and (b) do the `-Xlinker /libpath:` entries suffice?

- **Spike 1 (any Windows box):** clear `LIB`/`LIBPATH` in the child env, splat an xwin cache to a temp dir, and run the bundled `lld-link.exe` **directly** on a trivial object with the three `/libpath:` entries + `/nodefaultlib:libcmt` + `/defaultlib:{msvcrt,vcruntime,ucrt}` + a few closure system libs (kernel32, gdiplus, glu32, avrt, wsock32). Confirms the libs + subdir names + `/MD` recipe resolve, isolated from clang behavior.
- **Spike 2 (clean env — the Server Core job):** the real `cb --setup-toolchain` → `cb --backend llvm` flow. Contingency if clang bails on toolchain detection: invoke the bundled `lld-link.exe` directly (`link.rs` already knows its path and builds nearly all the args).

**Spike 1 result (PASSED, 2026-06-29):** `lld-link` linked a trivial object into a runnable exe (exit code preserved) using *only* the xwin splat's three `/libpath:` entries with `LIB`/`LIBPATH` cleared — the `crt/lib/x64`, `sdk/lib/um/x64`, `sdk/lib/ucrt/x64` subdir names are correct and the `/MD` recipe (incl. `mainCRTStartup`) + all sampled closure system libs resolve with no system SDK. Surfaced one robustness issue: `xwin splat` creates a symlink for the SDK *headers* version dir that fails without symlink privilege (os error 1314) on a non-admin machine — irrelevant to us (link-only, no headers) and the import libs still splat. `setup_windows_toolchain` therefore judges success by `is_complete_sysroot` (the libs), not xwin's exit code. Possible future optimization: prune the headers download via xwin's `--map` (not worth the version-fragility now). Settle Spike 1 first (cheap); only if it passes does Spike 2 matter.

## Verification

- **No-SDK end-to-end (the real proof):** the Server Core CI job + a manual run on a clean VM — `--setup-toolchain`, AOT-compile `hello.cb`, run it, diff stdout/exit against the interp oracle; confirm imports are the dynamic CRT (`VCRUNTIME140.dll`, `api-ms-win-crt-*`), no `libcmt`.
- **Dev-machine regression (existing path untouched):** on a box with a real VS/SDK and no `CB_WIN_SDK`/cache/`<exe-dir>/sdk`, `cargo test -p cb-driver --features llvm` (the `diff_llvm` suite) — `win_sdk_dir()` returns `None`, so the link command is identical to today's.
- **Pre-flight:** Spike 1 doubles as a fast, machine-agnostic check that the import libs + `/MD` recipe resolve.

## Related

- `docs/features/archive/FD-048_BASIC_LLVM_CODEGEN_AND_TOOLING_DRIVER.md` — built the CRT-aware driver link path (`/MD` recipe) this FD makes self-contained.
- `docs/features/archive/FD-049_IR_TO_LLVM_LOWERING.md` — real IR→native lowering whose output this links.
- `CLAUDE.md` → "AOT codegen & linking (FD-048, FD-049)" — the link driver, runtime closure, and `/MD` recipe; documents why the static CRT (and thus a fully redist-free output) is off the table.
