//! Per-function `Reg → IrType` derivation and String-temp lifetime info (FD-049).
//!
//! No such facility exists in the IR, so build one. Every Phase-1 instruction
//! has a statically derivable result type: consts → their type; loads → the
//! slot type; conversions → their target; comparisons / `StrLen` → `Int`;
//! `StrConcat` → `String`; arithmetic/bitwise `BinOp` → the common operand type
//! (Byte/Short widen to Int, matching the interpreter's `int_binop`); `Call` →
//! the callee signature's return type.
//!
//! Because results depend only on the defining instruction (and, for
//! `BinOp`/`UnOp`, on operand reg types that dominate the use), a monotone
//! fixpoint over all instructions converges — covering unreachable blocks too,
//! which still must be lowered to keep every LLVM block terminated.
//!
//! The pass additionally drives the String refcount discipline (FD-049 decision
//! B): it records which String regs are *consumed* (moved into a Store/Return)
//! and, for owned String temps that are neither consumed nor escape their
//! defining block, the point to `cb_rt_string_release` them — right after their
//! last in-block use.

use std::collections::{HashMap, HashSet};

use cb_ir::inst::{InstKind, IrBinOp, IrUnOp, Terminator};
use cb_ir::{BlockId, Function, IrType, Program, Reg};

/// Result of the per-function analysis.
pub struct RegInfo {
    /// Statically derived type of every result register.
    types: HashMap<Reg, IrType>,
    /// Owned String temps to release immediately after the instruction at
    /// `(block, inst_index)`. Keyed so the lowerer can drain it as it walks.
    pub releases: HashMap<(BlockId, usize), Vec<Reg>>,
}

impl RegInfo {
    /// The derived type of `reg`, if known.
    pub fn type_of(&self, reg: Reg) -> Option<&IrType> {
        self.types.get(&reg)
    }
}

/// Analyze one function: derive reg types and compute String-temp releases.
pub fn analyze(func: &Function, program: &Program) -> RegInfo {
    let types = derive_types(func, program);
    let releases = compute_releases(func, &types);
    RegInfo { types, releases }
}

/// Monotone fixpoint deriving every result reg's type.
fn derive_types(func: &Function, program: &Program) -> HashMap<Reg, IrType> {
    let mut types: HashMap<Reg, IrType> = HashMap::new();
    loop {
        let mut changed = false;
        for block in &func.blocks {
            for inst in &block.insts {
                let Some(result) = inst.result else { continue };
                if types.contains_key(&result) {
                    continue;
                }
                if let Some(ty) = result_type(&inst.kind, &types, func, program) {
                    types.insert(result, ty);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    types
}

/// Result type of one instruction, or `None` if an operand type is not yet
/// known (the fixpoint retries it on a later pass).
fn result_type(
    kind: &InstKind,
    types: &HashMap<Reg, IrType>,
    func: &Function,
    program: &Program,
) -> Option<IrType> {
    Some(match kind {
        InstKind::ConstInt(_) => IrType::Int,
        InstKind::ConstLong(_) => IrType::Long,
        InstKind::ConstFloat(_) => IrType::Float,
        InstKind::ConstString(_) => IrType::String,
        InstKind::ConstNull => IrType::Null,
        InstKind::LoadLocal { local } => func.locals[local.0 as usize].ty.clone(),
        InstKind::LoadGlobal { global } => program.globals[global.0 as usize].ty.clone(),
        InstKind::Convert { to, .. } => to.clone(),
        InstKind::ConvertExplicit { target, .. } => target.clone(),
        InstKind::StrLen { .. } => IrType::Int,
        InstKind::BinOp { op, lhs, .. } => binop_result(*op, types.get(lhs)?),
        InstKind::UnOp { op, operand } => unop_result(*op, types.get(operand)?),
        InstKind::Call { callee, .. } => (*program.func_table[callee.0 as usize].sig.ret).clone(),
        // ── Arrays (FD-049 Phase 2) ────────────────────────────────────
        // `Len`/`ArrayTotalLen` yield Int counts; `NewArray` yields the array
        // handle type; element reads yield the array's element type (resolved
        // once the array reg's type is known — the fixpoint retries otherwise).
        InstKind::Len { .. } | InstKind::ArrayTotalLen { .. } => IrType::Int,
        InstKind::NewArray { elem_type, dims } => IrType::Array {
            elem: Box::new(elem_type.clone()),
            rank: dims.len() as u8,
        },
        InstKind::GetElement { array, .. } | InstKind::GetElementFlat { array, .. } => {
            match types.get(array)? {
                IrType::Array { elem, .. } => (**elem).clone(),
                _ => return None,
            }
        }
        // ── User Types (FD-049 Phase 3a) ───────────────────────────────
        // `New`/`First`/`Last` yield a `TypeRef` to the named type; `GetField`
        // carries its result type in the IR; `Next`/`Previous` propagate the
        // operand's `TypeRef` (fixpoint-resolved like `GetElement`).
        InstKind::NewType { type_def }
        | InstKind::First { type_def }
        | InstKind::Last { type_def } => {
            IrType::TypeRef(program.type_defs[type_def.0 as usize].name)
        }
        InstKind::GetField { field_type, .. } => field_type.clone(),
        InstKind::Next { object } | InstKind::Previous { object } => types.get(object)?.clone(),
        // Out-of-scope producers carry no derivable scalar type here; the
        // lowerer rejects them when it actually encounters one.
        _ => return None,
    })
}

/// Result type of an arithmetic/bitwise/comparison binop given the lhs type,
/// matching the interpreter: comparisons and string relations yield `Int`,
/// `StrConcat` yields `String`, and arithmetic collapses Byte/Short to Int.
fn binop_result(op: IrBinOp, lhs: &IrType) -> IrType {
    use IrBinOp::*;
    match op {
        Eq | NotEq | Lt | Gt | LtEq | GtEq => IrType::Int,
        StrEq | StrNotEq | StrLt | StrGt | StrLtEq | StrGtEq => IrType::Int,
        StrConcat => IrType::String,
        // Arithmetic / bitwise / shift: the common operand type. `^` (Pow) is
        // always lowered to Float by sema, so Float flows through here.
        _ => match lhs {
            IrType::Float => IrType::Float,
            IrType::Long => IrType::Long,
            _ => IrType::Int,
        },
    }
}

/// Result type of a unary op given the operand type (mirrors `eval_unop`).
fn unop_result(op: IrUnOp, operand: &IrType) -> IrType {
    match op {
        // Logical NOT always yields Int 1/0.
        IrUnOp::Not => IrType::Int,
        // Neg/Abs/BinNot keep Float/Long; Byte/Short widen to Int.
        IrUnOp::Neg | IrUnOp::Abs | IrUnOp::BinNot => match operand {
            IrType::Float => IrType::Float,
            IrType::Long => IrType::Long,
            _ => IrType::Int,
        },
    }
}

/// Compute, for each owned String temp, the in-block instruction after which it
/// should be released (FD-049 decision B). A String reg is *consumed* if it is
/// moved into a Store or returned; consumed regs are never auto-released. An
/// unconsumed String reg whose every use is in its defining block is released
/// right after its last use there. A reg that escapes its block (or is dead) is
/// conservatively leaked — safe, and acceptable for Phase 1.
fn compute_releases(
    func: &Function,
    types: &HashMap<Reg, IrType>,
) -> HashMap<(BlockId, usize), Vec<Reg>> {
    // Consumed = moved into a slot (Store*) or returned.
    let mut consumed: HashSet<Reg> = HashSet::new();
    for block in &func.blocks {
        for inst in &block.insts {
            match &inst.kind {
                InstKind::StoreLocal { value, .. }
                | InstKind::StoreGlobal { value, .. }
                | InstKind::StorePlace { value, .. } => {
                    consumed.insert(*value);
                }
                _ => {}
            }
        }
        if let Some(Terminator::Return { value: Some(r) }) = &block.terminator {
            consumed.insert(*r);
        }
    }

    // Definition site of each result reg.
    let mut def_site: HashMap<Reg, BlockId> = HashMap::new();
    for block in &func.blocks {
        for inst in &block.insts {
            if let Some(r) = inst.result {
                def_site.insert(r, block.id);
            }
        }
    }

    // For each String reg, gather (block, inst_index) operand uses.
    let mut uses: HashMap<Reg, Vec<(BlockId, usize)>> = HashMap::new();
    for block in &func.blocks {
        for (i, inst) in block.insts.iter().enumerate() {
            for operand in operand_regs(&inst.kind) {
                if matches!(types.get(&operand), Some(IrType::String)) {
                    uses.entry(operand).or_default().push((block.id, i));
                }
            }
        }
    }

    let mut releases: HashMap<(BlockId, usize), Vec<Reg>> = HashMap::new();
    for (&reg, locs) in &uses {
        if consumed.contains(&reg) {
            continue;
        }
        let Some(&def_block) = def_site.get(&reg) else {
            continue;
        };
        // Release only when every use is intra-block; otherwise leak (safe).
        if locs.iter().any(|(b, _)| *b != def_block) {
            continue;
        }
        let last = locs.iter().map(|(_, i)| *i).max().unwrap();
        releases.entry((def_block, last)).or_default().push(reg);
    }
    releases
}

/// Reg operands of an instruction (the regs it *reads*). Covers the Phase-1
/// instruction set; out-of-scope kinds report no operands (their String regs,
/// if any, are simply leaked rather than released — never use-after-freed).
fn operand_regs(kind: &InstKind) -> Vec<Reg> {
    match kind {
        InstKind::BinOp { lhs, rhs, .. } => vec![*lhs, *rhs],
        InstKind::UnOp { operand, .. } => vec![*operand],
        InstKind::StoreLocal { value, .. } | InstKind::StoreGlobal { value, .. } => vec![*value],
        InstKind::Convert { value, .. } | InstKind::ConvertExplicit { value, .. } => vec![*value],
        InstKind::StrLen { s } => vec![*s],
        InstKind::Call { args, .. } => args.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cb_diagnostics::{Span, Symbol};
    use cb_ir::{BasicBlock, FnSig, FuncDecl, FuncKind, Inst, Local};

    fn span() -> Span {
        Span::new(0, 0, cb_diagnostics::FileId(0))
    }

    fn inst(result: Option<u32>, kind: InstKind) -> Inst {
        Inst {
            result: result.map(Reg),
            kind,
            span: span(),
        }
    }

    fn func_with(insts: Vec<Inst>, term: Terminator, locals: Vec<Local>) -> Function {
        Function {
            name: Symbol::DUMMY,
            params: vec![],
            return_type: IrType::Void,
            locals,
            blocks: vec![BasicBlock {
                id: BlockId(0),
                insts,
                terminator: Some(term),
                terminator_span: span(),
            }],
        }
    }

    fn empty_program() -> Program {
        Program {
            func_table: vec![],
            functions: vec![],
            globals: vec![],
            type_defs: vec![],
            struct_defs: vec![],
        }
    }

    #[test]
    fn derives_const_and_arithmetic_types() {
        // r0 = const_int 2; r1 = const_int 3; r2 = add r0, r1 (-> Int)
        let f = func_with(
            vec![
                inst(Some(0), InstKind::ConstInt(2)),
                inst(Some(1), InstKind::ConstInt(3)),
                inst(
                    Some(2),
                    InstKind::BinOp {
                        op: IrBinOp::Add,
                        lhs: Reg(0),
                        rhs: Reg(1),
                    },
                ),
            ],
            Terminator::Return { value: None },
            vec![],
        );
        let info = analyze(&f, &empty_program());
        assert_eq!(info.type_of(Reg(0)), Some(&IrType::Int));
        assert_eq!(info.type_of(Reg(2)), Some(&IrType::Int));
    }

    #[test]
    fn float_arithmetic_and_comparison() {
        // r0 = const_float; r1 = const_float; r2 = add (-> Float); r3 = lt (-> Int)
        let f = func_with(
            vec![
                inst(Some(0), InstKind::ConstFloat(1.0)),
                inst(Some(1), InstKind::ConstFloat(2.0)),
                inst(
                    Some(2),
                    InstKind::BinOp {
                        op: IrBinOp::Add,
                        lhs: Reg(0),
                        rhs: Reg(1),
                    },
                ),
                inst(
                    Some(3),
                    InstKind::BinOp {
                        op: IrBinOp::Lt,
                        lhs: Reg(0),
                        rhs: Reg(1),
                    },
                ),
            ],
            Terminator::Return { value: None },
            vec![],
        );
        let info = analyze(&f, &empty_program());
        assert_eq!(info.type_of(Reg(2)), Some(&IrType::Float));
        assert_eq!(info.type_of(Reg(3)), Some(&IrType::Int));
    }

    #[test]
    fn string_concat_temp_is_released_after_last_use() {
        // r0 = const_string; r1 = const_string; r2 = str_concat r0, r1;
        // call print, r2   (r2 consumed by no store/return -> released after use)
        let program = Program {
            func_table: vec![FuncDecl {
                name: Symbol::DUMMY,
                sig: FnSig {
                    params: vec![IrType::String],
                    ret: Box::new(IrType::Void),
                },
                kind: FuncKind::Runtime {
                    symbol: "cb_rt_print".to_string(),
                },
            }],
            functions: vec![],
            globals: vec![],
            type_defs: vec![],
            struct_defs: vec![],
        };
        let f = func_with(
            vec![
                inst(Some(0), InstKind::ConstString("a".into())),
                inst(Some(1), InstKind::ConstString("b".into())),
                inst(
                    Some(2),
                    InstKind::BinOp {
                        op: IrBinOp::StrConcat,
                        lhs: Reg(0),
                        rhs: Reg(1),
                    },
                ),
                inst(
                    None,
                    InstKind::Call {
                        callee: cb_ir::FuncId(0),
                        args: vec![Reg(2)],
                    },
                ),
            ],
            Terminator::Return { value: None },
            vec![],
        );
        let info = analyze(&f, &program);
        assert_eq!(info.type_of(Reg(2)), Some(&IrType::String));
        // r0, r1 last-used at the concat (index 2); r2 last-used at the call (index 3).
        let at_concat = info
            .releases
            .get(&(BlockId(0), 2))
            .cloned()
            .unwrap_or_default();
        assert!(at_concat.contains(&Reg(0)) && at_concat.contains(&Reg(1)));
        let at_call = info
            .releases
            .get(&(BlockId(0), 3))
            .cloned()
            .unwrap_or_default();
        assert!(at_call.contains(&Reg(2)));
    }

    #[test]
    fn returned_string_is_not_released() {
        // r0 = const_string; return r0  -> consumed, no release scheduled.
        let f = func_with(
            vec![inst(Some(0), InstKind::ConstString("x".into()))],
            Terminator::Return {
                value: Some(Reg(0)),
            },
            vec![],
        );
        let info = analyze(&f, &empty_program());
        assert!(info.releases.is_empty());
    }
}
