//! Debug-mode structural validation for the IR.
//!
//! Call `verify()` after lowering in debug builds and in all tests to catch
//! invariant violations early.

use std::collections::HashSet;

use crate::inst::{InstKind, Terminator};
use crate::{BlockId, Program, Reg};

/// Verify structural invariants of the IR program. Panics on violations.
pub fn verify(program: &Program) {
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
        InstKind::StoreLocal { value, .. } => check(*value),
        InstKind::GetField { object, .. } => check(*object),
        InstKind::SetField { object, value, .. } => {
            check(*object);
            check(*value);
        }
        InstKind::GetElement { array, indices } => {
            check(*array);
            for idx in indices {
                check(*idx);
            }
        }
        InstKind::SetElement {
            array,
            indices,
            value,
        } => {
            check(*array);
            for idx in indices {
                check(*idx);
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
        InstKind::Redim { dims, .. } => {
            for d in dims {
                check(*d);
            }
        }
        InstKind::LoadLocal { .. }
        | InstKind::NewType { .. }
        | InstKind::First { .. }
        | InstKind::Last { .. }
        | InstKind::DeleteLvalue { .. }
        | InstKind::ConstInt(_)
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
        Terminator::Return { .. } | Terminator::Trap(_) => {}
    }
}

fn verify_terminator_regs(term: &Terminator, defined: &HashSet<Reg>) {
    let check = |r: Reg| {
        assert!(defined.contains(&r), "register {r} used before definition");
    };

    match term {
        Terminator::BranchIf { cond, .. } => check(*cond),
        Terminator::Return { value: Some(r) } => check(*r),
        Terminator::Return { value: None } | Terminator::Goto(_) | Terminator::Trap(_) => {}
    }
}
