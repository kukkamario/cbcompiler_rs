//! Object-file emission for the AOT pipeline (FD-048, FD-049).
//!
//! The target/object-write back-half: given a finished `inkwell` module, select
//! the host `TargetMachine` and write a native object file. The module contents
//! (the IR→LLVM lowering) are built by [`crate::codegen`]; this module is
//! lowering-agnostic.

use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::module::Module;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

/// Write `module` to a native object file at `obj_path`.
///
/// Host-portable: `Target::initialize_native` registers only the host target, so
/// no per-architecture inkwell feature (`target-x86`, …) is required. Objects are
/// emitted PIC on non-Windows so the platform `cc` can link a PIE by default;
/// Windows uses the default reloc model.
pub fn write_module(module: &Module, obj_path: &Path) -> Result<(), String> {
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
            OptimizationLevel::None,
            reloc,
            CodeModel::Default,
        )
        .ok_or_else(|| "failed to create target machine for the host triple".to_string())?;

    // Pin the module's target triple so the object's headers match the machine.
    module.set_triple(&triple);

    machine
        .write_to_file(module, FileType::Object, obj_path)
        .map_err(|e| format!("write object {}: {e}", obj_path.display()))
}
