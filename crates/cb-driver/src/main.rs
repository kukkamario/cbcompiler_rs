//! `cb` — CoolBasic compiler driver.
//!
//! End-to-end smoke driver: tokenize + parse a single `.cb` file, render any
//! diagnostics to stderr, print a debug view of the AST to stdout, and exit
//! non-zero if any error-severity diagnostics were emitted. Codegen and
//! backend selection arrive later — see FD-002 plan §E.

use std::path::PathBuf;
use std::process::ExitCode;

use cb_diagnostics::{CliRenderer, Renderer, Severity, SourceMap};
use cb_frontend::ast::{Arena, Node, NodeId};
use cb_frontend::parser::ParseResult;
use cb_frontend::{LexerOptions, parse, tokenize};
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path_arg) = args.next() else {
        eprintln!("usage: cb <file.cb>");
        return ExitCode::from(2);
    };
    let path = PathBuf::from(&path_arg);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cb: failed to read {}: {}", path.display(), e);
            return ExitCode::from(2);
        }
    };

    let mut sources = SourceMap::new();
    let file = sources.add(path.display().to_string(), text.clone());

    let (tokens, lex_diags) = tokenize(&text, file, LexerOptions::default());
    let ParseResult {
        arena,
        program,
        diagnostics: parse_diags,
    } = parse(&tokens, &text, file);

    let mut stderr = CliRenderer::new(StandardStream::stderr(ColorChoice::Auto));
    let mut had_error = false;
    for d in lex_diags.iter().chain(parse_diags.iter()) {
        if matches!(d.severity, Severity::Error) {
            had_error = true;
        }
        stderr.emit(d, &sources);
    }

    println!("Program ({} top-level statements):", program.len());
    for &id in &program {
        print_node(&arena, id, 1);
    }

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_node(arena: &Arena, id: NodeId, depth: usize) {
    let span = arena.span_of(id);
    let pad = "  ".repeat(depth);
    let node = &arena[id];
    let header = match node {
        Node::Expr(e) => format!("Expr::{}", expr_variant_name(e)),
        Node::Stmt(s) => format!("Stmt::{}", stmt_variant_name(s)),
        Node::TypeExpr(t) => format!("TypeExpr::{}", type_expr_variant_name(t)),
        Node::Param(_) => "Param".to_string(),
        Node::CaseArm(c) => format!("CaseArm::{}", case_arm_variant_name(c)),
    };
    println!("{pad}{header} @ {}..{}", span.start, span.end);
    for child in children_of(node) {
        print_node(arena, child, depth + 1);
    }
}

fn children_of(node: &Node) -> Vec<NodeId> {
    use cb_frontend::ast::{CaseArm, Expr, NewKind, Stmt, TypeExpr};
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
            Expr::Field { target, name } => {
                out.push(*target);
                out.push(*name);
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
            Stmt::Dim { ty, init, .. } | Stmt::Global { ty, init, .. } => {
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
            Stmt::Type { fields, .. } | Stmt::Struct { fields, .. } => {
                out.extend_from_slice(fields);
            }
            Stmt::FieldDecl { ty: Some(t), .. } => {
                out.push(*t);
            }
            Stmt::Return { value: Some(v) } => out.push(*v),
            Stmt::Include { path } => out.push(*path),
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

fn expr_variant_name(e: &cb_frontend::ast::Expr) -> &'static str {
    use cb_frontend::ast::Expr;
    match e {
        Expr::IntLit(_) => "IntLit",
        Expr::FloatLit(_) => "FloatLit",
        Expr::BoolLit(_) => "BoolLit",
        Expr::NullLit => "NullLit",
        Expr::StrLit { .. } => "StrLit",
        Expr::Ident { .. } => "Ident",
        Expr::Unary { .. } => "Unary",
        Expr::Binary { .. } => "Binary",
        Expr::Call { .. } => "Call",
        Expr::Index { .. } => "Index",
        Expr::Field { .. } => "Field",
        Expr::Paren { .. } => "Paren",
        Expr::New(_) => "New",
        Expr::Error => "Error",
    }
}

fn stmt_variant_name(s: &cb_frontend::ast::Stmt) -> &'static str {
    use cb_frontend::ast::Stmt;
    match s {
        Stmt::Assign { .. } => "Assign",
        Stmt::ExprStmt { .. } => "ExprStmt",
        Stmt::Dim { .. } => "Dim",
        Stmt::Global { .. } => "Global",
        Stmt::Const { .. } => "Const",
        Stmt::Redim { .. } => "Redim",
        Stmt::If { .. } => "If",
        Stmt::While { .. } => "While",
        Stmt::RepeatForever { .. } => "RepeatForever",
        Stmt::RepeatWhile { .. } => "RepeatWhile",
        Stmt::For { .. } => "For",
        Stmt::ForEach { .. } => "ForEach",
        Stmt::Select { .. } => "Select",
        Stmt::Function { .. } => "Function",
        Stmt::Type { .. } => "Type",
        Stmt::Struct { .. } => "Struct",
        Stmt::FieldDecl { .. } => "FieldDecl",
        Stmt::Return { .. } => "Return",
        Stmt::Goto { .. } => "Goto",
        Stmt::Label { .. } => "Label",
        Stmt::Break { .. } => "Break",
        Stmt::Continue => "Continue",
        Stmt::Include { .. } => "Include",
        Stmt::Error => "Error",
    }
}

fn type_expr_variant_name(t: &cb_frontend::ast::TypeExpr) -> &'static str {
    use cb_frontend::ast::TypeExpr;
    match t {
        TypeExpr::Primitive { .. } => "Primitive",
        TypeExpr::Named { .. } => "Named",
        TypeExpr::Array { .. } => "Array",
        TypeExpr::FnPtr { .. } => "FnPtr",
        TypeExpr::Paren { .. } => "Paren",
        TypeExpr::Error => "Error",
    }
}

fn case_arm_variant_name(c: &cb_frontend::ast::CaseArm) -> &'static str {
    use cb_frontend::ast::CaseArm;
    match c {
        CaseArm::Case { .. } => "Case",
        CaseArm::Default { .. } => "Default",
    }
}
