//! IR → LLVM lowering orchestrator (FD-049 Phase 1).
//!
//! [`build_object`] is the entry point used by the `Backend` impl: build an
//! in-memory `inkwell` module from the lowered CoolBasic IR, verify it, and hand
//! it to [`crate::emit::write_module`] for native object emission. The link step
//! (CRT + runtime closure) stays in [`crate::link`].
//!
//! Scope is the Phase-1 scalar core (user functions, control flow, runtime
//! calls, strings, `Print`) plus the Phase-2 array surface (`New`/`Dim`,
//! index/`Redim`/`Len`/`For Each` via the `cb_rt_array_*` heap helpers). User
//! Types/structs and the `Trap` hard-fault path are later phases — reaching one
//! fails the codegen loudly rather than miscompiling.

mod func;
mod regtypes;
mod runtime;
mod types;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::values::{FunctionValue, GlobalValue};

use cb_diagnostics::Interner;
use cb_ir::{Function, IrType, Program};

use func::FunctionLowerer;

/// The shared lowering context: the LLVM context/module/builder plus the
/// declared user functions, globals, and the lazy runtime-symbol cache.
pub(crate) struct Codegen<'a, 'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    program: &'a Program,
    interner: &'a Interner,
    /// body_index → declared user `FunctionValue`.
    user_funcs: Vec<FunctionValue<'ctx>>,
    /// GlobalId → declared global variable.
    globals: Vec<GlobalValue<'ctx>>,
    /// Lazy `symbol → FunctionValue` runtime declaration cache (runtime.rs).
    runtime: RefCell<HashMap<String, FunctionValue<'ctx>>>,
    /// Counter for unique private string-literal global names.
    str_counter: Cell<u32>,
    /// Lazy `TypeDefId.0 → node struct type` cache (FD-049 Phase 3a). The node
    /// LLVM type is `{ptr, ptr, ptr, i32, i32, <field basic_types…>}` — the
    /// 32-byte `CbTypeHeader` prefix plus the type's inline fields.
    node_types: RefCell<HashMap<u32, inkwell::types::StructType<'ctx>>>,
}

impl<'a, 'ctx> Codegen<'a, 'ctx> {
    fn new(ctx: &'ctx Context, program: &'a Program, interner: &'a Interner) -> Self {
        Self {
            ctx,
            module: ctx.create_module("cb_main"),
            builder: ctx.create_builder(),
            program,
            interner,
            user_funcs: Vec::new(),
            globals: Vec::new(),
            runtime: RefCell::new(HashMap::new()),
            str_counter: Cell::new(0),
            node_types: RefCell::new(HashMap::new()),
        }
    }

    /// Next unique id for a private string-literal global.
    pub(crate) fn next_str_id(&self) -> u32 {
        let id = self.str_counter.get();
        self.str_counter.set(id + 1);
        id
    }

    /// Declare every user-defined function (body) up front so calls (including
    /// forward/recursive ones) resolve. The top-level body (`@main`) is renamed
    /// `cb_user_main` — the bootstrap `main` calls it; the rest get a stable
    /// generated symbol to avoid colliding with runtime symbols.
    fn declare_user_functions(&mut self) -> Result<usize, String> {
        let mut main_idx = None;
        for (idx, func) in self.program.functions.iter().enumerate() {
            let is_main = self.interner.resolve(func.name) == "@main";
            let name = if is_main {
                "cb_user_main".to_string()
            } else {
                format!("cb_user_fn{idx}")
            };
            let fty = types::fn_type(
                self.ctx,
                &self.program.struct_defs,
                &func.params,
                &func.return_type,
            )?;
            let fv = self.module.add_function(&name, fty, None);
            self.user_funcs.push(fv);
            if is_main {
                main_idx = Some(idx);
            }
        }
        main_idx.ok_or_else(|| "no @main function in the IR program".to_string())
    }

    /// Declare every program global as an `internal` variable: numerics zero-
    /// initialized; String/reference globals null-initialized. (Null is a safe
    /// empty String for Phase 1 — every runtime string primitive null-checks;
    /// real String globals are rare since top-level `Dim` lowers to `@main`
    /// locals.)
    fn declare_globals(&mut self) -> Result<(), String> {
        for (i, g) in self.program.globals.iter().enumerate() {
            let ty = types::basic_type(self.ctx, &self.program.struct_defs, &g.ty)?;
            let gv = self.module.add_global(ty, None, &format!("cb_global{i}"));
            gv.set_linkage(Linkage::Internal);
            match &g.ty {
                IrType::Byte => gv.set_initializer(&self.ctx.i8_type().const_zero()),
                IrType::Short => gv.set_initializer(&self.ctx.i16_type().const_zero()),
                IrType::Int => gv.set_initializer(&self.ctx.i32_type().const_zero()),
                IrType::Long => gv.set_initializer(&self.ctx.i64_type().const_zero()),
                IrType::Float => gv.set_initializer(&self.ctx.f64_type().const_zero()),
                IrType::String | IrType::Null | IrType::Array { .. } | IrType::TypeRef(_) => {
                    gv.set_initializer(&self.ptr_t().const_null())
                }
                // A value-struct global is zero-initialized (FD-049 Phase 3b).
                // Its String sub-fields stay null rather than the empty sentinel
                // — there is no runtime global-init hook to set them, and every
                // string primitive null-checks (the same scoped simplification
                // Phase 1 made for String globals; top-level `Dim` lowers to
                // `@main` locals, where the sentinel IS set, so this is rare).
                IrType::StructVal(_) => {
                    gv.set_initializer(&ty.into_struct_type().const_zero())
                }
                other => {
                    return Err(format!(
                        "global of type {other:?} is out of scope for the Phase-1 LLVM backend"
                    ));
                }
            }
            self.globals.push(gv);
        }
        Ok(())
    }

    /// Emit the C entry point: `i32 main() { cb_rt_standalone_run(@cb_user_main); ret 0 }`.
    fn build_entry_point(&self, main_idx: usize) -> Result<(), String> {
        let main_fn =
            self.module
                .add_function("main", self.ctx.i32_type().fn_type(&[], false), None);
        let entry = self.ctx.append_basic_block(main_fn, "entry");
        self.builder.position_at_end(entry);

        let user_main = self.user_funcs[main_idx];
        let user_main_ptr = user_main.as_global_value().as_pointer_value();
        self.builder
            .build_call(self.rt_standalone_run(), &[user_main_ptr.into()], "")
            .map_err(|e| format!("llvm builder error: {e:?}"))?;
        self.builder
            .build_return(Some(&self.ctx.i32_type().const_int(0, false)))
            .map_err(|e| format!("llvm builder error: {e:?}"))?;
        Ok(())
    }
}

/// Lower `program` to a native object at `obj_path`.
pub fn build_object(program: &Program, interner: &Interner, obj_path: &Path) -> Result<(), String> {
    let ctx = Context::create();
    let mut cg = Codegen::new(&ctx, program, interner);

    let main_idx = cg.declare_user_functions()?;
    cg.declare_globals()?;
    for (idx, func) in program.functions.iter().enumerate() {
        lower_function(&cg, idx, func)?;
    }
    cg.build_entry_point(main_idx)?;

    cg.module.verify().map_err(|e| {
        format!(
            "internal error: generated LLVM module failed verification: {}\n\
             ---- module ----\n{}",
            e.to_string().trim_end(),
            cg.module.print_to_string().to_string()
        )
    })?;

    crate::emit::write_module(&cg.module, obj_path)
}

/// Lower one function body into its pre-declared `FunctionValue`.
fn lower_function(cg: &Codegen, body_index: usize, func: &Function) -> Result<(), String> {
    FunctionLowerer::new(cg, body_index, func).lower()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cb_diagnostics::{FileId, Interner, Span};
    use cb_ir::inst::{InstKind, Terminator};
    use cb_ir::{BasicBlock, BlockId, FnSig, FuncDecl, FuncKind, Inst, Reg};

    fn span() -> Span {
        Span::new(0, 0, FileId(0))
    }

    /// A program with a `@main` doing a little scalar arithmetic verifies and
    /// emits an object — covering declare → lower → verify → object-write.
    #[test]
    fn tiny_program_builds_and_verifies() {
        let mut interner = Interner::new();
        let main = interner.intern("@main");

        let blocks = vec![BasicBlock {
            id: BlockId(0),
            insts: vec![
                Inst {
                    result: Some(Reg(0)),
                    kind: InstKind::ConstInt(2),
                    span: span(),
                },
                Inst {
                    result: Some(Reg(1)),
                    kind: InstKind::ConstInt(3),
                    span: span(),
                },
                Inst {
                    result: Some(Reg(2)),
                    kind: InstKind::BinOp {
                        op: cb_ir::IrBinOp::Add,
                        lhs: Reg(0),
                        rhs: Reg(1),
                    },
                    span: span(),
                },
            ],
            terminator: Some(Terminator::Return { value: None }),
            terminator_span: span(),
        }];

        let program = Program {
            func_table: vec![FuncDecl {
                name: main,
                sig: FnSig {
                    params: vec![],
                    ret: Box::new(IrType::Void),
                },
                kind: FuncKind::UserDefined { body_index: 0 },
            }],
            functions: vec![Function {
                name: main,
                params: vec![],
                return_type: IrType::Void,
                locals: vec![],
                blocks,
            }],
            globals: vec![],
            type_defs: vec![],
            struct_defs: vec![],
        };

        let tmp = tempfile::tempdir().expect("temp dir");
        let obj = tmp.path().join(if cfg!(windows) { "t.obj" } else { "t.o" });
        build_object(&program, &interner, &obj).expect("build_object should succeed");
        assert!(obj.is_file(), "object file should be written");
    }
}
