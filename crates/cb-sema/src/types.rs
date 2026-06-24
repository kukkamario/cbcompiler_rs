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
    Long,
    Float,
    String,

    // Composite
    Array {
        elem: Box<Type>,
        rank: u8,
    },
    TypeRef {
        name: Symbol,
    },
    StructVal {
        name: Symbol,
    },
    FnPtr {
        params: Vec<Type>,
        ret: Option<Box<Type>>,
    },
    RuntimeType {
        name: Symbol,
    },

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
            Type::Byte | Type::Short | Type::Int | Type::Long | Type::Float
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Type::Byte | Type::Short | Type::Int | Type::Long)
    }

    /// Reference types that support the full reference comparison/conversion
    /// surface: `Null` coercion (`Conversion::NullToRef`), `=`/`<>` against any
    /// other reference or `Null`, **and** ordering (`<`, `>`, …).
    ///
    /// `Type::RuntimeType` is deliberately **excluded** even though it is
    /// reference-like (defaults to `Null`, cb_syntax.md §3.5). Opaque runtime
    /// handles have identity-only equality and *no ordering*: `img1 < img2` is a
    /// compile error (§3.5). Folding `RuntimeType` in here would make ordering
    /// type-check. Equality and `Null`-conversion contexts that *do* accept
    /// `RuntimeType` therefore special-case it next to this predicate rather than
    /// through it — see `binary_result_type`'s `Eq`/`NotEq` arm and
    /// `find_implicit_conversion`'s `Null` arms.
    pub fn is_reference(&self) -> bool {
        matches!(
            self,
            Type::TypeRef { .. } | Type::Array { .. } | Type::FnPtr { .. }
        )
    }
}

/// Map a sigil to its locked type.
pub fn sigil_to_type(sigil: Sigil) -> Type {
    match sigil {
        Sigil::Integer => Type::Int,
        Sigil::Float => Type::Float,
        Sigil::String => Type::String,
    }
}

/// Map a type keyword to its `Type`.
pub fn kw_to_type(kw: Kw) -> Option<Type> {
    match kw {
        Kw::Byte => Some(Type::Byte),
        Kw::Short => Some(Type::Short),
        Kw::Int | Kw::Integer => Some(Type::Int),
        Kw::Long => Some(Type::Long),
        Kw::Float => Some(Type::Float),
        Kw::String => Some(Type::String),
        _ => None,
    }
}

/// Whether a keyword is a reserved-but-unsupported type name (FD-035). These
/// parse as type atoms (`is_primitive_type_kw`) but resolve to no type; sema
/// rejects them with a clear diagnostic instead of a generic parse error.
pub fn is_reserved_type_kw(kw: Kw) -> bool {
    matches!(
        kw,
        Kw::Bool | Kw::Boolean | Kw::UInt | Kw::UInteger | Kw::ULong
    )
}

/// Resolve a `TypeExpr` AST node to a semantic `Type`.
///
/// Named types are interned but NOT resolved against the symbol table here —
/// that happens during pass 2 when all declarations are known. For now we
/// return `TypeRef` for any `TypeExpr::Named`, which pass 2 may refine to
/// `StructVal` once it knows the declaration kind.
pub fn resolve_type_expr(arena: &Arena, id: NodeId, interner: &mut Interner, source: &str) -> Type {
    use cb_frontend::SpanExt;

    match &arena[id] {
        Node::TypeExpr(TypeExpr::Primitive { kw }) => kw_to_type(*kw).unwrap_or(Type::Error),
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
pub fn resolve_var_type(sigil: Option<Sigil>, as_ty: Option<&Type>) -> (Type, bool) {
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
pub fn resolve_return_type(return_sigil: Option<Sigil>, return_ty: Option<&Type>) -> (Type, bool) {
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

/// Apply CoolBasic's storage-widening rule (FD-035 / cb_syntax.md §3.4):
/// `Byte`/`Short` are storage-only and never compute in their own width, so
/// they widen to `Int`; every other type is returned unchanged. Used wherever
/// an arithmetic/bitwise/shift result must be promoted out of a narrow width.
pub(crate) fn widen_storage(t: &Type) -> Type {
    match t {
        Type::Byte | Type::Short => Type::Int,
        other => other.clone(),
    }
}

/// Numeric promotion for a binary operation: return the wider of the two
/// types, floored at `Int` for integers. `Byte`/`Short` are storage-only and
/// widen to `Int` for all arithmetic (FD-035 / cb_syntax.md §3.4); `Float`
/// beats every integer.
pub(crate) fn numeric_promote(a: &Type, b: &Type) -> Type {
    fn rank(t: &Type) -> u8 {
        match t {
            Type::Byte => 1,
            Type::Short => 2,
            Type::Int => 3,
            Type::Long => 4,
            Type::Float => 5,
            _ => 0,
        }
    }
    let wider = if rank(a) >= rank(b) { a } else { b };
    widen_storage(wider)
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
        BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            if lhs.is_numeric() && rhs.is_numeric() {
                Some(numeric_promote(lhs, rhs))
            } else {
                None
            }
        }
        // Exponentiation always yields Float (cb_syntax.md §3.4); operands are
        // coerced to Float by `check_binary` so the IR `Pow` runs on floats.
        BinOp::Pow => {
            if lhs.is_numeric() && rhs.is_numeric() {
                Some(Type::Float)
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
        // Shifts dispatch on the (widened) LHS: Byte/Short shift in Int width.
        // The count may be any integer; check_binary does not coerce shift
        // operands, so the interpreter widens the LHS (FD-035).
        BinOp::Shl | BinOp::Shr | BinOp::Sar => {
            if lhs.is_integer() && rhs.is_integer() {
                Some(widen_storage(lhs))
            } else {
                None
            }
        }

        // Equality — result is Int (1/0); there is no Bool type (FD-035)
        BinOp::Eq | BinOp::NotEq => {
            // `RuntimeType` is excluded from `is_reference()` (no ordering, see
            // its doc), so equality against opaque handles is spelled out here:
            // identity only between the *same* opaque type (`a == b`), plus
            // comparison with `Null`.
            if (lhs.is_numeric() && rhs.is_numeric())
                || (*lhs == Type::String && *rhs == Type::String)
                || (lhs.is_reference() && rhs.is_reference())
                || (*lhs == Type::Null && rhs.is_reference())
                || (lhs.is_reference() && *rhs == Type::Null)
                || (*lhs == Type::Null && *rhs == Type::Null)
                || matches!((lhs, rhs), (Type::RuntimeType { name: a }, Type::RuntimeType { name: b }) if a == b)
                || (matches!(lhs, Type::RuntimeType { .. }) && *rhs == Type::Null)
                || (*lhs == Type::Null && matches!(rhs, Type::RuntimeType { .. }))
            {
                Some(Type::Int)
            } else {
                None
            }
        }
        // Ordering — `RuntimeType` is intentionally absent: opaque runtime
        // handles have no ordering (`img1 < img2` is a compile error, §3.5), so
        // only `is_reference()` types (which do order) and `Null` appear here.
        BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
            if (lhs.is_numeric() && rhs.is_numeric())
                || (*lhs == Type::String && *rhs == Type::String)
                || (lhs.is_reference() && rhs.is_reference())
                || (*lhs == Type::Null && rhs.is_reference())
                || (lhs.is_reference() && *rhs == Type::Null)
                || (*lhs == Type::Null && *rhs == Type::Null)
            {
                Some(Type::Int)
            } else {
                None
            }
        }

        // Logical — operands tested as `<> 0`, result is Int 1/0 (FD-035)
        BinOp::And | BinOp::Or | BinOp::Xor => {
            if lhs.is_numeric() && rhs.is_numeric() {
                Some(Type::Int)
            } else {
                None
            }
        }
    }
}

/// Determine the result type of a unary operation.
pub fn unary_result_type(op: UnOp, operand: &Type) -> Option<Type> {
    // Byte/Short widen to Int for unary arithmetic/bitwise, mirroring binary
    // promotion (FD-035): e.g. negating a Byte yields a signed Int.
    match op {
        UnOp::Plus | UnOp::Neg => {
            if operand.is_numeric() {
                Some(widen_storage(operand))
            } else {
                None
            }
        }
        UnOp::Not => {
            if operand.is_numeric() {
                Some(Type::Int)
            } else {
                None
            }
        }
        UnOp::BinNot => {
            if operand.is_integer() {
                Some(widen_storage(operand))
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
    fn numeric_promote_narrow_floors_at_int() {
        // Byte/Short are storage-only: arithmetic widens to Int (FD-035).
        assert_eq!(numeric_promote(&Type::Byte, &Type::Byte), Type::Int);
        assert_eq!(numeric_promote(&Type::Short, &Type::Short), Type::Int);
        assert_eq!(numeric_promote(&Type::Byte, &Type::Short), Type::Int);
    }

    #[test]
    fn binary_pow_is_float() {
        // `^` always yields Float (cb_syntax.md §3.4).
        assert_eq!(
            binary_result_type(BinOp::Pow, &Type::Int, &Type::Int),
            Some(Type::Float)
        );
        assert_eq!(
            binary_result_type(BinOp::Pow, &Type::Float, &Type::Int),
            Some(Type::Float)
        );
        assert_eq!(
            binary_result_type(BinOp::Pow, &Type::Int, &Type::String),
            None
        );
    }

    #[test]
    fn binary_add_numeric() {
        assert_eq!(
            binary_result_type(BinOp::Add, &Type::Int, &Type::Int),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Add, &Type::Int, &Type::Float),
            Some(Type::Float)
        );
        assert_eq!(
            binary_result_type(BinOp::Add, &Type::Byte, &Type::Long),
            Some(Type::Long)
        );
    }

    #[test]
    fn binary_add_string() {
        assert_eq!(
            binary_result_type(BinOp::Add, &Type::String, &Type::String),
            Some(Type::String)
        );
        assert_eq!(
            binary_result_type(BinOp::Add, &Type::Int, &Type::String),
            Some(Type::String)
        );
        assert_eq!(
            binary_result_type(BinOp::Add, &Type::String, &Type::Float),
            Some(Type::String)
        );
    }

    #[test]
    fn binary_sub_requires_numeric() {
        assert_eq!(
            binary_result_type(BinOp::Sub, &Type::Int, &Type::Float),
            Some(Type::Float)
        );
        assert!(binary_result_type(BinOp::Sub, &Type::String, &Type::Int).is_none());
    }

    #[test]
    fn binary_div_promotes_int_vs_float() {
        // FD-028: `/` is integer division when both operands are integers, and
        // floating-point division when either operand is a Float (which
        // promotes both). There is no separate `\` integer-division operator.
        assert_eq!(
            binary_result_type(BinOp::Div, &Type::Int, &Type::Int),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Div, &Type::Int, &Type::Float),
            Some(Type::Float)
        );
        assert_eq!(
            binary_result_type(BinOp::Div, &Type::Float, &Type::Float),
            Some(Type::Float)
        );
    }

    #[test]
    fn binary_comparison_returns_int() {
        // Comparisons yield Int 1/0 — there is no Bool type (FD-035).
        assert_eq!(
            binary_result_type(BinOp::Eq, &Type::Int, &Type::Int),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Lt, &Type::Int, &Type::Float),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Eq, &Type::String, &Type::String),
            Some(Type::Int)
        );
    }

    #[test]
    fn binary_comparison_incompatible() {
        assert!(binary_result_type(BinOp::Eq, &Type::String, &Type::Int).is_none());
        assert!(binary_result_type(BinOp::Lt, &Type::String, &Type::Int).is_none());
    }

    #[test]
    fn binary_logical() {
        // Logical ops take numeric operands and yield Int 1/0 (FD-035).
        assert_eq!(
            binary_result_type(BinOp::And, &Type::Int, &Type::Int),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Or, &Type::Int, &Type::Byte),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Xor, &Type::Long, &Type::Int),
            Some(Type::Int)
        );
        assert!(binary_result_type(BinOp::And, &Type::String, &Type::Int).is_none());
    }

    #[test]
    fn binary_bitwise_requires_integer() {
        assert_eq!(
            binary_result_type(BinOp::BinAnd, &Type::Int, &Type::Int),
            Some(Type::Int)
        );
        assert!(binary_result_type(BinOp::BinAnd, &Type::Float, &Type::Int).is_none());
    }

    #[test]
    fn binary_shift_widens_narrow_lhs() {
        // Byte/Short shift in Int width; Int/Long keep their width (FD-035).
        assert_eq!(
            binary_result_type(BinOp::Shl, &Type::Byte, &Type::Int),
            Some(Type::Int)
        );
        assert_eq!(
            binary_result_type(BinOp::Shr, &Type::Long, &Type::Short),
            Some(Type::Long)
        );
        assert_eq!(
            binary_result_type(BinOp::Shl, &Type::Int, &Type::Int),
            Some(Type::Int)
        );
    }

    #[test]
    fn unary_result_types() {
        assert_eq!(unary_result_type(UnOp::Neg, &Type::Int), Some(Type::Int));
        assert_eq!(
            unary_result_type(UnOp::Neg, &Type::Float),
            Some(Type::Float)
        );
        assert!(unary_result_type(UnOp::Neg, &Type::String).is_none());
        // Byte/Short widen to Int for unary arithmetic/bitwise (FD-035).
        assert_eq!(unary_result_type(UnOp::Neg, &Type::Byte), Some(Type::Int));
        assert_eq!(
            unary_result_type(UnOp::BinNot, &Type::Short),
            Some(Type::Int)
        );
        assert_eq!(unary_result_type(UnOp::Not, &Type::Int), Some(Type::Int));
        assert_eq!(unary_result_type(UnOp::Not, &Type::Float), Some(Type::Int));
        assert!(unary_result_type(UnOp::Not, &Type::String).is_none());
        assert_eq!(unary_result_type(UnOp::BinNot, &Type::Int), Some(Type::Int));
        assert!(unary_result_type(UnOp::BinNot, &Type::Float).is_none());
    }
}
