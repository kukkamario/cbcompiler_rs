//! Type representation for CoolBasic's type system.

use cb_diagnostics::{Interner, Symbol};
use cb_frontend::ast::{Node, TypeExpr};
use cb_frontend::{Arena, BinOp, Kw, NodeId, Sigil, UnOp};

/// Resolved type of a CoolBasic expression or variable.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    // Primitives (§3.1)
    Byte,
    Short,
    Int,
    UInt,
    Long,
    ULong,
    Float,
    Bool,
    String,

    // Composite
    Array { elem: Box<Type>, rank: u8 },
    TypeRef { name: Symbol },
    StructVal { name: Symbol },
    FnPtr { params: Vec<Type>, ret: Option<Box<Type>> },

    // Special
    Null,
    Void,
    /// Propagated from parse errors; suppresses cascading diagnostics.
    Error,
}

impl Type {
    pub fn is_error(&self) -> bool {
        matches!(self, Type::Error)
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Type::Byte
                | Type::Short
                | Type::Int
                | Type::UInt
                | Type::Long
                | Type::ULong
                | Type::Float
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Type::Byte | Type::Short | Type::Int | Type::UInt | Type::Long | Type::ULong
        )
    }

    pub fn is_reference(&self) -> bool {
        matches!(self, Type::TypeRef { .. } | Type::Array { .. } | Type::FnPtr { .. })
    }
}

/// Map a sigil to its locked type.
pub fn sigil_to_type(sigil: Sigil) -> Type {
    match sigil {
        Sigil::Integer => Type::Int,
        Sigil::Float => Type::Float,
        Sigil::String => Type::String,
        Sigil::Bool => Type::Bool,
    }
}

/// Map a type keyword to its `Type`.
pub fn kw_to_type(kw: Kw) -> Option<Type> {
    match kw {
        Kw::Byte => Some(Type::Byte),
        Kw::Short => Some(Type::Short),
        Kw::Int | Kw::Integer => Some(Type::Int),
        Kw::UInt | Kw::UInteger => Some(Type::UInt),
        Kw::Long => Some(Type::Long),
        Kw::ULong => Some(Type::ULong),
        Kw::Float => Some(Type::Float),
        Kw::Bool => Some(Type::Bool),
        Kw::String => Some(Type::String),
        _ => None,
    }
}

/// Resolve a `TypeExpr` AST node to a semantic `Type`.
///
/// Named types are interned but NOT resolved against the symbol table here —
/// that happens during pass 2 when all declarations are known. For now we
/// return `TypeRef` for any `TypeExpr::Named`, which pass 2 may refine to
/// `StructVal` once it knows the declaration kind.
pub fn resolve_type_expr(
    arena: &Arena,
    id: NodeId,
    interner: &mut Interner,
    source: &str,
) -> Type {
    use cb_frontend::SpanExt;

    match &arena[id] {
        Node::TypeExpr(TypeExpr::Primitive { kw }) => {
            kw_to_type(*kw).unwrap_or(Type::Error)
        }
        Node::TypeExpr(TypeExpr::Named { name_span }) => {
            let name = name_span.slice(source);
            let sym = interner.intern(name);
            Type::TypeRef { name: sym }
        }
        Node::TypeExpr(TypeExpr::Array { elem, rank }) => {
            let elem_ty = resolve_type_expr(arena, *elem, interner, source);
            Type::Array {
                elem: Box::new(elem_ty),
                rank: *rank,
            }
        }
        Node::TypeExpr(TypeExpr::FnPtr { params, ret }) => {
            let param_types: Vec<Type> = params
                .iter()
                .filter_map(|&pid| {
                    if let Node::Param(p) = &arena[pid] {
                        p.ty.map(|tid| resolve_type_expr(arena, tid, interner, source))
                            .or_else(|| p.sigil.map(sigil_to_type))
                    } else {
                        None
                    }
                })
                .collect();
            let ret_ty = ret.map(|rid| Box::new(resolve_type_expr(arena, rid, interner, source)));
            Type::FnPtr {
                params: param_types,
                ret: ret_ty,
            }
        }
        Node::TypeExpr(TypeExpr::Paren { inner }) => {
            resolve_type_expr(arena, *inner, interner, source)
        }
        Node::TypeExpr(TypeExpr::Error) => Type::Error,
        _ => Type::Error,
    }
}

/// Determine a variable's type from its optional sigil and optional `As` type
/// annotation. Returns `(type, error_if_any)`.
///
/// Rules:
/// - Neither sigil nor As → defaults to Int
/// - Sigil only → sigil's type
/// - As only → the As type
/// - Both → must agree (error if not)
pub fn resolve_var_type(
    sigil: Option<Sigil>,
    as_ty: Option<&Type>,
) -> (Type, bool) {
    match (sigil, as_ty) {
        (None, None) => (Type::Int, false),
        (Some(s), None) => (sigil_to_type(s), false),
        (None, Some(t)) => (t.clone(), false),
        (Some(s), Some(t)) => {
            let sigil_ty = sigil_to_type(s);
            if sigil_ty == *t {
                (sigil_ty, false)
            } else {
                (sigil_ty, true) // E0320 — caller emits the diagnostic
            }
        }
    }
}

/// Determine a function's return type from its optional return sigil and
/// optional `As` return type annotation.
///
/// If neither is present, the function is a Sub (returns Void).
pub fn resolve_return_type(
    return_sigil: Option<Sigil>,
    return_ty: Option<&Type>,
) -> (Type, bool) {
    match (return_sigil, return_ty) {
        (None, None) => (Type::Void, false),
        (Some(s), None) => (sigil_to_type(s), false),
        (None, Some(t)) => (t.clone(), false),
        (Some(s), Some(t)) => {
            let sigil_ty = sigil_to_type(s);
            if sigil_ty == *t {
                (sigil_ty, false)
            } else {
                (sigil_ty, true) // sigil/As disagree
            }
        }
    }
}

/// Numeric promotion: given two numeric types, return the wider one.
pub(crate) fn numeric_promote(a: &Type, b: &Type) -> Type {
    fn rank(t: &Type) -> u8 {
        match t {
            Type::Byte => 1,
            Type::Short => 2,
            Type::Int | Type::UInt => 3,
            Type::Long | Type::ULong => 4,
            Type::Float => 5,
            _ => 0,
        }
    }
    if rank(a) >= rank(b) { a.clone() } else { b.clone() }
}

/// Determine the result type of a binary operation, or `None` if the types
/// are incompatible for this operator.
pub fn binary_result_type(op: BinOp, lhs: &Type, rhs: &Type) -> Option<Type> {
    match op {
        // Arithmetic
        BinOp::Add => {
            if *lhs == Type::String || *rhs == Type::String {
                Some(Type::String)
            } else if lhs.is_numeric() && rhs.is_numeric() {
                Some(numeric_promote(lhs, rhs))
            } else {
                None
            }
        }
        BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow | BinOp::Mod => {
            if lhs.is_numeric() && rhs.is_numeric() {
                Some(numeric_promote(lhs, rhs))
            } else {
                None
            }
        }
        BinOp::IntDiv => {
            if lhs.is_numeric() && rhs.is_numeric() {
                Some(Type::Int)
            } else {
                None
            }
        }

        // Bitwise
        BinOp::BinAnd | BinOp::BinOr | BinOp::BinXor => {
            if lhs.is_integer() && rhs.is_integer() {
                Some(numeric_promote(lhs, rhs))
            } else {
                None
            }
        }
        BinOp::Shl | BinOp::Shr | BinOp::Sar => {
            if lhs.is_integer() && rhs.is_integer() {
                Some(lhs.clone())
            } else {
                None
            }
        }

        // Comparison — result is always Bool
        BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
            if (lhs.is_numeric() && rhs.is_numeric())
                || (*lhs == Type::String && *rhs == Type::String)
                || (*lhs == Type::Bool && *rhs == Type::Bool)
                || (lhs.is_reference() && rhs.is_reference())
                || (*lhs == Type::Null && rhs.is_reference())
                || (lhs.is_reference() && *rhs == Type::Null)
                || (*lhs == Type::Null && *rhs == Type::Null)
            {
                Some(Type::Bool)
            } else {
                None
            }
        }

        // Logical
        BinOp::And | BinOp::Or | BinOp::Xor => {
            if (*lhs == Type::Bool || lhs.is_numeric())
                && (*rhs == Type::Bool || rhs.is_numeric())
            {
                Some(Type::Bool)
            } else {
                None
            }
        }
    }
}

/// Determine the result type of a unary operation.
pub fn unary_result_type(op: UnOp, operand: &Type) -> Option<Type> {
    match op {
        UnOp::Plus | UnOp::Neg => {
            if operand.is_numeric() {
                Some(operand.clone())
            } else {
                None
            }
        }
        UnOp::Not => {
            if *operand == Type::Bool || operand.is_numeric() {
                Some(Type::Bool)
            } else {
                None
            }
        }
        UnOp::BinNot => {
            if operand.is_integer() {
                Some(operand.clone())
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cb_frontend::BinOp;

    #[test]
    fn numeric_promote_wider_wins() {
        assert_eq!(numeric_promote(&Type::Byte, &Type::Int), Type::Int);
        assert_eq!(numeric_promote(&Type::Int, &Type::Byte), Type::Int);
        assert_eq!(numeric_promote(&Type::Int, &Type::Float), Type::Float);
        assert_eq!(numeric_promote(&Type::Short, &Type::Long), Type::Long);
    }

    #[test]
    fn numeric_promote_same_type() {
        assert_eq!(numeric_promote(&Type::Int, &Type::Int), Type::Int);
        assert_eq!(numeric_promote(&Type::Float, &Type::Float), Type::Float);
    }

    #[test]
    fn binary_add_numeric() {
        assert_eq!(binary_result_type(BinOp::Add, &Type::Int, &Type::Int), Some(Type::Int));
        assert_eq!(binary_result_type(BinOp::Add, &Type::Int, &Type::Float), Some(Type::Float));
        assert_eq!(binary_result_type(BinOp::Add, &Type::Byte, &Type::Long), Some(Type::Long));
    }

    #[test]
    fn binary_add_string() {
        assert_eq!(binary_result_type(BinOp::Add, &Type::String, &Type::String), Some(Type::String));
        assert_eq!(binary_result_type(BinOp::Add, &Type::Int, &Type::String), Some(Type::String));
        assert_eq!(binary_result_type(BinOp::Add, &Type::String, &Type::Float), Some(Type::String));
    }

    #[test]
    fn binary_sub_requires_numeric() {
        assert_eq!(binary_result_type(BinOp::Sub, &Type::Int, &Type::Float), Some(Type::Float));
        assert!(binary_result_type(BinOp::Sub, &Type::String, &Type::Int).is_none());
    }

    #[test]
    fn binary_intdiv_always_int() {
        assert_eq!(binary_result_type(BinOp::IntDiv, &Type::Int, &Type::Int), Some(Type::Int));
        assert_eq!(binary_result_type(BinOp::IntDiv, &Type::Float, &Type::Float), Some(Type::Int));
    }

    #[test]
    fn binary_comparison_returns_bool() {
        assert_eq!(binary_result_type(BinOp::Eq, &Type::Int, &Type::Int), Some(Type::Bool));
        assert_eq!(binary_result_type(BinOp::Lt, &Type::Int, &Type::Float), Some(Type::Bool));
        assert_eq!(binary_result_type(BinOp::Eq, &Type::String, &Type::String), Some(Type::Bool));
        assert_eq!(binary_result_type(BinOp::Eq, &Type::Bool, &Type::Bool), Some(Type::Bool));
    }

    #[test]
    fn binary_comparison_incompatible() {
        assert!(binary_result_type(BinOp::Eq, &Type::String, &Type::Int).is_none());
        assert!(binary_result_type(BinOp::Lt, &Type::Bool, &Type::Int).is_none());
    }

    #[test]
    fn binary_logical() {
        assert_eq!(binary_result_type(BinOp::And, &Type::Bool, &Type::Bool), Some(Type::Bool));
        assert_eq!(binary_result_type(BinOp::Or, &Type::Int, &Type::Bool), Some(Type::Bool));
        assert_eq!(binary_result_type(BinOp::Xor, &Type::Bool, &Type::Int), Some(Type::Bool));
    }

    #[test]
    fn binary_bitwise_requires_integer() {
        assert_eq!(binary_result_type(BinOp::BinAnd, &Type::Int, &Type::Int), Some(Type::Int));
        assert!(binary_result_type(BinOp::BinAnd, &Type::Float, &Type::Int).is_none());
    }

    #[test]
    fn binary_shift_preserves_lhs() {
        assert_eq!(binary_result_type(BinOp::Shl, &Type::Byte, &Type::Int), Some(Type::Byte));
        assert_eq!(binary_result_type(BinOp::Shr, &Type::Long, &Type::Short), Some(Type::Long));
    }

    #[test]
    fn unary_result_types() {
        assert_eq!(unary_result_type(UnOp::Neg, &Type::Int), Some(Type::Int));
        assert_eq!(unary_result_type(UnOp::Neg, &Type::Float), Some(Type::Float));
        assert!(unary_result_type(UnOp::Neg, &Type::String).is_none());
        assert_eq!(unary_result_type(UnOp::Not, &Type::Bool), Some(Type::Bool));
        assert_eq!(unary_result_type(UnOp::Not, &Type::Int), Some(Type::Bool));
        assert_eq!(unary_result_type(UnOp::BinNot, &Type::Int), Some(Type::Int));
        assert!(unary_result_type(UnOp::BinNot, &Type::Float).is_none());
    }
}

