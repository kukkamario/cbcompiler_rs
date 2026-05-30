//! Debug printer for the AST.
//!
//! Lives in `cb-frontend` (not `cb-driver`) so the variant `match` arms
//! are co-located with the AST definition. Every variant is named
//! explicitly — no `_ => {}` catch-alls — so adding a new variant to
//! [`crate::ast::Expr`], [`crate::ast::Stmt`], [`crate::ast::TypeExpr`],
//! or [`crate::ast::CaseArm`] forces an arm here at compile time.
//! That was the FD-005 regression: `Stmt::Delete` was added without
//! traversal support and silently fell through the driver's old catch-all.

use std::fmt;

use crate::ast::{Arena, CaseArm, Expr, NewKind, Node, NodeId, Stmt, TypeExpr};

/// Write a one-rooted AST subtree to `out`, indented by one level.
///
/// The driver prints a header ("Program (N top-level statements):") and
/// then calls this once per top-level [`NodeId`]. Output ends in a
/// newline.
pub fn debug_print(out: &mut dyn fmt::Write, arena: &Arena, root: NodeId) -> fmt::Result {
    print_node(out, arena, root, 1)
}

fn print_node(out: &mut dyn fmt::Write, arena: &Arena, id: NodeId, depth: usize) -> fmt::Result {
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
    writeln!(out, "{pad}{header} @ {}..{}", span.start, span.end)?;
    for child in children_of(node) {
        print_node(out, arena, child, depth + 1)?;
    }
    Ok(())
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
                // FD-004 #12: `Expr::Field`'s name is a bare `Span`, not a
                // child node, so there is nothing extra to traverse here.
                out.push(*target);
            }
            Expr::Paren { inner } => out.push(*inner),
            Expr::New(NewKind::Type(t)) => out.push(*t),
            Expr::New(NewKind::Array { elem, dims }) => {
                out.push(*elem);
                out.extend_from_slice(dims);
            }
            // Leaf expressions — no children.
            Expr::IntLit(_)
            | Expr::FloatLit(_)
            | Expr::BoolLit(_)
            | Expr::NullLit
            | Expr::StrLit { .. }
            | Expr::Ident { .. }
            | Expr::Error => {}
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
            Stmt::FieldDecl { ty: None, .. } => {}
            Stmt::Return { value: Some(v) } => out.push(*v),
            Stmt::Return { value: None } => {}
            Stmt::Include { path } => out.push(*path),
            Stmt::Delete { operand } => out.push(*operand),
            // Leaf statements — no children.
            Stmt::Goto { .. }
            | Stmt::Label { .. }
            | Stmt::Break { .. }
            | Stmt::Continue
            | Stmt::End
            | Stmt::Error => {}
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
            // Leaf type-exprs — no children.
            TypeExpr::Primitive { .. } | TypeExpr::Named { .. } | TypeExpr::Error => {}
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

fn expr_variant_name(e: &Expr) -> &'static str {
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

fn stmt_variant_name(s: &Stmt) -> &'static str {
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
        Stmt::End => "End",
        Stmt::Include { .. } => "Include",
        Stmt::Delete { .. } => "Delete",
        Stmt::Error => "Error",
    }
}

fn type_expr_variant_name(t: &TypeExpr) -> &'static str {
    match t {
        TypeExpr::Primitive { .. } => "Primitive",
        TypeExpr::Named { .. } => "Named",
        TypeExpr::Array { .. } => "Array",
        TypeExpr::FnPtr { .. } => "FnPtr",
        TypeExpr::Paren { .. } => "Paren",
        TypeExpr::Error => "Error",
    }
}

fn case_arm_variant_name(c: &CaseArm) -> &'static str {
    match c {
        CaseArm::Case { .. } => "Case",
        CaseArm::Default { .. } => "Default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::{FileId, Span};

    fn span(start: u32, end: u32) -> Span {
        Span::new(start, end, FileId(0))
    }

    #[test]
    fn prints_leaf_int_lit() {
        let mut arena = Arena::new();
        let id = arena.alloc(Node::Expr(Expr::IntLit(42)), span(0, 2));
        let mut buf = String::new();
        debug_print(&mut buf, &arena, id).unwrap();
        assert_eq!(buf, "  Expr::IntLit @ 0..2\n");
    }

    #[test]
    fn prints_binary_with_children() {
        let mut arena = Arena::new();
        let lhs = arena.alloc(Node::Expr(Expr::IntLit(1)), span(0, 1));
        let rhs = arena.alloc(Node::Expr(Expr::IntLit(2)), span(4, 5));
        let root = arena.alloc(
            Node::Expr(Expr::Binary {
                op: crate::ast::BinOp::Add,
                lhs,
                rhs,
            }),
            span(0, 5),
        );
        let mut buf = String::new();
        debug_print(&mut buf, &arena, root).unwrap();
        assert_eq!(
            buf,
            "  Expr::Binary @ 0..5\n    Expr::IntLit @ 0..1\n    Expr::IntLit @ 4..5\n"
        );
    }

    #[test]
    fn prints_delete_traverses_operand() {
        // FD-005 regression guard: `Stmt::Delete` must traverse its operand.
        let mut arena = Arena::new();
        let operand = arena.alloc(
            Node::Expr(Expr::Ident {
                name_span: span(7, 8),
                sigil: None,
            }),
            span(7, 8),
        );
        let id = arena.alloc(Node::Stmt(Stmt::Delete { operand }), span(0, 8));
        let mut buf = String::new();
        debug_print(&mut buf, &arena, id).unwrap();
        assert_eq!(buf, "  Stmt::Delete @ 0..8\n    Expr::Ident @ 7..8\n");
    }
}
