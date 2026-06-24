//! Snapshot tests of the parser against hand-curated `.cb` fixtures.
//!
//! Each test reads a fixture from `tests/fixtures/`, parses it with
//! [`cb_frontend::parse`], runs a small pretty-printer over the resulting
//! AST, and asserts the rendered text via `insta::assert_snapshot!`. The
//! pretty-printer is intentionally close to (but slightly richer than) the
//! one in `cb-driver/src/main.rs`: it includes literal values, operator
//! names, and sigils so the snapshot pins meaningful structural changes.
//! Diagnostics (lexer + parser, sorted by primary span start) are appended
//! below an `--- DIAGNOSTICS ---` separator.
//!
//! See plan §D.2 for the per-fixture coverage rationale.

use std::fmt::Write as _;

use cb_diagnostics::{Diagnostic, Severity};
use cb_frontend::ast::{
    Arena, CaseArm, DimName, Expr, NewKind, Node, NodeId, Stmt, TypeDeclKind, TypeExpr,
};
use cb_frontend::span::FileId;
use cb_frontend::token::{Kw, Sigil, StrLitKind};
use cb_frontend::{BinOp, LexerOptions, UnOp, parse, tokenize};

fn snapshot_parser_fixture(name: &str) -> String {
    let path = format!("tests/fixtures/{name}.cb");
    let src = std::fs::read_to_string(&path).expect("fixture missing");
    let (tokens, lex_diags) = tokenize(&src, FileId(0), LexerOptions::default());
    let r = parse(&tokens, &src, FileId(0));

    let mut out = String::new();
    writeln!(out, "Program ({} top-level statements)", r.program.len()).unwrap();
    for &id in &r.program {
        print_node(&r.arena, id, 1, &mut out);
    }

    // Diagnostics (lexer + parser) sorted by primary span start for stable order.
    let mut all_diags: Vec<&Diagnostic> = lex_diags.iter().chain(r.diagnostics.iter()).collect();
    if !all_diags.is_empty() {
        all_diags.sort_by_key(|d| d.primary.span.start);
        out.push_str("\n--- DIAGNOSTICS ---\n");
        for d in &all_diags {
            let sev = severity_str(d.severity);
            writeln!(
                out,
                "{sev}[{}] {} @ {}..{}",
                d.code.map_or("----", |c| c.as_str()),
                d.message,
                d.primary.span.start,
                d.primary.span.end,
            )
            .unwrap();
        }
    }
    out
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "ERROR",
        Severity::Warning => "WARNING",
        Severity::Note => "NOTE",
        Severity::Help => "HELP",
    }
}

fn print_node(arena: &Arena, id: NodeId, depth: usize, out: &mut String) {
    let span = arena.span_of(id);
    let pad = "  ".repeat(depth);
    let node = &arena[id];
    let header = format_node_header(arena, node);
    writeln!(out, "{pad}{header} @ {}..{}", span.start, span.end).unwrap();
    for child in children_of(node) {
        print_node(arena, child, depth + 1, out);
    }
}

fn format_node_header(_arena: &Arena, node: &Node) -> String {
    match node {
        Node::Expr(e) => format!("Expr::{}", format_expr(e)),
        Node::Stmt(s) => format!("Stmt::{}", format_stmt(s)),
        Node::TypeExpr(t) => format!("TypeExpr::{}", format_type_expr(t)),
        Node::Param(p) => {
            let name = if p.name_span.is_some() {
                "named"
            } else {
                "<anon>"
            };
            let sigil = match p.sigil {
                Some(s) => format!(", sigil={}", sigil_str(s)),
                None => String::new(),
            };
            let dflt = if p.default.is_some() {
                ", has_default"
            } else {
                ""
            };
            format!("Param({name}{sigil}{dflt})")
        }
        Node::CaseArm(c) => format!("CaseArm::{}", case_arm_name(c)),
    }
}

fn format_expr(e: &Expr) -> String {
    match e {
        Expr::IntLit(v) => format!("IntLit({v})"),
        Expr::FloatLit(v) => format!("FloatLit({v:?})"),
        Expr::NullLit => "NullLit".to_string(),
        Expr::StrLit { value, kind } => {
            format!("StrLit({:?}, {})", value, str_kind_name(*kind))
        }
        Expr::Ident { sigil, .. } => {
            let s = match sigil {
                Some(s) => sigil_str(*s),
                None => "None",
            };
            format!("Ident(sigil={s})")
        }
        Expr::Unary { op, .. } => format!("UnOp({})", un_op_name(*op)),
        Expr::Binary { op, .. } => format!("BinOp({})", bin_op_name(*op)),
        Expr::Call { args, .. } => format!("Call(args={})", args.len()),
        Expr::Index { indices, .. } => format!("Index(rank={})", indices.len()),
        Expr::Field { .. } => "Field".to_string(),
        Expr::Paren { .. } => "Paren".to_string(),
        Expr::New(NewKind::Type(_)) => "New(Type)".to_string(),
        Expr::New(NewKind::Array { dims, .. }) => format!("New(Array, dims={})", dims.len()),
        Expr::Error => "Error".to_string(),
    }
}

fn format_stmt(s: &Stmt) -> String {
    match s {
        Stmt::Assign { .. } => "Assign".to_string(),
        Stmt::ExprStmt { .. } => "ExprStmt".to_string(),
        Stmt::VarDecl {
            is_global,
            names,
            ty,
            init,
        } => format!(
            "{}(names={}, has_ty={}, has_init={})",
            if *is_global { "Global" } else { "Dim" },
            names.len(),
            ty.is_some(),
            init.is_some(),
        ),
        Stmt::Const {
            is_global,
            name: DimName { sigil, .. },
            ty,
            ..
        } => format!(
            "Const(is_global={}, sigil={}, has_ty={})",
            is_global,
            sigil.map(sigil_str).unwrap_or("None"),
            ty.is_some(),
        ),
        Stmt::Redim { dims, .. } => format!("Redim(rank={})", dims.len()),
        Stmt::If {
            elseifs,
            else_body,
            form,
            ..
        } => format!(
            "If(form={}, elseifs={}, has_else={})",
            if_form_name(*form),
            elseifs.len(),
            else_body.is_some(),
        ),
        Stmt::While { .. } => "While".to_string(),
        Stmt::RepeatForever { .. } => "RepeatForever".to_string(),
        Stmt::RepeatWhile { .. } => "RepeatWhile".to_string(),
        Stmt::For {
            step, next_name, ..
        } => format!(
            "For(has_step={}, has_next_name={})",
            step.is_some(),
            next_name.is_some(),
        ),
        Stmt::ForEach { next_name, .. } => {
            format!("ForEach(has_next_name={})", next_name.is_some())
        }
        Stmt::Select { arms, .. } => format!("Select(arms={})", arms.len()),
        Stmt::Function {
            params,
            return_ty,
            return_sigil,
            ..
        } => format!(
            "Function(params={}, return_sigil={}, has_return_ty={})",
            params.len(),
            return_sigil.map(sigil_str).unwrap_or("None"),
            return_ty.is_some(),
        ),
        Stmt::TypeDecl { kind, fields, .. } => format!(
            "{}(fields={})",
            match kind {
                TypeDeclKind::Type => "Type",
                TypeDeclKind::Struct => "Struct",
            },
            fields.len()
        ),
        Stmt::FieldDecl {
            name: DimName { sigil, .. },
            ty,
        } => format!(
            "FieldDecl(sigil={}, has_ty={})",
            sigil.map(sigil_str).unwrap_or("None"),
            ty.is_some(),
        ),
        Stmt::Return { value } => format!("Return(has_value={})", value.is_some()),
        Stmt::Goto { .. } => "Goto".to_string(),
        Stmt::Label { .. } => "Label".to_string(),
        Stmt::Break { count } => match count {
            Some(n) => format!("Break(count={n})"),
            None => "Break".to_string(),
        },
        Stmt::Continue => "Continue".to_string(),
        Stmt::End => "End".to_string(),
        Stmt::Include { .. } => "Include".to_string(),
        Stmt::Delete { .. } => "Delete".to_string(),
        Stmt::Error => "Error".to_string(),
    }
}

fn format_type_expr(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Primitive { kw } => format!("Primitive({})", kw_name(*kw)),
        TypeExpr::Named { .. } => "Named".to_string(),
        TypeExpr::Array { rank, .. } => format!("Array(rank={rank})"),
        TypeExpr::FnPtr { params, ret } => {
            format!("FnPtr(params={}, has_ret={})", params.len(), ret.is_some())
        }
        TypeExpr::Paren { .. } => "Paren".to_string(),
        TypeExpr::Error => "Error".to_string(),
    }
}

fn case_arm_name(c: &CaseArm) -> &'static str {
    match c {
        CaseArm::Case { .. } => "Case",
        CaseArm::Default { .. } => "Default",
    }
}

fn bin_op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "Add",
        BinOp::Sub => "Sub",
        BinOp::Mul => "Mul",
        BinOp::Div => "Div",
        BinOp::Pow => "Pow",
        BinOp::Mod => "Mod",
        BinOp::BinAnd => "BinAnd",
        BinOp::BinOr => "BinOr",
        BinOp::BinXor => "BinXor",
        BinOp::Shl => "Shl",
        BinOp::Shr => "Shr",
        BinOp::Sar => "Sar",
        BinOp::Eq => "Eq",
        BinOp::NotEq => "NotEq",
        BinOp::Lt => "Lt",
        BinOp::Gt => "Gt",
        BinOp::LtEq => "LtEq",
        BinOp::GtEq => "GtEq",
        BinOp::And => "And",
        BinOp::Or => "Or",
        BinOp::Xor => "Xor",
    }
}

fn un_op_name(op: UnOp) -> &'static str {
    match op {
        UnOp::Plus => "Plus",
        UnOp::Neg => "Neg",
        UnOp::Not => "Not",
        UnOp::BinNot => "BinNot",
    }
}

fn sigil_str(s: Sigil) -> &'static str {
    match s {
        Sigil::Integer => "Integer",
        Sigil::Float => "Float",
        Sigil::String => "String",
    }
}

fn str_kind_name(k: StrLitKind) -> &'static str {
    match k {
        StrLitKind::Plain => "Plain",
        StrLitKind::Escaped => "Escaped",
        StrLitKind::Raw => "Raw",
    }
}

fn if_form_name(f: cb_frontend::IfForm) -> &'static str {
    match f {
        cb_frontend::IfForm::Block => "Block",
        cb_frontend::IfForm::SingleLine => "SingleLine",
    }
}

fn kw_name(kw: Kw) -> &'static str {
    kw.as_str()
}

fn children_of(node: &Node) -> Vec<NodeId> {
    let mut out = Vec::new();
    match node {
        Node::Expr(e) => match e {
            Expr::Unary { operand, .. } => out.push(*operand),
            Expr::Binary { lhs, rhs, .. } => {
                out.push(*lhs);
                out.push(*rhs);
            }
            Expr::Call { callee, args } => {
                out.push(*callee);
                out.extend_from_slice(args);
            }
            Expr::Index { array, indices } => {
                out.push(*array);
                out.extend_from_slice(indices);
            }
            Expr::Field { target, .. } => {
                // FD-004 #12: field name is a bare Span, not a child node.
                out.push(*target);
            }
            Expr::Paren { inner } => out.push(*inner),
            Expr::New(NewKind::Type(t)) => out.push(*t),
            Expr::New(NewKind::Array { elem, dims }) => {
                out.push(*elem);
                out.extend_from_slice(dims);
            }
            _ => {}
        },
        Node::Stmt(s) => match s {
            Stmt::Assign { target, value } => {
                out.push(*target);
                out.push(*value);
            }
            Stmt::ExprStmt { expr } => out.push(*expr),
            Stmt::VarDecl { ty, init, .. } => {
                if let Some(t) = ty {
                    out.push(*t);
                }
                if let Some(i) = init {
                    out.push(*i);
                }
            }
            Stmt::Const { ty, value, .. } => {
                if let Some(t) = ty {
                    out.push(*t);
                }
                out.push(*value);
            }
            Stmt::Redim {
                target,
                elem_ty,
                dims,
            } => {
                out.push(*target);
                out.push(*elem_ty);
                out.extend_from_slice(dims);
            }
            Stmt::If {
                cond,
                then_body,
                elseifs,
                else_body,
                ..
            } => {
                out.push(*cond);
                out.extend_from_slice(then_body);
                for ei in elseifs {
                    out.push(ei.cond);
                    out.extend_from_slice(&ei.body);
                }
                if let Some(eb) = else_body {
                    out.extend_from_slice(eb);
                }
            }
            Stmt::While { cond, body } => {
                out.push(*cond);
                out.extend_from_slice(body);
            }
            Stmt::RepeatForever { body } => out.extend_from_slice(body),
            Stmt::RepeatWhile { body, cond } => {
                out.extend_from_slice(body);
                out.push(*cond);
            }
            Stmt::For {
                var,
                from,
                to,
                step,
                body,
                ..
            } => {
                out.push(*var);
                out.push(*from);
                out.push(*to);
                if let Some(s) = step {
                    out.push(*s);
                }
                out.extend_from_slice(body);
            }
            Stmt::ForEach {
                var, source, body, ..
            } => {
                out.push(*var);
                out.push(*source);
                out.extend_from_slice(body);
            }
            Stmt::Select { scrutinee, arms } => {
                out.push(*scrutinee);
                out.extend_from_slice(arms);
            }
            Stmt::Function {
                params,
                return_ty,
                body,
                ..
            } => {
                out.extend_from_slice(params);
                if let Some(r) = return_ty {
                    out.push(*r);
                }
                out.extend_from_slice(body);
            }
            Stmt::TypeDecl { fields, .. } => {
                out.extend_from_slice(fields);
            }
            Stmt::FieldDecl { ty: Some(t), .. } => {
                out.push(*t);
            }
            Stmt::Return { value: Some(v) } => out.push(*v),
            Stmt::Include { path } => out.push(*path),
            Stmt::Delete { operand } => out.push(*operand),
            _ => {}
        },
        Node::TypeExpr(t) => match t {
            TypeExpr::Array { elem, .. } => out.push(*elem),
            TypeExpr::FnPtr { params, ret } => {
                out.extend_from_slice(params);
                if let Some(r) = ret {
                    out.push(*r);
                }
            }
            TypeExpr::Paren { inner } => out.push(*inner),
            _ => {}
        },
        Node::Param(p) => {
            if let Some(t) = p.ty {
                out.push(t);
            }
            if let Some(d) = p.default {
                out.push(d);
            }
        }
        Node::CaseArm(c) => match c {
            CaseArm::Case { values, body } => {
                out.extend_from_slice(values);
                out.extend_from_slice(body);
            }
            CaseArm::Default { body } => out.extend_from_slice(body),
        },
    }
    out
}

#[test]
fn parser_if() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_if"));
}

#[test]
fn parser_loops() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_loops"));
}

#[test]
fn parser_select() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_select"));
}

#[test]
fn parser_func() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_func"));
}

#[test]
fn parser_type_struct() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_type_struct"));
}

#[test]
fn parser_dim() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_dim"));
}

#[test]
fn parser_type_expr() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_type_expr"));
}

#[test]
fn parser_recovery() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_recovery"));
}

#[test]
fn parser_labels_goto() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_labels_goto"));
}

#[test]
fn parser_strings() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_strings"));
}

#[test]
fn parser_includes() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_includes"));
}

#[test]
fn parser_break_continue() {
    insta::assert_snapshot!(snapshot_parser_fixture("parser_break_continue"));
}

#[test]
fn continuation_multi_line() {
    insta::assert_snapshot!(snapshot_parser_fixture("continuation_multi_line"));
}

#[test]
fn single_line_if_empty() {
    insta::assert_snapshot!(snapshot_parser_fixture("single_line_if_empty"));
}

#[test]
fn implicit_decl_as() {
    insta::assert_snapshot!(snapshot_parser_fixture("implicit_decl_as"));
}

#[test]
fn next_with_sigil() {
    insta::assert_snapshot!(snapshot_parser_fixture("next_with_sigil"));
}

#[test]
fn redim_array_element_type() {
    insta::assert_snapshot!(snapshot_parser_fixture("redim_array_element_type"));
}

#[test]
fn select_duplicate_default() {
    insta::assert_snapshot!(snapshot_parser_fixture("select_duplicate_default"));
}

#[test]
fn case_comma_list() {
    insta::assert_snapshot!(snapshot_parser_fixture("case_comma_list"));
}

#[test]
fn delete_statement() {
    insta::assert_snapshot!(snapshot_parser_fixture("delete_statement"));
}

#[test]
fn continuation_multi_line_preserve_trivia() {
    // Verifies that `\` line continuations are transparent to the parser even
    // when the lexer emits them as tokens (preserve_trivia=true). Same source
    // as the default snapshot; differs from the default tokenizer run only in
    // that the Continuation tokens are present in the stream.
    insta::assert_snapshot!(
        "continuation_multi_line_preserve_trivia",
        snapshot_parser_fixture_preserve_trivia("continuation_multi_line")
    );
}

fn snapshot_parser_fixture_preserve_trivia(name: &str) -> String {
    let path = format!("tests/fixtures/{name}.cb");
    let src = std::fs::read_to_string(&path).expect("fixture missing");
    let opts = LexerOptions {
        preserve_trivia: true,
    };
    let (tokens, lex_diags) = tokenize(&src, FileId(0), opts);
    // Drop everything except significant tokens + Continuation, so the parser
    // is exercised with the exact mix the FD-004 #1 fix targets. We KEEP
    // Continuation tokens precisely so the cursor's skip path runs.
    let tokens: Vec<_> = tokens
        .into_iter()
        .filter(|t| {
            !matches!(
                t.kind,
                cb_frontend::TokenKind::Whitespace | cb_frontend::TokenKind::Comment(_)
            )
        })
        .collect();
    let r = parse(&tokens, &src, FileId(0));

    let mut out = String::new();
    writeln!(out, "Program ({} top-level statements)", r.program.len()).unwrap();
    for &id in &r.program {
        print_node(&r.arena, id, 1, &mut out);
    }

    let mut all_diags: Vec<&Diagnostic> = lex_diags.iter().chain(r.diagnostics.iter()).collect();
    if !all_diags.is_empty() {
        all_diags.sort_by_key(|d| d.primary.span.start);
        out.push_str("\n--- DIAGNOSTICS ---\n");
        for d in &all_diags {
            let sev = severity_str(d.severity);
            writeln!(
                out,
                "{sev}[{}] {} @ {}..{}",
                d.code.map_or("----", |c| c.as_str()),
                d.message,
                d.primary.span.start,
                d.primary.span.end,
            )
            .unwrap();
        }
    }
    out
}
