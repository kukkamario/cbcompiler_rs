//! LLVM backend for CoolBasic IR.
//!
//! Under the `codegen` feature this drives the **output half** of the AOT
//! pipeline (FD-048): build an in-memory `inkwell` module, emit a native object,
//! and link it — against the full CoolBasic runtime closure — into a runnable
//! executable. It does *not* yet read the IR: it emits a fixed empty program
//! (`i32 @main() { ret i32 0 }`) so the object-emit + linker plumbing is stood up
//! independently of IR→LLVM instruction selection (a later FD). Without
//! `codegen`, [`Backend::execute`] still reports the gap (driver exit 3) so the
//! driver dispatches through the shared trait (FD-044) ahead of codegen.

use std::path::PathBuf;

use cb_backend_api::{Backend, BackendError, BackendOutcome};
use cb_diagnostics::Interner;
use cb_ir::Program;

#[cfg(feature = "codegen")]
mod emit;
#[cfg(feature = "codegen")]
mod link;

/// The LLVM / AOT backend. The `source`/`output` paths are injected at
/// construction (FD-048 decision 3) because the IR carries no source path and
/// the [`Backend::execute`] signature (FD-044) passes only `&Program` +
/// `&Interner` — so codegen could not otherwise name the artifact. They are
/// unused in the no-`codegen` stub build.
pub struct LlvmBackend {
    #[cfg_attr(not(feature = "codegen"), allow(dead_code))]
    source: PathBuf,
    #[cfg_attr(not(feature = "codegen"), allow(dead_code))]
    output: Option<PathBuf>,
}

impl LlvmBackend {
    /// Construct the backend for `source`, writing the artifact to `output` (or,
    /// when `None`, next to the source as `<stem>` + the platform exe suffix).
    pub fn new(source: PathBuf, output: Option<PathBuf>) -> Self {
        Self { source, output }
    }

    /// The executable path to write: the explicit `-o` output, or `<source
    /// stem>` + `EXE_SUFFIX` next to the source file.
    #[cfg(feature = "codegen")]
    fn artifact_path(&self) -> PathBuf {
        if let Some(out) = &self.output {
            return out.clone();
        }
        let stem = self.source.file_stem().unwrap_or(self.source.as_os_str());
        let mut name = stem.to_os_string();
        name.push(std::env::consts::EXE_SUFFIX);
        self.source.with_file_name(name)
    }
}

impl Backend for LlvmBackend {
    fn name(&self) -> &'static str {
        "llvm"
    }

    /// Emit the empty-program object and link it into a runtime-linked exe,
    /// returning [`BackendOutcome::Produced`]. The `program` is accepted but not
    /// read (FD-048): the body is independent of the IR. Any emit/link failure
    /// becomes `BackendError::failed` → driver exit 1.
    #[cfg(feature = "codegen")]
    fn execute(
        &self,
        _program: &Program,
        _interner: &Interner,
    ) -> Result<BackendOutcome, BackendError> {
        let artifact = self.artifact_path();

        // Intermediate object in an auto-cleaned temp dir.
        let tmp = tempfile::tempdir()
            .map_err(|e| BackendError::failed(format!("create temp dir: {e}")))?;
        let obj_name = if cfg!(target_os = "windows") {
            "cb_main.obj"
        } else {
            "cb_main.o"
        };
        let obj = tmp.path().join(obj_name);

        emit::emit_empty_main(&obj).map_err(BackendError::failed)?;
        link::link(&obj, &artifact, link::WholeArchive::No).map_err(BackendError::failed)?;

        Ok(BackendOutcome::Produced { artifact })
    }

    /// Stub path: codegen is compiled out, so report the gap explicitly (driver
    /// exit 3) rather than silently doing nothing (FD-025).
    #[cfg(not(feature = "codegen"))]
    fn execute(
        &self,
        _program: &Program,
        _interner: &Interner,
    ) -> Result<BackendOutcome, BackendError> {
        Err(BackendError::unimplemented(
            "the llvm backend needs the `codegen` feature (build the driver with \
             --features llvm); run with --backend interp to execute programs",
        ))
    }
}

// AOT smoke tests (FD-047 linkage check + FD-048 emit→link→run). Gated on
// `codegen` so the default LLVM-free build has no dead code and needs no LLVM
// install. The link test exercises whatever runtime cb-runtime-sys built — the
// full Allegro closure locally, the SDK-free core on CI.
#[cfg(all(test, feature = "codegen"))]
mod codegen_smoke {
    use crate::{emit, link};

    #[test]
    fn context_and_module_boot() {
        let ctx = inkwell::context::Context::create();
        let module = ctx.create_module("cb_smoke");
        assert_eq!(module.get_name().to_str(), Ok("cb_smoke"));
    }

    #[test]
    fn empty_program_links_and_runs() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let obj = tmp.path().join(if cfg!(windows) { "m.obj" } else { "m.o" });
        let exe = tmp
            .path()
            .join(format!("m{}", std::env::consts::EXE_SUFFIX));

        emit::emit_empty_main(&obj).expect("emit object");
        // Whole-archive the runtime so the closure must actually resolve, not
        // just parse as link args (FD-048 decision 2).
        link::link(&obj, &exe, link::WholeArchive::Yes).expect("link runtime closure");

        let status = std::process::Command::new(&exe)
            .status()
            .expect("run produced exe");
        assert_eq!(status.code(), Some(0), "empty program should exit 0");
    }
}
