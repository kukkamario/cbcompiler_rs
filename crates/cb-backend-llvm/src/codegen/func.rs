//! Per-function lowering: IR blocks/instructions/terminators → LLVM (FD-049).
//!
//! One LLVM block is pre-created per IR `BlockId` (so forward branches resolve);
//! locals become entry-block `alloca` slots (zero/null-initialized, then the
//! incoming params stored — String params retained into their slot). Registers
//! are SSA values held in a `Reg → BasicValueEnum` map; because the IR respects
//! dominance, a value is always available where it is used without phi nodes
//! (mutable variables flow through the alloca slots, not registers).
//!
//! String refcounting follows FD-049 decision B: producers own +1, loads retain,
//! stores release-old then move-in, call args borrow, and unconsumed owned temps
//! are released after their last use (from the regtypes pass). Every String
//! local slot is released at each `Return`.

use std::collections::HashMap;

use inkwell::basic_block::BasicBlock as LlvmBlock;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FloatValue, FunctionValue, IntValue,
    PointerValue,
};
use inkwell::{FloatPredicate, IntPredicate};

use cb_ir::inst::{InstKind, IrBinOp, IrUnOp, PlaceRoot, Projection, Terminator};
use cb_ir::{BlockId, FuncKind, Function, Inst, IrType, Reg};

use super::Codegen;
use super::regtypes::{self, RegInfo};
use super::types::basic_type;

/// Format a builder error as a String (BuilderError isn't `Display`-friendly).
fn berr<E: std::fmt::Debug>(e: E) -> String {
    format!("llvm builder error: {e:?}")
}

pub(super) struct FunctionLowerer<'a, 'ctx, 'f> {
    cg: &'f Codegen<'a, 'ctx>,
    func: &'f Function,
    llvm_func: FunctionValue<'ctx>,
    info: RegInfo,
    /// The synthetic entry block that holds the allocas and branches to bb0.
    alloca_bb: LlvmBlock<'ctx>,
    /// IR BlockId → LLVM block.
    blocks: HashMap<BlockId, LlvmBlock<'ctx>>,
    /// LocalId index → its alloca slot.
    locals: Vec<PointerValue<'ctx>>,
    /// SSA reg → its LLVM value.
    regs: HashMap<Reg, BasicValueEnum<'ctx>>,
    /// One reusable entry-block `[maxRank x i64]` scratch buffer for array
    /// index/dims lists (FD-049 Phase 2). A per-instruction alloca inside a
    /// loop body would grow the O0 stack each iteration; this single buffer is
    /// rewritten and consumed synchronously by each `cb_rt_array_*` call. The
    /// struct-of-i64 layout is contiguous (passed as the `int64_t*` arg) and
    /// lets us index it with the safe `build_struct_gep`. `None` when the
    /// function has no array ops.
    idx_scratch: Option<(PointerValue<'ctx>, inkwell::types::StructType<'ctx>)>,
}

impl<'a, 'ctx, 'f> FunctionLowerer<'a, 'ctx, 'f> {
    pub(super) fn new(cg: &'f Codegen<'a, 'ctx>, body_index: usize, func: &'f Function) -> Self {
        let llvm_func = cg.user_funcs[body_index];
        let info = regtypes::analyze(func, cg.program);
        // Append the alloca/entry block FIRST so it is the LLVM function entry,
        // then one block per IR block (so branch targets resolve).
        let alloca_bb = cg.ctx.append_basic_block(llvm_func, "entry");
        let mut blocks = HashMap::new();
        for b in &func.blocks {
            blocks.insert(
                b.id,
                cg.ctx
                    .append_basic_block(llvm_func, &format!("bb{}", b.id.0)),
            );
        }
        Self {
            cg,
            func,
            llvm_func,
            info,
            alloca_bb,
            blocks,
            locals: Vec::new(),
            regs: HashMap::new(),
            idx_scratch: None,
        }
    }

    pub(super) fn lower(mut self) -> Result<(), String> {
        self.emit_entry()?;
        // Copy the `&Function` (a Copy reference) into a local so iterating it
        // does not hold a borrow of `self` while `&mut self` methods run.
        let func = self.func;
        for block in &func.blocks {
            let llb = self.blocks[&block.id];
            self.cg.builder.position_at_end(llb);
            for (i, inst) in block.insts.iter().enumerate() {
                self.lower_inst(inst)?;
                // Release any owned String temps whose last in-block use is here.
                if let Some(regs) = self.info.releases.get(&(block.id, i)) {
                    let regs = regs.clone();
                    for r in regs {
                        self.release_reg(r)?;
                    }
                }
            }
            self.lower_terminator(block.terminator.as_ref())?;
        }
        Ok(())
    }

    // ── Entry: allocas, default-init, params ────────────────────────────

    fn emit_entry(&mut self) -> Result<(), String> {
        let func = self.func;
        self.cg.builder.position_at_end(self.alloca_bb);

        // One alloca per local.
        for (i, local) in func.locals.iter().enumerate() {
            let lty = basic_type(self.cg.ctx, &local.ty)?;
            let slot = self
                .cg
                .builder
                .build_alloca(lty, &format!("local{i}"))
                .map_err(berr)?;
            self.locals.push(slot);
        }
        // One reusable `[maxRank x i64]` scratch for array index/dims lists
        // (FD-049 Phase 2). Allocated once in the entry block — never per
        // instruction — so a loop body with an indexed array does not grow the
        // O0 stack each iteration. Skipped entirely when the function has no
        // array ops (max rank 0).
        let max_rank = self.max_index_rank();
        if max_rank > 0 {
            let i64t = self.cg.ctx.i64_type();
            let buf_ty = self
                .cg
                .ctx
                .struct_type(&vec![i64t.into(); max_rank], false);
            let buf = self
                .cg
                .builder
                .build_alloca(buf_ty, "idxbuf")
                .map_err(berr)?;
            self.idx_scratch = Some((buf, buf_ty));
        }
        // Default-init every slot (interp default-inits all locals; String slots
        // MUST be initialized before any release-on-store sees garbage).
        for (i, local) in func.locals.iter().enumerate() {
            let zero = self.default_value(&local.ty)?;
            self.cg
                .builder
                .build_store(self.locals[i], zero)
                .map_err(berr)?;
        }
        // Store incoming params into their slots (in declaration order). A String
        // param is retained into its slot — the caller passed it borrowed.
        let mut p = 0u32;
        for (i, local) in func.locals.iter().enumerate() {
            if !local.is_param {
                continue;
            }
            let param = self
                .llvm_func
                .get_nth_param(p)
                .ok_or_else(|| format!("missing LLVM param {p}"))?;
            p += 1;
            if matches!(local.ty, IrType::String) {
                let retained = self.call_value(self.cg.rt_string_retain(), &[param.into()])?;
                self.cg
                    .builder
                    .build_store(self.locals[i], retained)
                    .map_err(berr)?;
            } else {
                self.cg
                    .builder
                    .build_store(self.locals[i], param)
                    .map_err(berr)?;
            }
        }

        let ir_entry = self.blocks[&func.blocks[0].id];
        self.cg
            .builder
            .build_unconditional_branch(ir_entry)
            .map_err(berr)?;
        Ok(())
    }

    fn default_value(&self, ty: &IrType) -> Result<BasicValueEnum<'ctx>, String> {
        Ok(match ty {
            IrType::Byte => self.cg.ctx.i8_type().const_zero().into(),
            IrType::Short => self.cg.ctx.i16_type().const_zero().into(),
            IrType::Int => self.cg.ctx.i32_type().const_zero().into(),
            IrType::Long => self.cg.ctx.i64_type().const_zero().into(),
            IrType::Float => self.cg.ctx.f64_type().const_zero().into(),
            // String/Null and an array handle (`CbArray*`) default to a null
            // pointer (an un-`New`'d array is Null, matching the interpreter).
            IrType::String | IrType::Null | IrType::Array { .. } => {
                self.cg.ptr_t().const_null().into()
            }
            other => {
                return Err(format!(
                    "local of type {other:?} is out of scope for the Phase-1 LLVM backend"
                ));
            }
        })
    }

    // ── Instruction selection ───────────────────────────────────────────

    fn lower_inst(&mut self, inst: &Inst) -> Result<(), String> {
        match &inst.kind {
            InstKind::ConstInt(v) => {
                let val = self.cg.ctx.i32_type().const_int(*v as u64, false);
                self.bind(inst, val.into());
            }
            InstKind::ConstLong(v) => {
                let val = self.cg.ctx.i64_type().const_int(*v as u64, false);
                self.bind(inst, val.into());
            }
            InstKind::ConstFloat(v) => {
                let val = self.cg.ctx.f64_type().const_float(*v);
                self.bind(inst, val.into());
            }
            InstKind::ConstString(s) => {
                let val = self.const_string(s)?;
                self.bind(inst, val);
            }
            InstKind::ConstNull => {
                self.bind(inst, self.cg.ptr_t().const_null().into());
            }
            InstKind::BinOp { op, lhs, rhs } => {
                let val = self.lower_binop(*op, *lhs, *rhs)?;
                self.bind(inst, val);
            }
            InstKind::UnOp { op, operand } => {
                let val = self.lower_unop(*op, *operand)?;
                self.bind(inst, val);
            }
            InstKind::LoadLocal { local } => {
                let slot = self.locals[local.0 as usize];
                let ty = self.func.locals[local.0 as usize].ty.clone();
                let val = self.load_slot(slot, &ty)?;
                self.bind(inst, val);
            }
            InstKind::StoreLocal { local, value } => {
                let slot = self.locals[local.0 as usize];
                let ty = self.func.locals[local.0 as usize].ty.clone();
                self.store_slot(slot, &ty, *value)?;
            }
            InstKind::LoadGlobal { global } => {
                let slot = self.cg.globals[global.0 as usize].as_pointer_value();
                let ty = self.cg.program.globals[global.0 as usize].ty.clone();
                let val = self.load_slot(slot, &ty)?;
                self.bind(inst, val);
            }
            InstKind::StoreGlobal { global, value } => {
                let slot = self.cg.globals[global.0 as usize].as_pointer_value();
                let ty = self.cg.program.globals[global.0 as usize].ty.clone();
                self.store_slot(slot, &ty, *value)?;
            }
            InstKind::Convert { value, from, to } => {
                let val = self.lower_convert(*value, from, to)?;
                self.bind(inst, val);
            }
            InstKind::ConvertExplicit { value, target } => {
                let from = self
                    .info
                    .type_of(*value)
                    .cloned()
                    .ok_or_else(|| format!("untyped operand {value} for convert"))?;
                let val = self.lower_convert(*value, &from, target)?;
                self.bind(inst, val);
            }
            InstKind::StrLen { s } => {
                // Codepoint count (CB `Len(s$)`), not the byte length.
                let len =
                    self.call_value(self.cg.rt_string_char_len(), &[self.pval(*s)?.into()])?;
                let trunc = self
                    .cg
                    .builder
                    .build_int_truncate(len.into_int_value(), self.cg.ctx.i32_type(), "")
                    .map_err(berr)?;
                self.bind(inst, trunc.into());
            }
            InstKind::Call { callee, args } => {
                self.lower_call(inst, *callee, args)?;
            }

            // ── Arrays (FD-049 Phase 2) ────────────────────────────────
            InstKind::NewArray { elem_type, dims } => {
                let handle = self.build_new_array(elem_type, dims)?;
                self.bind(inst, handle);
            }
            InstKind::GetElement { array, indices } => {
                let elem = self.array_elem_type(*array)?;
                let handle = self.pval(*array)?;
                let buf = self.i64_buf(indices)?;
                let rank = self.cg.ctx.i64_type().const_int(indices.len() as u64, false);
                let elem_ptr = self
                    .call_value(
                        self.cg.rt_array_elem_addr(),
                        &[handle.into(), buf.into(), rank.into()],
                    )?
                    .into_pointer_value();
                let val = self.load_elem(elem_ptr, &elem)?;
                self.bind(inst, val);
            }
            InstKind::GetElementFlat { array, index } => {
                let elem = self.array_elem_type(*array)?;
                let handle = self.pval(*array)?;
                let idx = self.ext_to_i64(*index)?;
                let elem_ptr = self
                    .call_value(
                        self.cg.rt_array_elem_addr_flat(),
                        &[handle.into(), idx.into()],
                    )?
                    .into_pointer_value();
                let val = self.load_elem(elem_ptr, &elem)?;
                self.bind(inst, val);
            }
            InstKind::Len { array, dim } => {
                let handle = self.pval(*array)?;
                let d = match dim {
                    Some(d) => self.ext_to_i64(*d)?,
                    None => self.cg.ctx.i64_type().const_zero(),
                };
                let len = self
                    .call_value(self.cg.rt_array_dim_len(), &[handle.into(), d.into()])?
                    .into_int_value();
                let trunc = self
                    .cg
                    .builder
                    .build_int_truncate(len, self.cg.ctx.i32_type(), "")
                    .map_err(berr)?;
                self.bind(inst, trunc.into());
            }
            InstKind::ArrayTotalLen { array } => {
                let handle = self.pval(*array)?;
                let total = self
                    .call_value(self.cg.rt_array_total_len(), &[handle.into()])?
                    .into_int_value();
                let trunc = self
                    .cg
                    .builder
                    .build_int_truncate(total, self.cg.ctx.i32_type(), "")
                    .map_err(berr)?;
                self.bind(inst, trunc.into());
            }
            InstKind::Redim {
                local,
                elem_type,
                dims,
            } => {
                // Fresh, zero-initialised array; the slot's old handle leaks
                // (arrays are not freed/refcounted in Phase 2). No preserve.
                let handle = self.build_new_array(elem_type, dims)?;
                self.cg
                    .builder
                    .build_store(self.locals[local.0 as usize], handle)
                    .map_err(berr)?;
            }
            InstKind::RedimGlobal {
                global,
                elem_type,
                dims,
            } => {
                let handle = self.build_new_array(elem_type, dims)?;
                let slot = self.cg.globals[global.0 as usize].as_pointer_value();
                self.cg.builder.build_store(slot, handle).map_err(berr)?;
            }
            InstKind::StorePlace { root, path, value } => {
                self.lower_store_place(root, path, *value)?;
            }

            other => {
                return Err(format!(
                    "instruction {other:?} is out of scope for the Phase-1 LLVM backend"
                ));
            }
        }
        Ok(())
    }

    /// Bind an instruction's result register to a computed value.
    fn bind(&mut self, inst: &Inst, val: BasicValueEnum<'ctx>) {
        if let Some(r) = inst.result {
            self.regs.insert(r, val);
        }
    }

    fn load_slot(
        &self,
        slot: PointerValue<'ctx>,
        ty: &IrType,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let lty = basic_type(self.cg.ctx, ty)?;
        let v = self.cg.builder.build_load(lty, slot, "").map_err(berr)?;
        if matches!(ty, IrType::String) {
            // A loaded String is an independently-owned reg (+1).
            Ok(self.call_value(self.cg.rt_string_retain(), &[v.into()])?)
        } else {
            Ok(v)
        }
    }

    fn store_slot(&self, slot: PointerValue<'ctx>, ty: &IrType, value: Reg) -> Result<(), String> {
        if matches!(ty, IrType::String) {
            // Release the slot's prior contents, then move the reg's +1 in.
            let old = self
                .cg
                .builder
                .build_load(self.cg.ptr_t(), slot, "")
                .map_err(berr)?;
            self.call_void(self.cg.rt_string_release(), &[old.into()])?;
        }
        let val = self.regs[&value];
        self.cg.builder.build_store(slot, val).map_err(berr)?;
        Ok(())
    }

    // ── Array helpers (FD-049 Phase 2) ──────────────────────────────────

    /// The widest array index/dims list across every block — the size of the
    /// single reusable entry-block scratch buffer. `GetElementFlat`/`Len` pass
    /// their one index/dim directly (not via the buffer), so they don't count.
    fn max_index_rank(&self) -> usize {
        let mut max = 0;
        for block in &self.func.blocks {
            for inst in &block.insts {
                let n = match &inst.kind {
                    InstKind::NewArray { dims, .. }
                    | InstKind::Redim { dims, .. }
                    | InstKind::RedimGlobal { dims, .. } => dims.len(),
                    InstKind::GetElement { indices, .. } => indices.len(),
                    InstKind::StorePlace { path, .. } => path
                        .iter()
                        .map(|p| match p {
                            Projection::Index(idxs) => idxs.len(),
                            Projection::Field(_) => 0,
                        })
                        .max()
                        .unwrap_or(0),
                    _ => 0,
                };
                max = max.max(n);
            }
        }
        max
    }

    /// Sign-/zero-extend an integer reg to `i64` per its IR type — the width the
    /// array helpers take index/dim args in.
    fn ext_to_i64(&self, reg: Reg) -> Result<IntValue<'ctx>, String> {
        let ty = self
            .info
            .type_of(reg)
            .cloned()
            .ok_or_else(|| format!("untyped array index/dim reg {reg}"))?;
        self.ext_int(self.ival(reg)?, &ty, 64)
    }

    /// Fill the reusable scratch buffer with `regs` (each extended to i64) and
    /// return its base pointer (== the `int64_t*` arg the array helpers take).
    fn i64_buf(&self, regs: &[Reg]) -> Result<PointerValue<'ctx>, String> {
        let (buf, buf_ty) = self
            .idx_scratch
            .ok_or("array op without an allocated index scratch (prescan missed it)")?;
        for (i, r) in regs.iter().enumerate() {
            let v = self.ext_to_i64(*r)?;
            let slot = self
                .cg
                .builder
                .build_struct_gep(buf_ty, buf, i as u32, "")
                .map_err(berr)?;
            self.cg.builder.build_store(slot, v).map_err(berr)?;
        }
        Ok(buf)
    }

    /// Element type of the array a reg holds (its IR type is `Array { elem }`).
    fn array_elem_type(&self, array: Reg) -> Result<IrType, String> {
        match self.info.type_of(array) {
            Some(IrType::Array { elem, .. }) => Ok((**elem).clone()),
            other => Err(format!(
                "expected an array reg for {array}, found {other:?}"
            )),
        }
    }

    /// Load an element through `elem_ptr`, retaining it if it is a String (a
    /// loaded String is an independently-owned +1 reg), mirroring `load_slot`.
    fn load_elem(
        &self,
        elem_ptr: PointerValue<'ctx>,
        elem: &IrType,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let lty = basic_type(self.cg.ctx, elem)?;
        let v = self.cg.builder.build_load(lty, elem_ptr, "").map_err(berr)?;
        if matches!(elem, IrType::String) {
            Ok(self.call_value(self.cg.rt_string_retain(), &[v.into()])?)
        } else {
            Ok(v)
        }
    }

    /// Lower a `NewArray`/`Redim` allocation: build the dims into the scratch,
    /// then call `cb_rt_array_new(rank, dims, elem_size, elem_is_ref)`.
    fn build_new_array(
        &self,
        elem_type: &IrType,
        dims: &[Reg],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (elem_size, elem_is_ref) = elem_layout(elem_type)?;
        let buf = self.i64_buf(dims)?;
        let rank = self.cg.ctx.i64_type().const_int(dims.len() as u64, false);
        let esize = self.cg.ctx.i64_type().const_int(elem_size, false);
        let eref = self
            .cg
            .ctx
            .i32_type()
            .const_int(elem_is_ref as u64, false);
        self.call_value(
            self.cg.rt_array_new(),
            &[rank.into(), buf.into(), esize.into(), eref.into()],
        )
    }

    /// Lower a `StorePlace` (Phase 2 = a single `Index` projection into an
    /// array element). Resolve the root slot, load the handle, address the
    /// element, then the Phase-1 String discipline (release the old element,
    /// move the value reg's +1 in); non-String elements plain-store.
    fn lower_store_place(
        &self,
        root: &PlaceRoot,
        path: &[Projection],
        value: Reg,
    ) -> Result<(), String> {
        // Phase 2 only handles `arr[idxs] = v` — exactly one Index step. A
        // field projection (struct/Type fields) is Phase 3.
        let idxs = match path {
            [Projection::Index(idxs)] => idxs.as_slice(),
            [Projection::Field(_)] => {
                return Err("StorePlace field projection is out of Phase-2 scope".into());
            }
            _ => {
                return Err(format!(
                    "StorePlace with a {}-step path is out of Phase-2 scope",
                    path.len()
                ));
            }
        };

        // Resolve the root variable's slot and its (array) IR type.
        let (slot, root_ty) = match root {
            PlaceRoot::Local(id) => (
                self.locals[id.0 as usize],
                self.func.locals[id.0 as usize].ty.clone(),
            ),
            PlaceRoot::Global(id) => (
                self.cg.globals[id.0 as usize].as_pointer_value(),
                self.cg.program.globals[id.0 as usize].ty.clone(),
            ),
        };
        let elem = match &root_ty {
            IrType::Array { elem, .. } => (**elem).clone(),
            other => return Err(format!("StorePlace root is not an array: {other:?}")),
        };

        // Load the array handle and address the element.
        let handle = self
            .cg
            .builder
            .build_load(self.cg.ptr_t(), slot, "")
            .map_err(berr)?;
        let buf = self.i64_buf(idxs)?;
        let rank = self.cg.ctx.i64_type().const_int(idxs.len() as u64, false);
        let elem_ptr = self
            .call_value(
                self.cg.rt_array_elem_addr(),
                &[handle.into(), buf.into(), rank.into()],
            )?
            .into_pointer_value();

        // String element: release the slot's prior contents, then move the
        // reg's +1 in. Non-String: plain store.
        if matches!(elem, IrType::String) {
            let old = self
                .cg
                .builder
                .build_load(self.cg.ptr_t(), elem_ptr, "")
                .map_err(berr)?;
            self.call_void(self.cg.rt_string_release(), &[old.into()])?;
        }
        let val = self.regs[&value];
        self.cg.builder.build_store(elem_ptr, val).map_err(berr)?;
        Ok(())
    }

    fn lower_binop(
        &mut self,
        op: IrBinOp,
        lhs: Reg,
        rhs: Reg,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use IrBinOp::*;
        let lty = self
            .info
            .type_of(lhs)
            .cloned()
            .ok_or_else(|| format!("untyped binop lhs {lhs}"))?;

        // String operations.
        if op == StrConcat {
            return self.call_value(
                self.cg.rt_string_concat(),
                &[self.pval(lhs)?.into(), self.pval(rhs)?.into()],
            );
        }
        if matches!(op, StrEq | StrNotEq | StrLt | StrGt | StrLtEq | StrGtEq) {
            let cmp = self
                .call_value(
                    self.cg.rt_string_compare(),
                    &[self.pval(lhs)?.into(), self.pval(rhs)?.into()],
                )?
                .into_int_value();
            let zero = self.cg.ctx.i32_type().const_zero();
            let pred = match op {
                StrEq => IntPredicate::EQ,
                StrNotEq => IntPredicate::NE,
                StrLt => IntPredicate::SLT,
                StrGt => IntPredicate::SGT,
                StrLtEq => IntPredicate::SLE,
                _ => IntPredicate::SGE, // StrGtEq
            };
            let i1 = self
                .cg
                .builder
                .build_int_compare(pred, cmp, zero, "")
                .map_err(berr)?;
            return Ok(self.zext_i32(i1)?.into());
        }

        let b = &self.cg.builder;
        // Float arithmetic / comparison.
        if matches!(lty, IrType::Float) {
            let l = self.fval(lhs)?;
            let r = self.fval(rhs)?;
            let v: BasicValueEnum = match op {
                Add => b.build_float_add(l, r, "").map_err(berr)?.into(),
                Sub => b.build_float_sub(l, r, "").map_err(berr)?.into(),
                Mul => b.build_float_mul(l, r, "").map_err(berr)?.into(),
                Div => b.build_float_div(l, r, "").map_err(berr)?.into(),
                Mod => b.build_float_rem(l, r, "").map_err(berr)?.into(),
                Pow => self.call_pow(l, r)?.into(),
                Eq | NotEq | Lt | Gt | LtEq | GtEq => {
                    let pred = match op {
                        Eq => FloatPredicate::OEQ,
                        NotEq => FloatPredicate::UNE,
                        Lt => FloatPredicate::OLT,
                        Gt => FloatPredicate::OGT,
                        LtEq => FloatPredicate::OLE,
                        _ => FloatPredicate::OGE, // GtEq
                    };
                    let i1 = b.build_float_compare(pred, l, r, "").map_err(berr)?;
                    self.zext_i32(i1)?.into()
                }
                _ => return Err(format!("unsupported float binop {op:?}")),
            };
            return Ok(v);
        }

        // Integer arithmetic / bitwise / shift / comparison. Operands widen to
        // the operation width (i32, or i64 if Long is involved); Byte/Short
        // zero-extend, Int/Long sign-extend — matching the interpreter's
        // `to_i64` widening with an i32 result for the Byte/Short/Int class.
        let rty = self
            .info
            .type_of(rhs)
            .cloned()
            .ok_or_else(|| format!("untyped binop rhs {rhs}"))?;
        let width = if matches!(lty, IrType::Long) || matches!(rty, IrType::Long) {
            64
        } else {
            32
        };
        let l = self.ext_int(self.ival(lhs)?, &lty, width)?;
        let r = self.ext_int(self.ival(rhs)?, &rty, width)?;
        let b = &self.cg.builder;
        let v: BasicValueEnum = match op {
            Add => b.build_int_add(l, r, "").map_err(berr)?.into(),
            Sub => b.build_int_sub(l, r, "").map_err(berr)?.into(),
            Mul => b.build_int_mul(l, r, "").map_err(berr)?.into(),
            Div => b.build_int_signed_div(l, r, "").map_err(berr)?.into(),
            Mod => b.build_int_signed_rem(l, r, "").map_err(berr)?.into(),
            BinAnd => b.build_and(l, r, "").map_err(berr)?.into(),
            BinOr => b.build_or(l, r, "").map_err(berr)?.into(),
            BinXor => b.build_xor(l, r, "").map_err(berr)?.into(),
            Shl => {
                let c = self.mask_shift(r, width)?;
                b.build_left_shift(l, c, "").map_err(berr)?.into()
            }
            Shr => {
                let c = self.mask_shift(r, width)?;
                b.build_right_shift(l, c, false, "").map_err(berr)?.into()
            }
            Sar => {
                let c = self.mask_shift(r, width)?;
                b.build_right_shift(l, c, true, "").map_err(berr)?.into()
            }
            Eq | NotEq | Lt | Gt | LtEq | GtEq => {
                let pred = match op {
                    Eq => IntPredicate::EQ,
                    NotEq => IntPredicate::NE,
                    Lt => IntPredicate::SLT,
                    Gt => IntPredicate::SGT,
                    LtEq => IntPredicate::SLE,
                    _ => IntPredicate::SGE, // GtEq
                };
                let i1 = b.build_int_compare(pred, l, r, "").map_err(berr)?;
                self.zext_i32(i1)?.into()
            }
            Pow => return Err("integer Pow should have been lowered to float by sema".into()),
            StrConcat | StrEq | StrNotEq | StrLt | StrGt | StrLtEq | StrGtEq => {
                unreachable!("string ops handled above")
            }
        };
        Ok(v)
    }

    fn lower_unop(&mut self, op: IrUnOp, operand: Reg) -> Result<BasicValueEnum<'ctx>, String> {
        let ot = self
            .info
            .type_of(operand)
            .cloned()
            .ok_or_else(|| format!("untyped unop operand {operand}"))?;
        let b = &self.cg.builder;
        let v: BasicValueEnum = match op {
            IrUnOp::Not => {
                let val = self.ival(operand)?;
                let zero = val.get_type().const_zero();
                let i1 = b
                    .build_int_compare(IntPredicate::EQ, val, zero, "")
                    .map_err(berr)?;
                self.zext_i32(i1)?.into()
            }
            IrUnOp::Neg => match ot {
                IrType::Float => b
                    .build_float_neg(self.fval(operand)?, "")
                    .map_err(berr)?
                    .into(),
                IrType::Long => b
                    .build_int_neg(self.ival(operand)?, "")
                    .map_err(berr)?
                    .into(),
                _ => {
                    let e = self.ext_int(self.ival(operand)?, &ot, 32)?;
                    self.cg.builder.build_int_neg(e, "").map_err(berr)?.into()
                }
            },
            IrUnOp::Abs => match ot {
                IrType::Float => self.call_fabs(self.fval(operand)?)?.into(),
                IrType::Long => self.call_abs(self.ival(operand)?, 64)?.into(),
                _ => {
                    let e = self.ext_int(self.ival(operand)?, &ot, 32)?;
                    self.call_abs(e, 32)?.into()
                }
            },
            IrUnOp::BinNot => match ot {
                IrType::Long => b.build_not(self.ival(operand)?, "").map_err(berr)?.into(),
                _ => {
                    let e = self.ext_int(self.ival(operand)?, &ot, 32)?;
                    self.cg.builder.build_not(e, "").map_err(berr)?.into()
                }
            },
        };
        Ok(v)
    }

    fn lower_convert(
        &mut self,
        value: Reg,
        from: &IrType,
        to: &IrType,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match to {
            IrType::Byte | IrType::Short | IrType::Int | IrType::Long => {
                let bits = int_bits(to);
                match from {
                    IrType::Float => {
                        // Round half-away-from-zero, saturate to i64, truncate to
                        // the target width — matching interp `value_to_int_cb`.
                        let rounded = self.call_round(self.fval(value)?)?;
                        let sat = self.call_fptosi_sat(rounded)?;
                        Ok(self.trunc_int(sat, bits)?.into())
                    }
                    IrType::Byte | IrType::Short | IrType::Int | IrType::Long => {
                        Ok(self.ext_int(self.ival(value)?, from, bits)?.into())
                    }
                    IrType::String => {
                        let l = self
                            .call_value(self.cg.rt_string_to_long(), &[self.pval(value)?.into()])?
                            .into_int_value();
                        Ok(self.trunc_int(l, bits)?.into())
                    }
                    other => Err(format!("convert from {other:?} to integer unsupported")),
                }
            }
            IrType::Float => match from {
                IrType::Float => Ok(self.regs[&value]),
                IrType::Byte | IrType::Short | IrType::Int | IrType::Long => {
                    let wide = self.ext_int(self.ival(value)?, from, 64)?;
                    Ok(self
                        .cg
                        .builder
                        .build_signed_int_to_float(wide, self.cg.ctx.f64_type(), "")
                        .map_err(berr)?
                        .into())
                }
                IrType::String => {
                    self.call_value(self.cg.rt_string_to_float(), &[self.pval(value)?.into()])
                }
                other => Err(format!("convert from {other:?} to Float unsupported")),
            },
            IrType::String => match from {
                IrType::Int => {
                    self.call_value(self.cg.rt_int_to_string(), &[self.ival(value)?.into()])
                }
                IrType::Byte | IrType::Short => {
                    // Byte/Short are unsigned: widen to i32 (matching interp's
                    // `*v as i32`) before the int→string formatting.
                    let w = self.ext_int(self.ival(value)?, from, 32)?;
                    self.call_value(self.cg.rt_int_to_string(), &[w.into()])
                }
                IrType::Long => {
                    self.call_value(self.cg.rt_long_to_string(), &[self.ival(value)?.into()])
                }
                IrType::Float => {
                    self.call_value(self.cg.rt_float_to_string(), &[self.fval(value)?.into()])
                }
                IrType::String => {
                    // A retained owned copy (the convert produces a fresh +1).
                    self.call_value(self.cg.rt_string_retain(), &[self.pval(value)?.into()])
                }
                other => Err(format!("convert from {other:?} to String unsupported")),
            },
            other => Err(format!("convert to {other:?} is out of Phase-1 scope")),
        }
    }

    fn lower_call(
        &mut self,
        inst: &Inst,
        callee: cb_ir::FuncId,
        args: &[Reg],
    ) -> Result<(), String> {
        let decl = &self.cg.program.func_table[callee.0 as usize];
        let sig = decl.sig.clone();
        if args.len() != sig.params.len() {
            return Err(format!(
                "call arity mismatch for {}: {} args vs {} params",
                self.cg.interner.resolve(decl.name),
                args.len(),
                sig.params.len()
            ));
        }
        let mut margs: Vec<BasicMetadataValueEnum<'ctx>> = Vec::with_capacity(args.len());
        for (arg, pty) in args.iter().zip(&sig.params) {
            margs.push(self.marshal_arg(*arg, pty)?);
        }
        let fv: FunctionValue<'ctx> = match &decl.kind {
            FuncKind::UserDefined { body_index } => self.cg.user_funcs[*body_index],
            FuncKind::Runtime { symbol } => self.cg.rt_catalog(symbol, &sig)?,
        };
        let cs = self.cg.builder.build_call(fv, &margs, "").map_err(berr)?;
        // Bind the result only when the callee actually returns a value: a Void
        // runtime call still carries a result reg in the IR — don't bind it.
        if !matches!(*sig.ret, IrType::Void) {
            let v = call_basic(cs)?;
            self.bind(inst, v);
        }
        Ok(())
    }

    /// Marshal one argument to the callee's declared param type. String args are
    /// borrowed (the callee retains into its own slot); numbers are width-cast.
    fn marshal_arg(&self, arg: Reg, pty: &IrType) -> Result<BasicMetadataValueEnum<'ctx>, String> {
        Ok(match pty {
            // String, a null reference, and an array handle are all borrowed
            // pointers (the callee retains/copies as it needs).
            IrType::String | IrType::Null | IrType::Array { .. } => self.pval(arg)?.into(),
            IrType::Float => self.fval(arg)?.into(),
            IrType::Byte | IrType::Short | IrType::Int | IrType::Long => {
                let aty = self
                    .info
                    .type_of(arg)
                    .cloned()
                    .ok_or_else(|| format!("untyped call arg {arg}"))?;
                self.ext_int(self.ival(arg)?, &aty, int_bits(pty))?.into()
            }
            other => return Err(format!("call argument of type {other:?} is out of scope")),
        })
    }

    // ── Terminators ─────────────────────────────────────────────────────

    fn lower_terminator(&self, term: Option<&Terminator>) -> Result<(), String> {
        let b = &self.cg.builder;
        match term {
            Some(Terminator::Goto(target)) => {
                b.build_unconditional_branch(self.blocks[target])
                    .map_err(berr)?;
            }
            Some(Terminator::BranchIf {
                cond,
                then_block,
                else_block,
            }) => {
                let c = self.ival(*cond)?;
                let zero = c.get_type().const_zero();
                let truthy = b
                    .build_int_compare(IntPredicate::NE, c, zero, "")
                    .map_err(berr)?;
                b.build_conditional_branch(
                    truthy,
                    self.blocks[then_block],
                    self.blocks[else_block],
                )
                .map_err(berr)?;
            }
            Some(Terminator::Return { value }) => {
                self.release_string_locals()?;
                match value {
                    Some(r) => {
                        let v = self.regs[r];
                        b.build_return(Some(&v as &dyn BasicValue)).map_err(berr)?;
                    }
                    None => {
                        if matches!(self.func.return_type, IrType::Void) {
                            b.build_return(None).map_err(berr)?;
                        } else {
                            // An implicit fall-through return in a value-returning
                            // function — an unreachable synthetic block. Return a
                            // deterministic zero of the return type.
                            let zero = self.default_value(&self.func.return_type)?;
                            b.build_return(Some(&zero as &dyn BasicValue))
                                .map_err(berr)?;
                        }
                    }
                }
            }
            Some(Terminator::Halt { code }) => {
                let c = self.cg.ctx.i32_type().const_int(*code as u64, false);
                self.call_void(self.cg.rt_exit(), &[c.into()])?;
                b.build_unreachable().map_err(berr)?;
            }
            Some(Terminator::Trap(_)) => {
                // Traps are out of Phase-1 scope; rather than miscompile to UB,
                // exit non-zero. (Seed fixtures never reach a Trap.)
                let one = self.cg.ctx.i32_type().const_int(1, false);
                self.call_void(self.cg.rt_exit(), &[one.into()])?;
                b.build_unreachable().map_err(berr)?;
            }
            None => return Err("IR block has no terminator".into()),
        }
        Ok(())
    }

    /// Release every String local slot (called before each `Return`).
    fn release_string_locals(&self) -> Result<(), String> {
        for (i, local) in self.func.locals.iter().enumerate() {
            if matches!(local.ty, IrType::String) {
                let v = self
                    .cg
                    .builder
                    .build_load(self.cg.ptr_t(), self.locals[i], "")
                    .map_err(berr)?;
                self.call_void(self.cg.rt_string_release(), &[v.into()])?;
            }
        }
        Ok(())
    }

    /// Release a String reg (its owned +1) at its last use.
    fn release_reg(&self, reg: Reg) -> Result<(), String> {
        let v = self.regs[&reg];
        self.call_void(self.cg.rt_string_release(), &[v.into()])
    }

    // ── Small helpers ───────────────────────────────────────────────────

    fn ival(&self, reg: Reg) -> Result<IntValue<'ctx>, String> {
        Ok(self
            .regs
            .get(&reg)
            .ok_or_else(|| format!("undefined reg {reg}"))?
            .into_int_value())
    }
    fn fval(&self, reg: Reg) -> Result<FloatValue<'ctx>, String> {
        Ok(self
            .regs
            .get(&reg)
            .ok_or_else(|| format!("undefined reg {reg}"))?
            .into_float_value())
    }
    fn pval(&self, reg: Reg) -> Result<PointerValue<'ctx>, String> {
        Ok(self
            .regs
            .get(&reg)
            .ok_or_else(|| format!("undefined reg {reg}"))?
            .into_pointer_value())
    }

    /// Zero-extend an `i1` predicate result to `i32` (CB booleans are Int 1/0).
    fn zext_i32(&self, i1: IntValue<'ctx>) -> Result<IntValue<'ctx>, String> {
        self.cg
            .builder
            .build_int_z_extend(i1, self.cg.ctx.i32_type(), "")
            .map_err(berr)
    }

    /// Extend/truncate `v` (whose IR type is `from`) to `to_bits`. Byte/Short are
    /// unsigned → zero-extend; Int/Long are signed → sign-extend; a narrower
    /// target truncates.
    fn ext_int(
        &self,
        v: IntValue<'ctx>,
        from: &IrType,
        to_bits: u32,
    ) -> Result<IntValue<'ctx>, String> {
        let from_bits = int_bits(from);
        if from_bits == to_bits {
            return Ok(v);
        }
        let target = self.int_type(to_bits);
        if from_bits < to_bits {
            if matches!(from, IrType::Byte | IrType::Short) {
                self.cg
                    .builder
                    .build_int_z_extend(v, target, "")
                    .map_err(berr)
            } else {
                self.cg
                    .builder
                    .build_int_s_extend(v, target, "")
                    .map_err(berr)
            }
        } else {
            self.cg
                .builder
                .build_int_truncate(v, target, "")
                .map_err(berr)
        }
    }

    fn trunc_int(&self, v: IntValue<'ctx>, to_bits: u32) -> Result<IntValue<'ctx>, String> {
        if to_bits >= 64 {
            return Ok(v);
        }
        self.cg
            .builder
            .build_int_truncate(v, self.int_type(to_bits), "")
            .map_err(berr)
    }

    /// Mask a shift count to the operand width (x86-style: `& (width-1)`),
    /// matching the interpreter.
    fn mask_shift(&self, count: IntValue<'ctx>, width: u32) -> Result<IntValue<'ctx>, String> {
        let mask = self.int_type(width).const_int((width - 1) as u64, false);
        self.cg.builder.build_and(count, mask, "").map_err(berr)
    }

    fn int_type(&self, bits: u32) -> inkwell::types::IntType<'ctx> {
        match bits {
            8 => self.cg.ctx.i8_type(),
            16 => self.cg.ctx.i16_type(),
            32 => self.cg.ctx.i32_type(),
            _ => self.cg.ctx.i64_type(),
        }
    }

    // ── Intrinsic + runtime call wrappers ───────────────────────────────

    fn call_value(
        &self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let cs = self.cg.builder.build_call(f, args, "").map_err(berr)?;
        call_basic(cs)
    }

    fn call_void(
        &self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Result<(), String> {
        self.cg.builder.build_call(f, args, "").map_err(berr)?;
        Ok(())
    }

    fn const_string(&self, s: &str) -> Result<BasicValueEnum<'ctx>, String> {
        let bytes = s.as_bytes();
        let arr_ty = self.cg.ctx.i8_type().array_type(bytes.len() as u32);
        let id = self.cg.next_str_id();
        let global = self
            .cg
            .module
            .add_global(arr_ty, None, &format!(".str{id}"));
        global.set_initializer(&self.cg.ctx.const_string(bytes, false));
        global.set_constant(true);
        global.set_linkage(inkwell::module::Linkage::Private);
        let ptr = global.as_pointer_value();
        let len = self.cg.ctx.i64_type().const_int(bytes.len() as u64, false);
        self.call_value(self.cg.rt_string_from_literal(), &[ptr.into(), len.into()])
    }

    fn intrinsic(
        &self,
        name: &str,
        types: &[inkwell::types::BasicTypeEnum<'ctx>],
    ) -> Result<FunctionValue<'ctx>, String> {
        let i = inkwell::intrinsics::Intrinsic::find(name)
            .ok_or_else(|| format!("LLVM intrinsic {name} not found"))?;
        i.get_declaration(&self.cg.module, types)
            .ok_or_else(|| format!("could not declare LLVM intrinsic {name}"))
    }

    fn call_pow(
        &self,
        l: FloatValue<'ctx>,
        r: FloatValue<'ctx>,
    ) -> Result<FloatValue<'ctx>, String> {
        let f = self.intrinsic("llvm.pow", &[self.cg.ctx.f64_type().into()])?;
        Ok(self
            .call_value(f, &[l.into(), r.into()])?
            .into_float_value())
    }

    fn call_round(&self, x: FloatValue<'ctx>) -> Result<FloatValue<'ctx>, String> {
        let f = self.intrinsic("llvm.round", &[self.cg.ctx.f64_type().into()])?;
        Ok(self.call_value(f, &[x.into()])?.into_float_value())
    }

    fn call_fabs(&self, x: FloatValue<'ctx>) -> Result<FloatValue<'ctx>, String> {
        let f = self.intrinsic("llvm.fabs", &[self.cg.ctx.f64_type().into()])?;
        Ok(self.call_value(f, &[x.into()])?.into_float_value())
    }

    fn call_abs(&self, x: IntValue<'ctx>, bits: u32) -> Result<IntValue<'ctx>, String> {
        let f = self.intrinsic("llvm.abs", &[self.int_type(bits).into()])?;
        let poison = self.cg.ctx.bool_type().const_zero();
        Ok(self
            .call_value(f, &[x.into(), poison.into()])?
            .into_int_value())
    }

    fn call_fptosi_sat(&self, x: FloatValue<'ctx>) -> Result<IntValue<'ctx>, String> {
        let f = self.intrinsic(
            "llvm.fptosi.sat",
            &[self.cg.ctx.i64_type().into(), self.cg.ctx.f64_type().into()],
        )?;
        Ok(self.call_value(f, &[x.into()])?.into_int_value())
    }
}

/// Array element layout: `(elem_size_bytes, elem_is_ref)` for the
/// `cb_rt_array_new` call. The size matches the element's LLVM type width (so a
/// plain element store/load writes exactly one slot); `elem_is_ref` flags
/// String elements, whose slots default to the empty sentinel and follow the
/// retain/release discipline. Non-Phase-2 element types fail loudly (Phase 3).
fn elem_layout(elem: &IrType) -> Result<(u64, bool), String> {
    Ok(match elem {
        IrType::Byte => (1, false),
        IrType::Short => (2, false),
        IrType::Int => (4, false),
        IrType::Long | IrType::Float => (8, false),
        IrType::String => (8, true),
        other => {
            return Err(format!(
                "array element type {other:?} is out of scope for the Phase-2 LLVM backend"
            ));
        }
    })
}

/// Number of bits for a scalar integer IR type.
fn int_bits(ty: &IrType) -> u32 {
    match ty {
        IrType::Byte => 8,
        IrType::Short => 16,
        IrType::Int => 32,
        IrType::Long => 64,
        _ => 32,
    }
}

/// Extract the basic value from a call site, erroring on a void call.
fn call_basic<'ctx>(
    cs: inkwell::values::CallSiteValue<'ctx>,
) -> Result<BasicValueEnum<'ctx>, String> {
    match cs.try_as_basic_value() {
        inkwell::values::ValueKind::Basic(v) => Ok(v),
        inkwell::values::ValueKind::Instruction(_) => {
            Err("expected a value-returning runtime call".into())
        }
    }
}
