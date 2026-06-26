//! Object-file emission for the AOT pipeline (FD-048).
//!
//! Builds a fixed *empty program* — a module with a single `i32 @main()` that
//! returns `0` — and writes it as a native object file. The CoolBasic IR is not
//! consulted: this FD stands up the object-emit + link back-half independently
//! of IR→LLVM instruction selection (a later FD). When real lowering lands it
//! replaces the body here; the surrounding target/object machinery is unchanged.

use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

/// Emit `i32 @main() { ret i32 0 }` as a native object file at `obj_path`.
///
/// Host-portable: `Target::initialize_native` registers only the host target, so
/// no per-architecture inkwell feature (`target-x86`, …) is required. Objects are
/// emitted PIC on non-Windows so the platform `cc` can link a PIE by default;
/// Windows uses the default reloc model.
pub fn emit_empty_main(obj_path: &Path) -> Result<(), String> {
    let context = Context::create();
    let module = context.create_module("cb_main");
    let builder = context.create_builder();

    // fn main() -> i32 { ret 0 }
    let i32_type = context.i32_type();
    let fn_type = i32_type.fn_type(&[], false);
    let function = module.add_function("main", fn_type, None);
    let entry = context.append_basic_block(function, "entry");
    builder.position_at_end(entry);
    builder
        .build_return(Some(&i32_type.const_int(0, false)))
        .map_err(|e| format!("build main return: {e}"))?;

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

    machine
        .write_to_file(&module, FileType::Object, obj_path)
        .map_err(|e| format!("write object {}: {e}", obj_path.display()))
}
