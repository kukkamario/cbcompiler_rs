//! Debug-mode structural validation for the IR.
//!
//! Call `verify()` after lowering in debug builds and in all tests to catch
//! invariant violations early.

use std::collections::HashSet;

use crate::inst::{InstKind, PlaceRoot, Projection, Terminator};
use crate::{BlockId, Program, Reg};

/// Verify structural invariants of the IR program. Panics on violations.
pub fn verify(program: &Program) {
    let func_table_len = program.func_table.len() as u32;
    let num_type_defs = program.type_defs.len() as u32;
    let num_globals = program.globals.len() as u32;

    for func in &program.functions {
        let num_locals = func.locals.len() as u32;
        let block_ids: HashSet<BlockId> = func.blocks.iter().map(|b| b.id).collect();
        let mut defined_regs: HashSet<Reg> = HashSet::new();

        for block in &func.blocks {
            assert!(
                block.terminator.is_some(),
                "block {} in function has no terminator",
                block.id,
            );

            for inst in &block.insts {
                if let Some(r) = inst.result {
                    defined_regs.insert(r);
                }
                verify_inst_locals(&inst.kind, num_locals);
                verify_inst_regs(&inst.kind, &defined_regs);
                verify_inst_func_ids(&inst.kind, func_table_len);
                verify_inst_type_defs(&inst.kind, num_type_defs);
                verify_inst_globals(&inst.kind, num_globals);
            }

            if let Some(ref term) = block.terminator {
                verify_terminator_targets(term, &block_ids);
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
        InstKind::StorePlace { root: PlaceRoot::Local(local), .. } => check(*local),
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
        InstKind::NewType { type_def } | InstKind::First { type_def } | InstKind::Last { type_def } => {
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
        InstKind::StorePlace { root: PlaceRoot::Global(global), .. } => check(*global),
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

fn verify_terminator_targets(term: &Terminator, block_ids: &HashSet<BlockId>) {
    let check = |id: BlockId| {
        assert!(
            block_ids.contains(&id),
            "terminator references non-existent {id}",
        );
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
    use cb_diagnostics::{Span, Symbol};
    use cb_diagnostics::source::FileId;
    use crate::inst::IrBinOp;
    use crate::types::IrType;
    use crate::{BasicBlock, Function, Inst, Local, Program};

    const DUMMY_SPAN: Span = Span::new(0, 0, FileId::SYNTHETIC);

    fn dummy_sym() -> Symbol {
        Symbol::DUMMY
    }

    fn minimal_program(func: Function) -> Program {
        Program {
            func_table: Vec::new(),
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
            Terminator::Return { value: Some(Reg(0)) },
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
}
