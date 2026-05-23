//! Type representation for CoolBasic's type system.

use cb_diagnostics::{Interner, Symbol};
use cb_frontend::ast::{Node, TypeExpr};
use cb_frontend::{Arena, Kw, NodeId, Sigil};

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
