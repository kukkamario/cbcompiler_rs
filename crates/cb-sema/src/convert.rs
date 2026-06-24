//! Implicit type conversion rules for CoolBasic.

use std::collections::HashMap;

use cb_frontend::NodeId;

use crate::types::Type;

/// An implicit conversion the type checker inserted on a node.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Conversion {
    NumericWiden,
    IntToFloat,
    FloatToInt,
    NumericToString,
    NullToRef,
}

/// Records which AST nodes need an implicit conversion applied.
pub struct ConversionTable {
    entries: HashMap<NodeId, (Conversion, Type)>,
}

impl ConversionTable {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, id: NodeId, conv: Conversion, target: Type) {
        self.entries.insert(id, (conv, target));
    }

    /// Look up whether a node has an implicit conversion.
    pub fn get(&self, id: NodeId) -> Option<Conversion> {
        self.entries.get(&id).map(|(c, _)| *c)
    }

    /// Look up the conversion and target type for a node.
    pub fn get_with_target(&self, id: NodeId) -> Option<(Conversion, &Type)> {
        self.entries.get(&id).map(|(c, t)| (*c, t))
    }
}

/// Integer width rank for widening/narrowing determination.
fn int_rank(t: &Type) -> Option<u8> {
    match t {
        Type::Byte => Some(1),
        Type::Short => Some(2),
        Type::Int => Some(3),
        Type::Long => Some(4),
        _ => None,
    }
}

/// Find the implicit conversion needed to convert `from` to `to`, if one exists.
pub fn find_implicit_conversion(from: &Type, to: &Type) -> Option<Conversion> {
    if from == to {
        return None; // no conversion needed
    }

    match (from, to) {
        // Integer widening/narrowing
        (f, t) if f.is_integer() && t.is_integer() => Some(Conversion::NumericWiden),

        // Int → Float
        (f, Type::Float) if f.is_integer() => Some(Conversion::IntToFloat),

        // Float → Int (narrowing)
        (Type::Float, t) if t.is_integer() => Some(Conversion::FloatToInt),

        // Numeric → String (implicit in + context)
        (f, Type::String) if f.is_numeric() => Some(Conversion::NumericToString),

        // Null → any reference type
        (Type::Null, t) if t.is_reference() => Some(Conversion::NullToRef),

        // Null → runtime opaque type. This is a *separate* arm, not dead code:
        // `is_reference()` deliberately excludes `RuntimeType` (it has no
        // ordering — see `Type::is_reference`), so the arm above does not cover
        // opaque handles even though they default to `Null` (§3.5).
        (Type::Null, Type::RuntimeType { .. }) => Some(Conversion::NullToRef),

        _ => None,
    }
}

/// Inclusive value range `[min, max]` of an integer `Type`, in `i128` so every
/// bound is representable. Returns `None` for non-integer types. Used to
/// range-check integer literals against a narrower target type
/// (cb_syntax.md §1.6/§3.4).
pub fn int_range(ty: &Type) -> Option<(i128, i128)> {
    let (min, max): (i128, i128) = match ty {
        Type::Byte => (0, u8::MAX as i128),
        Type::Short => (0, u16::MAX as i128),
        Type::Int => (i32::MIN as i128, i32::MAX as i128),
        Type::Long => (i64::MIN as i128, i64::MAX as i128),
        _ => return None,
    };
    Some((min, max))
}

/// Whether a conversion is narrowing (loses precision) and should emit a warning.
pub fn is_narrowing(conv: Conversion, from: &Type, to: &Type) -> bool {
    match conv {
        Conversion::FloatToInt => true,
        Conversion::NumericWiden => {
            // Narrowing if the destination is smaller than the source.
            match (int_rank(from), int_rank(to)) {
                (Some(fr), Some(tr)) => tr < fr,
                _ => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_type_no_conversion() {
        assert!(find_implicit_conversion(&Type::Int, &Type::Int).is_none());
        assert!(find_implicit_conversion(&Type::Float, &Type::Float).is_none());
        assert!(find_implicit_conversion(&Type::String, &Type::String).is_none());
    }

    #[test]
    fn integer_widening() {
        assert_eq!(
            find_implicit_conversion(&Type::Byte, &Type::Int),
            Some(Conversion::NumericWiden)
        );
        assert_eq!(
            find_implicit_conversion(&Type::Short, &Type::Long),
            Some(Conversion::NumericWiden)
        );
    }

    #[test]
    fn integer_narrowing() {
        let conv = find_implicit_conversion(&Type::Long, &Type::Byte).unwrap();
        assert_eq!(conv, Conversion::NumericWiden);
        assert!(is_narrowing(conv, &Type::Long, &Type::Byte));
    }

    #[test]
    fn int_to_float() {
        assert_eq!(
            find_implicit_conversion(&Type::Int, &Type::Float),
            Some(Conversion::IntToFloat)
        );
        assert!(!is_narrowing(
            Conversion::IntToFloat,
            &Type::Int,
            &Type::Float
        ));
    }

    #[test]
    fn float_to_int_narrowing() {
        let conv = find_implicit_conversion(&Type::Float, &Type::Int).unwrap();
        assert_eq!(conv, Conversion::FloatToInt);
        assert!(is_narrowing(conv, &Type::Float, &Type::Int));
    }

    #[test]
    fn numeric_to_string() {
        assert_eq!(
            find_implicit_conversion(&Type::Int, &Type::String),
            Some(Conversion::NumericToString)
        );
        assert_eq!(
            find_implicit_conversion(&Type::Float, &Type::String),
            Some(Conversion::NumericToString)
        );
    }

    #[test]
    fn null_to_ref() {
        let ty = Type::TypeRef {
            name: cb_diagnostics::Symbol::DUMMY,
        };
        assert_eq!(
            find_implicit_conversion(&Type::Null, &ty),
            Some(Conversion::NullToRef)
        );
    }

    #[test]
    fn no_path_string_to_int() {
        assert!(find_implicit_conversion(&Type::String, &Type::Int).is_none());
    }

    #[test]
    fn int_range_bounds() {
        assert_eq!(int_range(&Type::Byte), Some((0, 255)));
        assert_eq!(int_range(&Type::Short), Some((0, 65535)));
        assert_eq!(
            int_range(&Type::Int),
            Some((i32::MIN as i128, i32::MAX as i128))
        );
        assert_eq!(
            int_range(&Type::Long),
            Some((i64::MIN as i128, i64::MAX as i128))
        );
        assert_eq!(int_range(&Type::Float), None);
        assert_eq!(int_range(&Type::String), None);
    }

    #[test]
    fn widening_is_not_narrowing() {
        assert!(!is_narrowing(
            Conversion::NumericWiden,
            &Type::Byte,
            &Type::Int
        ));
        assert!(!is_narrowing(
            Conversion::NumericWiden,
            &Type::Short,
            &Type::Long
        ));
    }
}
