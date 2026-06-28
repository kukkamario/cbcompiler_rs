//! Object-file emission for the AOT pipeline.
//!
//! The target/object-write back-half: given a finished `inkwell` module, select
//! the host `TargetMachine` and write a native object file. The module contents
//! (the IR→LLVM lowering) are built by [`crate::codegen`]; this module is
//! lowering-agnostic.

use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::module::Module;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use crate::OptLevel;

/// Write `module` to a native object file at `obj_path`, optimized at `opt`.
///
/// Optimization has two coordinated halves, both driven by `opt`: the IR-level
/// `default<Ox>` pass pipeline (run here via `Module::run_passes`) and the
/// codegen-level `TargetMachine` optimization (instruction selection, scheduling,
/// register allocation). At `O0` neither runs, so the rawest lowering is emitted.
///
/// Host-portable: `Target::initialize_native` registers only the host target, so
/// no per-architecture inkwell feature (`target-x86`, …) is required. Objects are
/// emitted PIC on non-Windows so the platform `cc` can link a PIE by default;
/// Windows uses the default reloc model.
pub fn write_module(module: &Module, obj_path: &Path, opt: OptLevel) -> Result<(), String> {
    // Host target machine.
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("initialize native target: {e}"))?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| format!("look up host target: {e}"))?;

    let reloc = if cfg!(target_os = "windows") {
        RelocMode::Default
    } else {
        RelocMode::PIC
    };
    let cpu = TargetMachine::get_host_cpu_name();
    let features = TargetMachine::get_host_cpu_features();
    let machine = target
        .create_target_machine(
            &triple,
            cpu.to_str().unwrap_or("generic"),
            features.to_str().unwrap_or(""),
            codegen_opt_level(opt),
            reloc,
            CodeModel::Default,
        )
        .ok_or_else(|| "failed to create target machine for the host triple".to_string())?;

    // Pin the module's target triple so the object's headers match the machine.
    module.set_triple(&triple);

    // IR-level optimization: run the new-pass-manager `default<Ox>` pipeline over
    // the module before codegen. Skipped at O0 so the lowering is emitted
    // untouched. The pass pipeline targets `machine`, so cost models and any
    // size bias (`Os`/`Oz`) see the real host triple/CPU.
    if let Some(pipeline) = pass_pipeline(opt) {
        module
            .run_passes(pipeline, &machine, PassBuilderOptions::create())
            .map_err(|e| {
                format!(
                    "run optimization passes ({pipeline}): {}",
                    e.to_string().trim_end()
                )
            })?;
    }

    machine
        .write_to_file(module, FileType::Object, obj_path)
        .map_err(|e| format!("write object {}: {e}", obj_path.display()))
}

/// The codegen-level optimization for the `TargetMachine`. The machine has no
/// size knob, so `Os`/`Oz` use the `Default` codegen level — their size bias is
/// carried entirely by the [`pass_pipeline`] string.
fn codegen_opt_level(opt: OptLevel) -> OptimizationLevel {
    match opt {
        OptLevel::O0 => OptimizationLevel::None,
        OptLevel::O1 => OptimizationLevel::Less,
        OptLevel::O2 | OptLevel::Os | OptLevel::Oz => OptimizationLevel::Default,
        OptLevel::O3 => OptimizationLevel::Aggressive,
    }
}

/// The new-pass-manager pipeline string for `opt`, or `None` at `O0` (no IR
/// passes run). The format matches `opt`'s `-passes=` argument for the new pass
/// manager, which is what `Module::run_passes` consumes.
fn pass_pipeline(opt: OptLevel) -> Option<&'static str> {
    Some(match opt {
        OptLevel::O0 => return None,
        OptLevel::O1 => "default<O1>",
        OptLevel::O2 => "default<O2>",
        OptLevel::O3 => "default<O3>",
        OptLevel::Os => "default<Os>",
        OptLevel::Oz => "default<Oz>",
    })
}
