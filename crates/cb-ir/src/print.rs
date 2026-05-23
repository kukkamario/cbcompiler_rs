//! Text-format IR printer for `--dump-ir` output and snapshot tests.

use cb_diagnostics::Interner;

use crate::inst::{InstKind, IrBinOp, IrUnOp, Terminator, TrapKind};
use crate::types::IrType;
use crate::{Function, Program};

/// Render the entire program as human-readable IR text.
pub fn print_program(program: &Program, interner: &Interner) -> String {
    let mut out = String::new();
    for (i, func) in program.functions.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        print_function(&mut out, func, interner);
    }
    out
}

fn print_function(out: &mut String, func: &Function, interner: &Interner) {
    use std::fmt::Write;

    let name = interner.resolve(func.name);
    write!(out, "fn {name}(").unwrap();
    for (i, param) in func.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{}", format_type(param, interner)).unwrap();
    }
    writeln!(out, ") -> {} {{", format_type(&func.return_type, interner)).unwrap();

    if !func.locals.is_empty() {
        out.push_str("  locals:\n");
        for (i, local) in func.locals.iter().enumerate() {
            let name = interner.resolve(local.name);
            let ty = format_type(&local.ty, interner);
            let mut flags = Vec::new();
            if local.is_global {
                flags.push("global");
            }
            if local.is_param {
                flags.push("param");
            }
            if flags.is_empty() {
                writeln!(out, "    local{i}: {name} ({ty})").unwrap();
            } else {
                writeln!(out, "    local{i}: {name} ({ty}, {})", flags.join(", ")).unwrap();
            }
        }
        out.push('\n');
    }

    for block in &func.blocks {
        writeln!(out, "  {}:", block.id).unwrap();
        for inst in &block.insts {
            out.push_str("    ");
            if let Some(r) = inst.result {
                write!(out, "{r} = ").unwrap();
            }
            print_inst_kind(out, &inst.kind, interner);
            out.push('\n');
        }
        out.push_str("    ");
        match &block.terminator {
            Some(term) => print_terminator(out, term),
            None => out.push_str("<no terminator>"),
        }
        out.push('\n');
    }

    out.push_str("}\n");
}

fn print_inst_kind(out: &mut String, kind: &InstKind, interner: &Interner) {
    use std::fmt::Write;

    match kind {
        InstKind::BinOp { op, lhs, rhs } => {
            write!(out, "{} {lhs}, {rhs}", format_binop(*op)).unwrap();
        }
        InstKind::UnOp { op, operand } => {
            write!(out, "{} {operand}", format_unop(*op)).unwrap();
        }
        InstKind::LoadLocal { local } => {
            write!(out, "load_local {local}").unwrap();
        }
        InstKind::StoreLocal { local, value } => {
            write!(out, "store_local {local}, {value}").unwrap();
        }
        InstKind::NewType { type_name } => {
            write!(out, "new_type {}", interner.resolve(*type_name)).unwrap();
        }
        InstKind::NewArray { elem_type, dims } => {
            write!(out, "new_array {}", format_type(elem_type, interner)).unwrap();
            for d in dims {
                write!(out, ", {d}").unwrap();
            }
        }
        InstKind::GetField {
            object,
            field,
            field_type,
        } => {
            write!(
                out,
                "get_field {object}, {} ({})",
                interner.resolve(*field),
                format_type(field_type, interner)
            )
            .unwrap();
        }
        InstKind::SetField {
            object,
            field,
            value,
        } => {
            write!(
                out,
                "set_field {object}, {}, {value}",
                interner.resolve(*field)
            )
            .unwrap();
        }
        InstKind::GetElement { array, indices } => {
            write!(out, "get_element {array}").unwrap();
            for idx in indices {
                write!(out, ", {idx}").unwrap();
            }
        }
        InstKind::SetElement {
            array,
            indices,
            value,
        } => {
            write!(out, "set_element {array}").unwrap();
            for idx in indices {
                write!(out, ", {idx}").unwrap();
            }
            write!(out, ", {value}").unwrap();
        }
        InstKind::First { type_name } => {
            write!(out, "first {}", interner.resolve(*type_name)).unwrap();
        }
        InstKind::Last { type_name } => {
            write!(out, "last {}", interner.resolve(*type_name)).unwrap();
        }
        InstKind::Next { object } => {
            write!(out, "next {object}").unwrap();
        }
        InstKind::Previous { object } => {
            write!(out, "previous {object}").unwrap();
        }
        InstKind::DeleteLvalue { local } => {
            write!(out, "delete_lvalue {local}").unwrap();
        }
        InstKind::DeleteRvalue { value } => {
            write!(out, "delete_rvalue {value}").unwrap();
        }
        InstKind::Len { array, dim } => {
            write!(out, "len {array}").unwrap();
            if let Some(d) = dim {
                write!(out, ", {d}").unwrap();
            }
        }
        InstKind::ConvertExplicit { value, target } => {
            write!(
                out,
                "convert_explicit {value} -> {}",
                format_type(target, interner)
            )
            .unwrap();
        }
        InstKind::Convert { value, from, to } => {
            write!(
                out,
                "convert {value}: {} -> {}",
                format_type(from, interner),
                format_type(to, interner)
            )
            .unwrap();
        }
        InstKind::Call { callee, args } => {
            write!(out, "call {}", interner.resolve(*callee)).unwrap();
            for arg in args {
                write!(out, ", {arg}").unwrap();
            }
        }
        InstKind::CallIndirect { callee, args } => {
            write!(out, "call_indirect {callee}").unwrap();
            for arg in args {
                write!(out, ", {arg}").unwrap();
            }
        }
        InstKind::ConstInt(v) => {
            write!(out, "const_int {v}").unwrap();
        }
        InstKind::ConstFloat(v) => {
            write!(out, "const_float {v}").unwrap();
        }
        InstKind::ConstBool(v) => {
            write!(out, "const_bool {v}").unwrap();
        }
        InstKind::ConstString(v) => {
            write!(out, "const_string {v:?}").unwrap();
        }
        InstKind::ConstNull => {
            out.push_str("const_null");
        }
        InstKind::Redim {
            local,
            elem_type,
            dims,
        } => {
            write!(out, "redim {local}, {}", format_type(elem_type, interner)).unwrap();
            for d in dims {
                write!(out, ", {d}").unwrap();
            }
        }
    }
}

fn print_terminator(out: &mut String, term: &Terminator) {
    use std::fmt::Write;

    match term {
        Terminator::Goto(block) => {
            write!(out, "goto {block}").unwrap();
        }
        Terminator::BranchIf {
            cond,
            then_block,
            else_block,
        } => {
            write!(out, "branch_if {cond}, {then_block}, {else_block}").unwrap();
        }
        Terminator::Return { value: Some(r) } => {
            write!(out, "return {r}").unwrap();
        }
        Terminator::Return { value: None } => {
            out.push_str("return");
        }
        Terminator::Trap(kind) => {
            let name = match kind {
                TrapKind::NullDeref => "null_deref",
                TrapKind::DeletedAccess => "deleted_access",
                TrapKind::DivisionByZero => "division_by_zero",
                TrapKind::IndexOutOfBounds => "index_out_of_bounds",
                TrapKind::NullFnPtr => "null_fn_ptr",
                TrapKind::DoubleDelete => "double_delete",
            };
            write!(out, "trap {name}").unwrap();
        }
    }
}

fn format_binop(op: IrBinOp) -> &'static str {
    match op {
        IrBinOp::Add => "add",
        IrBinOp::Sub => "sub",
        IrBinOp::Mul => "mul",
        IrBinOp::Div => "div",
        IrBinOp::IntDiv => "int_div",
        IrBinOp::Mod => "mod",
        IrBinOp::Pow => "pow",
        IrBinOp::BinAnd => "bin_and",
        IrBinOp::BinOr => "bin_or",
        IrBinOp::BinXor => "bin_xor",
        IrBinOp::Shl => "shl",
        IrBinOp::Shr => "shr",
        IrBinOp::Sar => "sar",
        IrBinOp::Eq => "eq",
        IrBinOp::NotEq => "not_eq",
        IrBinOp::Lt => "lt",
        IrBinOp::Gt => "gt",
        IrBinOp::LtEq => "lt_eq",
        IrBinOp::GtEq => "gt_eq",
        IrBinOp::StrConcat => "str_concat",
        IrBinOp::StrEq => "str_eq",
        IrBinOp::StrNotEq => "str_not_eq",
        IrBinOp::StrLt => "str_lt",
        IrBinOp::StrGt => "str_gt",
        IrBinOp::StrLtEq => "str_lt_eq",
        IrBinOp::StrGtEq => "str_gt_eq",
    }
}

fn format_unop(op: IrUnOp) -> &'static str {
    match op {
        IrUnOp::Neg => "neg",
        IrUnOp::Plus => "plus",
        IrUnOp::Not => "not",
        IrUnOp::BinNot => "bin_not",
    }
}

fn format_type(ty: &IrType, interner: &Interner) -> String {
    match ty {
        IrType::Byte => "Byte".to_string(),
        IrType::Short => "Short".to_string(),
        IrType::Int => "Int".to_string(),
        IrType::UInt => "UInt".to_string(),
        IrType::Long => "Long".to_string(),
        IrType::ULong => "ULong".to_string(),
        IrType::Float => "Float".to_string(),
        IrType::Bool => "Bool".to_string(),
        IrType::String => "String".to_string(),
        IrType::Array { elem, rank } => {
            format!("Array<{}, {rank}>", format_type(elem, interner))
        }
        IrType::TypeRef(sym) => format!("TypeRef({})", interner.resolve(*sym)),
        IrType::StructVal(sym) => format!("StructVal({})", interner.resolve(*sym)),
        IrType::FnPtr(sig) => {
            let params: Vec<_> = sig.params.iter().map(|p| format_type(p, interner)).collect();
            format!(
                "FnPtr({}) -> {}",
                params.join(", "),
                format_type(&sig.ret, interner)
            )
        }
        IrType::Null => "Null".to_string(),
        IrType::Void => "Void".to_string(),
    }
}
