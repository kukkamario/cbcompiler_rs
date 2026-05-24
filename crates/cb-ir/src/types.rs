//! IR-level type representation.

use cb_diagnostics::Symbol;

/// Type representation in the IR — mirrors the semantic `Type` but without
/// `Error` and with backend-friendly structure.
#[derive(Clone, Debug, PartialEq)]
pub enum IrType {
    Byte,
    Short,
    Int,
    UInt,
    Long,
    ULong,
    Float,
    Bool,
    String,
    Array { elem: Box<IrType>, rank: u8 },
    TypeRef(Symbol),
    StructVal(Symbol),
    FnPtr(Box<FnSig>),
    RuntimeType(String),
    Null,
    Void,
}

/// Function signature for function pointer types and function declarations.
#[derive(Clone, Debug, PartialEq)]
pub struct FnSig {
    pub params: Vec<IrType>,
    pub ret: Box<IrType>,
}

impl IrType {
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Self::Byte
                | Self::Short
                | Self::Int
                | Self::UInt
                | Self::Long
                | Self::ULong
                | Self::Float
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Self::Byte | Self::Short | Self::Int | Self::UInt | Self::Long | Self::ULong
        )
    }
}
