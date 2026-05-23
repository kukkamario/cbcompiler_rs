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
    BoolToNumeric,
    NumericToBool,
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
        Type::Int | Type::UInt => Some(3),
        Type::Long | Type::ULong => Some(4),
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
        (f, t) if f.is_integer() && t.is_integer() => {
            Some(Conversion::NumericWiden)
        }

        // Int → Float
        (f, Type::Float) if f.is_integer() => Some(Conversion::IntToFloat),

        // Float → Int (narrowing)
        (Type::Float, t) if t.is_integer() => Some(Conversion::FloatToInt),

        // Bool → numeric
        (Type::Bool, t) if t.is_numeric() => Some(Conversion::BoolToNumeric),

        // Numeric → Bool
        (f, Type::Bool) if f.is_numeric() => Some(Conversion::NumericToBool),

        // Numeric → String (implicit in + context)
        (f, Type::String) if f.is_numeric() => Some(Conversion::NumericToString),

        // Bool → String
        (Type::Bool, Type::String) => Some(Conversion::NumericToString),

        // Null → any reference type
        (Type::Null, t) if t.is_reference() => Some(Conversion::NullToRef),

        _ => None,
    }
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
