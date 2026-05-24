use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use cb_ir::types::IrType;
use cb_ir::{FuncId, StructDefInfo};

use crate::heap::{ArrayObj, StructObj, TypeInstanceId};

#[derive(Clone, Debug)]
pub enum Value {
    Byte(u8),
    Short(i16),
    Int(i32),
    UInt(u32),
    Long(i64),
    ULong(u64),
    Float(f64),
    Bool(bool),
    String(Rc<str>),
    Array(Rc<RefCell<ArrayObj>>),
    TypeInstance(TypeInstanceId),
    Struct(Box<StructObj>),
    FnPtr(Option<FuncId>),
    Null,
    Void,
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(v) => *v != 0,
            Value::Long(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Byte(v) => *v != 0,
            Value::Short(v) => *v != 0,
            Value::UInt(v) => *v != 0,
            Value::ULong(v) => *v != 0,
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::TypeInstance(_) | Value::Struct(_) => true,
            Value::FnPtr(f) => f.is_some(),
            Value::Null => false,
            Value::Void => false,
        }
    }

    pub fn as_string(&self) -> Rc<str> {
        match self {
            Value::String(s) => Rc::clone(s),
            Value::Int(v) => Rc::from(v.to_string().as_str()),
            Value::Long(v) => Rc::from(v.to_string().as_str()),
            Value::Float(v) => Rc::from(v.to_string().as_str()),
            Value::Bool(v) => Rc::from(if *v { "True" } else { "False" }),
            Value::Byte(v) => Rc::from(v.to_string().as_str()),
            Value::Short(v) => Rc::from(v.to_string().as_str()),
            Value::UInt(v) => Rc::from(v.to_string().as_str()),
            Value::ULong(v) => Rc::from(v.to_string().as_str()),
            Value::Array(_) => Rc::from("<Array>"),
            Value::TypeInstance(_) => Rc::from("<TypeInstance>"),
            Value::Struct(_) => Rc::from("<Struct>"),
            Value::FnPtr(_) => Rc::from("<FnPtr>"),
            Value::Null => Rc::from("Null"),
            Value::Void => Rc::from(""),
        }
    }
}

pub fn default_value(ty: &IrType, struct_defs: &[StructDefInfo]) -> Value {
    match ty {
        IrType::Byte => Value::Byte(0),
        IrType::Short => Value::Short(0),
        IrType::Int => Value::Int(0),
        IrType::UInt => Value::UInt(0),
        IrType::Long => Value::Long(0),
        IrType::ULong => Value::ULong(0),
        IrType::Float => Value::Float(0.0),
        IrType::Bool => Value::Bool(false),
        IrType::String => Value::String(Rc::from("")),
        IrType::StructVal(name) => {
            if let Some(def) = struct_defs.iter().find(|d| d.name == *name) {
                let fields = def
                    .fields
                    .iter()
                    .map(|(fname, fty)| (*fname, default_value(fty, struct_defs)))
                    .collect();
                Value::Struct(Box::new(StructObj {
                    struct_name: *name,
                    fields,
                }))
            } else {
                Value::Null
            }
        }
        IrType::FnPtr(_) => Value::FnPtr(None),
        IrType::Null | IrType::Void => Value::Null,
        IrType::TypeRef(_) => Value::Null,
        IrType::Array { .. } => Value::Null,
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Byte(v) => write!(f, "{v}"),
            Value::Short(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::UInt(v) => write!(f, "{v}"),
            Value::Long(v) => write!(f, "{v}"),
            Value::ULong(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{}", if *v { "True" } else { "False" }),
            Value::String(s) => write!(f, "{s}"),
            Value::Array(_) => write!(f, "<Array>"),
            Value::TypeInstance(id) => write!(f, "<TypeInstance#{}>", id.0),
            Value::Struct(_) => write!(f, "<Struct>"),
            Value::FnPtr(Some(id)) => write!(f, "<FnPtr#{}>", id.0),
            Value::FnPtr(None) => write!(f, "<FnPtr:Null>"),
            Value::Null => write!(f, "Null"),
            Value::Void => Ok(()),
        }
    }
}
