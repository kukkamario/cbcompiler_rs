//! Debug-mode structural validation for the IR.
//!
//! Call `verify()` after lowering in debug builds and in all tests to catch
//! invariant violations early.

use std::collections::HashSet;

use crate::inst::{InstKind, PlaceRoot, Projection, Terminator};
use crate::{BlockId, FuncKind, Program, Reg};

/// Verify structural invariants of the IR program. Panics on violations.
///
/// # Block ordering & dominance contract
///
/// Blocks within a function must be **dense and index-aligned**:
/// `blocks[i].id == BlockId(i)`. Lowering guarantees this (`fresh_block`), and
/// the interpreter relies on it directly (`func.blocks[block_id.0 as usize]`).
/// Asserting it here subsumes the "entry block is `BlockId(0)`" and "no
/// duplicate `BlockId`" invariants in a single check.
///
/// Register definitions are validated in one forward pass over blocks in
/// vector order: a use is accepted only if a *previously visited* instruction
/// (or an earlier block) defined the register. This is sound **because lowering
/// emits blocks in a dominance-respecting (reverse-postorder) order**, so every
/// definition is visited before any use it dominates — including across loop
/// back-edges. The verifier documents and assumes this ordering rather than
/// computing a dominator tree; if a future pass starts producing non-ordered
/// blocks, replace this single-set pass with real dominance analysis.
pub fn verify(program: &Program) {
    let func_table_len = program.func_table.len() as u32;
    let num_type_defs = program.type_defs.len() as u32;
    let num_globals = program.globals.len() as u32;

    // Every user-defined function body must be referenced by exactly one
    // `FuncKind::UserDefined` declaration — the mapping is a bijection onto
    // `program.functions`. An out-of-range or duplicated `body_index` only
    // misbehaves at backend time, so catch it here.
    let num_bodies = program.functions.len();
    let mut body_referenced = vec![false; num_bodies];
    for decl in &program.func_table {
        if let FuncKind::UserDefined { body_index } = &decl.kind {
            assert!(
                *body_index < num_bodies,
                "body_index {body_index} out of range (program has {num_bodies} function bodies)",
            );
            assert!(
                !body_referenced[*body_index],
                "body_index {body_index} referenced by more than one function declaration",
            );
            body_referenced[*body_index] = true;
        }
    }
    assert!(
        body_referenced.iter().all(|&seen| seen),
        "not every function body is referenced by a UserDefined declaration (mapping is not a bijection)",
    );

    for func in &program.functions {
        let num_locals = func.locals.len() as u32;
        let num_blocks = func.blocks.len() as u32;

        assert!(
            !func.blocks.is_empty(),
            "function has no blocks (every function needs at least an entry block)",
        );
        for (i, block) in func.blocks.iter().enumerate() {
            assert!(
                block.id == BlockId(i as u32),
                "block at index {i} has id {} but blocks must be dense and index-aligned (id == index)",
                block.id,
            );
        }

        let mut defined_regs: HashSet<Reg> = HashSet::new();

        for block in &func.blocks {
            assert!(
                block.terminator.is_some(),
                "block {} in function has no terminator",
                block.id,
            );

            for inst in &block.insts {
                verify_inst_locals(&inst.kind, num_locals);
                // Check operand regs *before* defining the result, so an
                // instruction cannot satisfy use-before-def with its own result.
                verify_inst_regs(&inst.kind, &defined_regs);
                verify_inst_func_ids(&inst.kind, func_table_len);
                verify_inst_type_defs(&inst.kind, num_type_defs);
                verify_inst_globals(&inst.kind, num_globals);
                if let Some(r) = inst.result {
                    defined_regs.insert(r);
                }
            }

            if let Some(ref term) = block.terminator {
                verify_terminator_targets(term, num_blocks);
                verify_terminator_regs(term, &defined_regs);
            }
        }
    }
}

fn verify_inst_locals(kind: &InstKind, num_locals: u32) {
    let check = |local: crate::LocalId| {
        assert!(
            local.0 < num_locals,
            "local{} out of range (function has {num_locals} locals)",
            local.0,
        );
    };

    match kind {
        InstKind::LoadLocal { local } | InstKind::StoreLocal { local, .. } => check(*local),
        InstKind::DeleteLvalue { local } => check(*local),
        InstKind::Redim { local, .. } => check(*local),
        InstKind::StorePlace {
            root: PlaceRoot::Local(local),
            ..
        } => check(*local),
        _ => {}
    }
}

fn verify_inst_func_ids(kind: &InstKind, func_table_len: u32) {
    if let InstKind::Call { callee, .. } = kind {
        assert!(
            callee.0 < func_table_len,
            "FuncId({}) out of range (func_table has {func_table_len} entries)",
            callee.0,
        );
    }
}

fn verify_inst_type_defs(kind: &InstKind, num_type_defs: u32) {
    let check = |id: crate::TypeDefId| {
        assert!(
            id.0 < num_type_defs,
            "TypeDefId({}) out of range (program has {num_type_defs} type defs)",
            id.0,
        );
    };

    match kind {
        InstKind::NewType { type_def }
        | InstKind::First { type_def }
        | InstKind::Last { type_def } => {
            check(*type_def);
        }
        _ => {}
    }
}

fn verify_inst_globals(kind: &InstKind, num_globals: u32) {
    let check = |id: crate::GlobalId| {
        assert!(
            id.0 < num_globals,
            "GlobalId({}) out of range (program has {num_globals} globals)",
            id.0,
        );
    };

    match kind {
        InstKind::LoadGlobal { global }
        | InstKind::StoreGlobal { global, .. }
        | InstKind::DeleteLvalueGlobal { global }
        | InstKind::RedimGlobal { global, .. } => check(*global),
        InstKind::StorePlace {
            root: PlaceRoot::Global(global),
            ..
        } => check(*global),
        _ => {}
    }
}

fn verify_inst_regs(kind: &InstKind, defined: &HashSet<Reg>) {
    let check = |r: Reg| {
        assert!(defined.contains(&r), "register {r} used before definition");
    };

    match kind {
        InstKind::BinOp { lhs, rhs, .. } => {
            check(*lhs);
            check(*rhs);
        }
        InstKind::UnOp { operand, .. } => check(*operand),
        InstKind::StoreLocal { value, .. } | InstKind::StoreGlobal { value, .. } => check(*value),
        InstKind::GetField { object, .. } => check(*object),
        InstKind::GetElement { array, indices } => {
            check(*array);
            for idx in indices {
                check(*idx);
            }
        }
        InstKind::StorePlace { path, value, .. } => {
            for proj in path {
                if let Projection::Index(idxs) = proj {
                    for idx in idxs {
                        check(*idx);
                    }
                }
            }
            check(*value);
        }
        InstKind::NewArray { dims, .. } => {
            for d in dims {
                check(*d);
            }
        }
        InstKind::Next { object } | InstKind::Previous { object } => check(*object),
        InstKind::DeleteRvalue { value } => check(*value),
        InstKind::Len { array, dim } => {
            check(*array);
            if let Some(d) = dim {
                check(*d);
            }
        }
        InstKind::StrLen { s } => check(*s),
        InstKind::ConvertExplicit { value, .. } | InstKind::Convert { value, .. } => {
            check(*value);
        }
        InstKind::Call { args, .. } => {
            for a in args {
                check(*a);
            }
        }
        InstKind::CallIndirect { callee, args } => {
            check(*callee);
            for a in args {
                check(*a);
            }
        }
        InstKind::Redim { dims, .. } | InstKind::RedimGlobal { dims, .. } => {
            for d in dims {
                check(*d);
            }
        }
        InstKind::ArrayTotalLen { array } => check(*array),
        InstKind::GetElementFlat { array, index } => {
            check(*array);
            check(*index);
        }
        InstKind::LoadLocal { .. }
        | InstKind::LoadGlobal { .. }
        | InstKind::NewType { .. }
        | InstKind::First { .. }
        | InstKind::Last { .. }
        | InstKind::DeleteLvalue { .. }
        | InstKind::DeleteLvalueGlobal { .. }
        | InstKind::ConstInt(_)
        | InstKind::ConstLong(_)
        | InstKind::ConstFloat(_)
        | InstKind::ConstBool(_)
        | InstKind::ConstString(_)
        | InstKind::ConstNull => {}
    }
}

fn verify_terminator_targets(term: &Terminator, num_blocks: u32) {
    // Block ids are dense and index-aligned (see `verify`), so a target is
    // valid iff it is in range.
    let check = |id: BlockId| {
        assert!(id.0 < num_blocks, "terminator references non-existent {id}",);
    };

    match term {
        Terminator::Goto(target) => check(*target),
        Terminator::BranchIf {
            then_block,
            else_block,
            ..
        } => {
            check(*then_block);
            check(*else_block);
        }
        Terminator::Return { .. } | Terminator::Halt { .. } | Terminator::Trap(_) => {}
    }
}

fn verify_terminator_regs(term: &Terminator, defined: &HashSet<Reg>) {
    let check = |r: Reg| {
        assert!(defined.contains(&r), "register {r} used before definition");
    };

    match term {
        Terminator::BranchIf { cond, .. } => check(*cond),
        Terminator::Return { value: Some(r) } => check(*r),
        Terminator::Return { value: None }
        | Terminator::Goto(_)
        | Terminator::Halt { .. }
        | Terminator::Trap(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inst::{IrBinOp, TrapKind};
    use crate::types::{FnSig, IrType};
    use crate::{BasicBlock, FuncDecl, Function, Inst, Local, Program};
    use cb_diagnostics::source::FileId;
    use cb_diagnostics::{Span, Symbol};

    const DUMMY_SPAN: Span = Span::new(0, 0, FileId::SYNTHETIC);

    fn dummy_sym() -> Symbol {
        Symbol::DUMMY
    }

    /// A `UserDefined` declaration referencing body `body_index`. Required for
    /// the verifier's bijection check over function bodies.
    fn user_decl(body_index: usize) -> FuncDecl {
        FuncDecl {
            name: dummy_sym(),
            sig: FnSig {
                params: Vec::new(),
                ret: Box::new(IrType::Void),
            },
            kind: FuncKind::UserDefined { body_index },
        }
    }

    fn minimal_program(func: Function) -> Program {
        Program {
            func_table: vec![user_decl(0)],
            functions: vec![func],
            globals: Vec::new(),
            type_defs: Vec::new(),
            struct_defs: Vec::new(),
        }
    }

    fn valid_one_block(insts: Vec<Inst>, locals: Vec<Local>, term: Terminator) -> Program {
        minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals,
            blocks: vec![BasicBlock {
                id: BlockId(0),
                insts,
                terminator: Some(term),
                terminator_span: DUMMY_SPAN,
            }],
        })
    }

    #[test]
    fn valid_empty_function() {
        let prog = valid_one_block(vec![], vec![], Terminator::Return { value: None });
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "no terminator")]
    fn unterminated_block() {
        let prog = minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals: Vec::new(),
            blocks: vec![BasicBlock {
                id: BlockId(0),
                insts: Vec::new(),
                terminator: None,
                terminator_span: DUMMY_SPAN,
            }],
        });
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "used before definition")]
    fn use_undefined_register() {
        let prog = valid_one_block(
            vec![Inst {
                result: None,
                kind: InstKind::StoreLocal {
                    local: crate::LocalId(0),
                    value: Reg(99),
                },
                span: DUMMY_SPAN,
            }],
            vec![Local {
                name: dummy_sym(),
                ty: IrType::Int,
                is_param: false,
            }],
            Terminator::Return { value: None },
        );
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn local_out_of_range() {
        let prog = valid_one_block(
            vec![Inst {
                result: Some(Reg(0)),
                kind: InstKind::LoadLocal {
                    local: crate::LocalId(5),
                },
                span: DUMMY_SPAN,
            }],
            vec![],
            Terminator::Return { value: None },
        );
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "non-existent")]
    fn branch_to_nonexistent_block() {
        let prog = valid_one_block(
            vec![Inst {
                result: Some(Reg(0)),
                kind: InstKind::ConstBool(true),
                span: DUMMY_SPAN,
            }],
            vec![],
            Terminator::BranchIf {
                cond: Reg(0),
                then_block: BlockId(99),
                else_block: BlockId(0),
            },
        );
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "used before definition")]
    fn return_undefined_register() {
        let prog = valid_one_block(
            vec![],
            vec![],
            Terminator::Return {
                value: Some(Reg(0)),
            },
        );
        verify(&prog);
    }

    #[test]
    fn valid_two_blocks_with_branch() {
        let prog = minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals: Vec::new(),
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    insts: vec![Inst {
                        result: Some(Reg(0)),
                        kind: InstKind::ConstBool(true),
                        span: DUMMY_SPAN,
                    }],
                    terminator: Some(Terminator::BranchIf {
                        cond: Reg(0),
                        then_block: BlockId(1),
                        else_block: BlockId(1),
                    }),
                    terminator_span: DUMMY_SPAN,
                },
                BasicBlock {
                    id: BlockId(1),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Return { value: None }),
                    terminator_span: DUMMY_SPAN,
                },
            ],
        });
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "used before definition")]
    fn binop_with_undefined_operand() {
        let prog = valid_one_block(
            vec![
                Inst {
                    result: Some(Reg(0)),
                    kind: InstKind::ConstInt(1),
                    span: DUMMY_SPAN,
                },
                Inst {
                    result: Some(Reg(2)),
                    kind: InstKind::BinOp {
                        op: IrBinOp::Add,
                        lhs: Reg(0),
                        rhs: Reg(1),
                    },
                    span: DUMMY_SPAN,
                },
            ],
            vec![],
            Terminator::Return { value: None },
        );
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "FuncId(5) out of range")]
    fn call_with_out_of_range_func_id() {
        let prog = valid_one_block(
            vec![Inst {
                result: Some(Reg(0)),
                kind: InstKind::Call {
                    callee: crate::FuncId(5),
                    args: Vec::new(),
                },
                span: DUMMY_SPAN,
            }],
            vec![],
            Terminator::Return { value: None },
        );
        verify(&prog);
    }

    // ── Strong block-ID invariant (FD-023) ──────────────────────────────

    #[test]
    #[should_panic(expected = "has no blocks")]
    fn zero_block_function() {
        let prog = minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals: Vec::new(),
            blocks: Vec::new(),
        });
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "index-aligned")]
    fn duplicate_block_id() {
        // Two blocks both claiming BlockId(0): the block at index 1 violates
        // the dense index-alignment invariant.
        let prog = minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals: Vec::new(),
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Goto(BlockId(0))),
                    terminator_span: DUMMY_SPAN,
                },
                BasicBlock {
                    id: BlockId(0),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Return { value: None }),
                    terminator_span: DUMMY_SPAN,
                },
            ],
        });
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "blocks must be dense and index-aligned")]
    fn non_index_aligned_block_id() {
        let prog = minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals: Vec::new(),
            blocks: vec![BasicBlock {
                id: BlockId(7),
                insts: Vec::new(),
                terminator: Some(Terminator::Return { value: None }),
                terminator_span: DUMMY_SPAN,
            }],
        });
        verify(&prog);
    }

    // ── body_index / bijection checks (FD-023) ──────────────────────────

    #[test]
    #[should_panic(expected = "body_index 5 out of range")]
    fn body_index_out_of_range() {
        let prog = Program {
            func_table: vec![user_decl(5)],
            functions: vec![Function {
                name: dummy_sym(),
                params: Vec::new(),
                return_type: IrType::Void,
                locals: Vec::new(),
                blocks: vec![BasicBlock {
                    id: BlockId(0),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Return { value: None }),
                    terminator_span: DUMMY_SPAN,
                }],
            }],
            globals: Vec::new(),
            type_defs: Vec::new(),
            struct_defs: Vec::new(),
        };
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "not a bijection")]
    fn unreferenced_body() {
        // One function body but no declaration referencing it.
        let prog = Program {
            func_table: Vec::new(),
            functions: vec![Function {
                name: dummy_sym(),
                params: Vec::new(),
                return_type: IrType::Void,
                locals: Vec::new(),
                blocks: vec![BasicBlock {
                    id: BlockId(0),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Return { value: None }),
                    terminator_span: DUMMY_SPAN,
                }],
            }],
            globals: Vec::new(),
            type_defs: Vec::new(),
            struct_defs: Vec::new(),
        };
        verify(&prog);
    }

    #[test]
    #[should_panic(expected = "more than one function declaration")]
    fn duplicate_body_index() {
        let prog = Program {
            func_table: vec![user_decl(0), user_decl(0)],
            functions: vec![Function {
                name: dummy_sym(),
                params: Vec::new(),
                return_type: IrType::Void,
                locals: Vec::new(),
                blocks: vec![BasicBlock {
                    id: BlockId(0),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Return { value: None }),
                    terminator_span: DUMMY_SPAN,
                }],
            }],
            globals: Vec::new(),
            type_defs: Vec::new(),
            struct_defs: Vec::new(),
        };
        verify(&prog);
    }

    // ── Accept-cases for previously untested IR (FD-023) ─────────────────

    #[test]
    fn accept_halt_terminator() {
        let prog = valid_one_block(vec![], vec![], Terminator::Halt { code: 0 });
        verify(&prog);
    }

    #[test]
    fn accept_trap_terminator() {
        let prog = valid_one_block(vec![], vec![], Terminator::Trap(TrapKind::DivisionByZero));
        verify(&prog);
    }

    #[test]
    fn accept_consts_string_binop_and_convert() {
        let prog = valid_one_block(
            vec![
                Inst {
                    result: Some(Reg(0)),
                    kind: InstKind::ConstInt(1),
                    span: DUMMY_SPAN,
                },
                Inst {
                    result: Some(Reg(1)),
                    kind: InstKind::ConstLong(2),
                    span: DUMMY_SPAN,
                },
                Inst {
                    result: Some(Reg(2)),
                    kind: InstKind::ConstFloat(3.0),
                    span: DUMMY_SPAN,
                },
                Inst {
                    result: Some(Reg(3)),
                    kind: InstKind::ConstString("hi".to_string()),
                    span: DUMMY_SPAN,
                },
                Inst {
                    result: Some(Reg(4)),
                    kind: InstKind::ConstNull,
                    span: DUMMY_SPAN,
                },
                // String concatenation of two strings.
                Inst {
                    result: Some(Reg(5)),
                    kind: InstKind::BinOp {
                        op: IrBinOp::StrConcat,
                        lhs: Reg(3),
                        rhs: Reg(3),
                    },
                    span: DUMMY_SPAN,
                },
                // Comparison binop (result is Bool).
                Inst {
                    result: Some(Reg(6)),
                    kind: InstKind::BinOp {
                        op: IrBinOp::Lt,
                        lhs: Reg(0),
                        rhs: Reg(0),
                    },
                    span: DUMMY_SPAN,
                },
                // Numeric conversion.
                Inst {
                    result: Some(Reg(7)),
                    kind: InstKind::Convert {
                        value: Reg(0),
                        from: IrType::Int,
                        to: IrType::Float,
                    },
                    span: DUMMY_SPAN,
                },
            ],
            vec![],
            Terminator::Return { value: None },
        );
        verify(&prog);
    }

    #[test]
    fn accept_loop_with_back_edge() {
        // bb0: r0 = const_bool true; goto bb1
        // bb1: branch_if r0, bb1 (back-edge), bb2
        // bb2: return
        // Pins the documented contract: a def in an earlier block (bb0) is
        // visible to a use across a back-edge (bb1 -> bb1).
        let prog = minimal_program(Function {
            name: dummy_sym(),
            params: Vec::new(),
            return_type: IrType::Void,
            locals: Vec::new(),
            blocks: vec![
                BasicBlock {
                    id: BlockId(0),
                    insts: vec![Inst {
                        result: Some(Reg(0)),
                        kind: InstKind::ConstBool(true),
                        span: DUMMY_SPAN,
                    }],
                    terminator: Some(Terminator::Goto(BlockId(1))),
                    terminator_span: DUMMY_SPAN,
                },
                BasicBlock {
                    id: BlockId(1),
                    insts: Vec::new(),
                    terminator: Some(Terminator::BranchIf {
                        cond: Reg(0),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    }),
                    terminator_span: DUMMY_SPAN,
                },
                BasicBlock {
                    id: BlockId(2),
                    insts: Vec::new(),
                    terminator: Some(Terminator::Return { value: None }),
                    terminator_span: DUMMY_SPAN,
                },
            ],
        });
        verify(&prog);
    }
}
