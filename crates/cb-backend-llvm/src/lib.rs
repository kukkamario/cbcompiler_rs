//! LLVM backend for CoolBasic IR.
//!
//! Under the `codegen` feature this drives the full AOT pipeline: lower the
//! CoolBasic IR to an in-memory `inkwell` module, emit a native object,
//! and link it — against the CoolBasic runtime closure — into a runnable
//! executable. It lowers the scalar core: user functions, control
//! flow, runtime calls, strings, and `Print`; the produced exe's stdout + exit
//! code match the interpreter (the reference oracle). Without `codegen`,
//! [`Backend::execute`] still reports the gap (driver exit 3) so the driver
//! dispatches through the shared trait ahead of codegen.

use std::path::PathBuf;

use cb_backend_api::{Backend, BackendError, BackendOutcome};
use cb_diagnostics::Interner;
use cb_ir::Program;

#[cfg(feature = "codegen")]
mod codegen;
#[cfg(feature = "codegen")]
mod emit;
#[cfg(feature = "codegen")]
mod link;

/// Stage a relocatable copy of the CoolBasic runtime under `dest` (which becomes
/// `<exe-dir>/lib` in a published release), so a moved `cb` links AOT output
/// without the build machine's paths. See [`link::stage_runtime_bundle`].
#[cfg(feature = "codegen")]
pub use link::{BundleReport, stage_runtime_bundle};

/// Optimization level for the AOT pipeline, selected by the driver's `-O` flag
/// and injected at construction. It drives **both** knobs together: the IR-level
/// pass pipeline (`Module::run_passes`) and the codegen-level `TargetMachine`
/// optimization level — see [`emit::write_module`].
///
/// Defined unconditionally (not behind `codegen`) so [`LlvmBackend::new`] keeps a
/// stable signature in the LLVM-free stub build; the inkwell mappings live next
/// to the machine in `emit`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum OptLevel {
    /// No optimization: skip the IR passes and keep the `TargetMachine` at
    /// `None`. Preserves the rawest lowering output — best for debugging native
    /// codegen, and exactly the pre-FD-054 behavior.
    O0,
    /// Light optimization.
    O1,
    /// The default for the LLVM backend — its reason to exist is optimized
    /// native code.
    #[default]
    O2,
    /// Aggressive optimization.
    O3,
    /// Optimize for size.
    Os,
    /// Optimize aggressively for size.
    Oz,
}

/// The LLVM / AOT backend. The `source`/`output`/`opt` settings are injected at
/// construction because the IR carries no source path and
/// the [`Backend::execute`] signature passes only `&Program` +
/// `&Interner` — so codegen could not otherwise name the artifact or know the
/// requested optimization level. They are unused in the no-`codegen` stub build.
pub struct LlvmBackend {
    #[cfg_attr(not(feature = "codegen"), allow(dead_code))]
    source: PathBuf,
    #[cfg_attr(not(feature = "codegen"), allow(dead_code))]
    output: Option<PathBuf>,
    #[cfg_attr(not(feature = "codegen"), allow(dead_code))]
    opt: OptLevel,
}

impl LlvmBackend {
    /// Construct the backend for `source`, writing the artifact to `output` (or,
    /// when `None`, next to the source as `<stem>` + the platform exe suffix), at
    /// optimization level `opt`.
    pub fn new(source: PathBuf, output: Option<PathBuf>, opt: OptLevel) -> Self {
        Self {
            source,
            output,
            opt,
        }
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

    /// Lower the IR to a native object and link it into a runtime-linked exe,
    /// returning [`BackendOutcome::Produced`]. Any codegen/link failure becomes
    /// `BackendError::failed` → driver exit 1.
    #[cfg(feature = "codegen")]
    fn execute(
        &self,
        program: &Program,
        interner: &Interner,
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

        codegen::build_object(program, interner, &obj, self.opt).map_err(BackendError::failed)?;
        link::link(&obj, &artifact, link::WholeArchive::No).map_err(BackendError::failed)?;

        Ok(BackendOutcome::Produced { artifact })
    }

    /// Stub path: codegen is compiled out, so report the gap explicitly (driver
    /// exit 3) rather than silently doing nothing.
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

// AOT smoke tests: a linkage check plus emit→link→run. Gated on
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
    fn trivial_module_links_and_runs() {
        // A minimal `i32 main() { ret 0 }` built directly, then emit→link→run.
        // Keeps the object-emit + runtime-closure-resolution smoke green
        // independent of the IR→LLVM lowering (which the diff suite exercises).
        let ctx = inkwell::context::Context::create();
        let module = ctx.create_module("cb_smoke");
        let builder = ctx.create_builder();
        let i32_t = ctx.i32_type();
        let main = module.add_function("main", i32_t.fn_type(&[], false), None);
        let entry = ctx.append_basic_block(main, "entry");
        builder.position_at_end(entry);
        builder
            .build_return(Some(&i32_t.const_int(0, false)))
            .expect("build return");

        let tmp = tempfile::tempdir().expect("temp dir");
        let obj = tmp.path().join(if cfg!(windows) { "m.obj" } else { "m.o" });
        let exe = tmp
            .path()
            .join(format!("m{}", std::env::consts::EXE_SUFFIX));

        emit::write_module(&module, &obj, crate::OptLevel::O2).expect("emit object");
        // Whole-archive the runtime so the closure must actually resolve, not
        // just parse as link args.
        link::link(&obj, &exe, link::WholeArchive::Yes).expect("link runtime closure");

        let status = std::process::Command::new(&exe)
            .status()
            .expect("run produced exe");
        assert_eq!(status.code(), Some(0), "trivial program should exit 0");
    }
}
