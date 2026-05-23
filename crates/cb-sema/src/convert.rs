//! Implicit type conversion rules for CoolBasic.

use std::collections::HashMap;

use cb_frontend::NodeId;

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
    entries: HashMap<NodeId, Conversion>,
}

impl ConversionTable {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, id: NodeId, conv: Conversion) {
        self.entries.insert(id, conv);
    }

    /// Look up whether a node has an implicit conversion.
    pub fn get(&self, id: NodeId) -> Option<Conversion> {
        self.entries.get(&id).copied()
    }
}
