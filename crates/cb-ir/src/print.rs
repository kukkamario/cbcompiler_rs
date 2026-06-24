//! Text-format IR printer for `--dump-ir` output and snapshot tests.

use cb_diagnostics::Interner;

use crate::inst::{InstKind, IrBinOp, IrUnOp, PlaceRoot, Projection, Terminator};
use crate::types::IrType;
use crate::{FuncDecl, Function, Global, Program, TypeDefInfo};

/// Render the entire program as human-readable IR text.
pub fn print_program(program: &Program, interner: &Interner) -> String {
    let mut out = String::new();
    if !program.globals.is_empty() {
        print_globals(&mut out, &program.globals, interner);
        out.push('\n');
    }
    for (i, func) in program.functions.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        print_function(
            &mut out,
            func,
            &program.func_table,
            &program.type_defs,
            interner,
        );
    }
    out
}

fn print_globals(out: &mut String, globals: &[Global], interner: &Interner) {
    use std::fmt::Write;

    out.push_str("globals:\n");
    for (i, g) in globals.iter().enumerate() {
        let name = interner.resolve(g.name);
        let ty = format_type(&g.ty, interner);
        writeln!(out, "  global{i}: {name} ({ty})").unwrap();
    }
}

fn print_function(
    out: &mut String,
    func: &Function,
    func_table: &[FuncDecl],
    type_defs: &[TypeDefInfo],
    interner: &Interner,
) {
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
            print_inst_kind(out, &inst.kind, func_table, type_defs, interner);
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

fn print_inst_kind(
    out: &mut String,
    kind: &InstKind,
    func_table: &[FuncDecl],
    type_defs: &[TypeDefInfo],
    interner: &Interner,
) {
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
        InstKind::LoadGlobal { global } => {
            write!(out, "load_global {global}").unwrap();
        }
        InstKind::StoreGlobal { global, value } => {
            write!(out, "store_global {global}, {value}").unwrap();
        }
        InstKind::NewType { type_def } => {
            let name = type_defs
                .get(type_def.0 as usize)
                .map(|t| interner.resolve(t.name))
                .unwrap_or("<unknown_type>");
            write!(out, "new_type {name}").unwrap();
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
        InstKind::GetElement { array, indices } => {
            write!(out, "get_element {array}").unwrap();
            for idx in indices {
                write!(out, ", {idx}").unwrap();
            }
        }
        InstKind::StorePlace { root, path, value } => {
            match root {
                PlaceRoot::Local(l) => write!(out, "store_place {l}").unwrap(),
                PlaceRoot::Global(g) => write!(out, "store_place {g}").unwrap(),
            }
            for proj in path {
                match proj {
                    Projection::Field(f) => {
                        write!(out, ".{}", interner.resolve(*f)).unwrap();
                    }
                    Projection::Index(idxs) => {
                        write!(out, "[").unwrap();
                        for (i, idx) in idxs.iter().enumerate() {
                            if i > 0 {
                                write!(out, ", ").unwrap();
                            }
                            write!(out, "{idx}").unwrap();
                        }
                        write!(out, "]").unwrap();
                    }
                }
            }
            write!(out, ", {value}").unwrap();
        }
        InstKind::First { type_def } => {
            let name = type_defs
                .get(type_def.0 as usize)
                .map(|t| interner.resolve(t.name))
                .unwrap_or("<unknown_type>");
            write!(out, "first {name}").unwrap();
        }
        InstKind::Last { type_def } => {
            let name = type_defs
                .get(type_def.0 as usize)
                .map(|t| interner.resolve(t.name))
                .unwrap_or("<unknown_type>");
            write!(out, "last {name}").unwrap();
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
        InstKind::DeleteLvalueGlobal { global } => {
            write!(out, "delete_lvalue {global}").unwrap();
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
        InstKind::StrLen { s } => {
            write!(out, "strlen {s}").unwrap();
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
            let name = func_table
                .get(callee.0 as usize)
                .map(|d| interner.resolve(d.name))
                .unwrap_or("<unknown_func>");
            write!(out, "call {name}").unwrap();
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
        InstKind::FuncAddr { func } => {
            let name = func_table
                .get(func.0 as usize)
                .map(|d| interner.resolve(d.name))
                .unwrap_or("<unknown_func>");
            write!(out, "func_addr {name}").unwrap();
        }
        InstKind::ConstInt(v) => {
            write!(out, "const_int {v}").unwrap();
        }
        InstKind::ConstLong(v) => {
            write!(out, "const_long {v}").unwrap();
        }
        InstKind::ConstFloat(v) => {
            write!(out, "const_float {v}").unwrap();
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
        InstKind::RedimGlobal {
            global,
            elem_type,
            dims,
        } => {
            write!(out, "redim {global}, {}", format_type(elem_type, interner)).unwrap();
            for d in dims {
                write!(out, ", {d}").unwrap();
            }
        }
        InstKind::ArrayTotalLen { array } => {
            write!(out, "array_total_len {array}").unwrap();
        }
        InstKind::GetElementFlat { array, index } => {
            write!(out, "get_element_flat {array}, {index}").unwrap();
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
        Terminator::Halt { code } => {
            write!(out, "halt {code}").unwrap();
        }
        Terminator::Trap(kind) => {
            write!(out, "trap {}", kind.mnemonic()).unwrap();
        }
    }
}

fn format_binop(op: IrBinOp) -> &'static str {
    match op {
        IrBinOp::Add => "add",
        IrBinOp::Sub => "sub",
        IrBinOp::Mul => "mul",
        IrBinOp::Div => "div",
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
        IrUnOp::Abs => "abs",
        IrUnOp::Not => "not",
        IrUnOp::BinNot => "bin_not",
    }
}

fn format_type(ty: &IrType, interner: &Interner) -> String {
    match ty {
        IrType::Byte => "Byte".to_string(),
        IrType::Short => "Short".to_string(),
        IrType::Int => "Int".to_string(),
        IrType::Long => "Long".to_string(),
        IrType::Float => "Float".to_string(),
        IrType::String => "String".to_string(),
        IrType::Array { elem, rank } => {
            format!("Array<{}, {rank}>", format_type(elem, interner))
        }
        IrType::TypeRef(sym) => format!("TypeRef({})", interner.resolve(*sym)),
        IrType::StructVal(sym) => format!("StructVal({})", interner.resolve(*sym)),
        IrType::FnPtr(sig) => {
            let params: Vec<_> = sig
                .params
                .iter()
                .map(|p| format_type(p, interner))
                .collect();
            format!(
                "FnPtr({}) -> {}",
                params.join(", "),
                format_type(&sig.ret, interner)
            )
        }
        IrType::RuntimeType(name) => format!("RuntimeType({name})"),
        IrType::Null => "Null".to_string(),
        IrType::Void => "Void".to_string(),
    }
}
