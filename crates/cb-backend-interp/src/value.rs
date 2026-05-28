use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use cb_ir::types::IrType;
use cb_ir::{FuncId, StructDefInfo};
use cb_runtime_sys::CbStringApi;

use crate::heap::{ArrayObj, StructObj, TypeInstanceId};
use crate::string_handle::CbStringHandle;

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
    String(CbStringHandle),
    Array(Rc<RefCell<ArrayObj>>),
    TypeInstance(TypeInstanceId),
    Struct(Box<StructObj>),
    FnPtr(Option<FuncId>),
    OpaqueHandle(u64),
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
            Value::OpaqueHandle(h) => *h != 0,
            Value::Null => false,
            Value::Void => false,
        }
    }

    /// Coerce any value to a CbString handle. For Value::String this is
    /// a refcount-bumped clone of the existing handle (no allocation). For
    /// other types this formats the value and allocates a fresh handle.
    /// Used by `Convert(String)` and any other site that needs a string
    /// view of a non-string value.
    pub fn as_cb_string(&self, api: &'static CbStringApi) -> CbStringHandle {
        match self {
            Value::String(s) => s.clone(),
            Value::Int(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::Long(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::Float(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::Bool(v) => CbStringHandle::from_bytes(
                api,
                if *v { b"True" } else { b"False" },
            ),
            Value::Byte(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::Short(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::UInt(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::ULong(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::Array(_) => CbStringHandle::from_bytes(api, b"<Array>"),
            Value::TypeInstance(_) => CbStringHandle::from_bytes(api, b"<TypeInstance>"),
            Value::Struct(_) => CbStringHandle::from_bytes(api, b"<Struct>"),
            Value::FnPtr(_) => CbStringHandle::from_bytes(api, b"<FnPtr>"),
            Value::OpaqueHandle(h) => {
                CbStringHandle::from_bytes(api, format!("<Handle#{h}>").as_bytes())
            }
            Value::Null => CbStringHandle::from_bytes(api, b"Null"),
            Value::Void => CbStringHandle::empty(api),
        }
    }
}

pub fn default_value(
    ty: &IrType,
    struct_defs: &[StructDefInfo],
    string_api: &'static CbStringApi,
) -> Value {
    match ty {
        IrType::Byte => Value::Byte(0),
        IrType::Short => Value::Short(0),
        IrType::Int => Value::Int(0),
        IrType::UInt => Value::UInt(0),
        IrType::Long => Value::Long(0),
        IrType::ULong => Value::ULong(0),
        IrType::Float => Value::Float(0.0),
        IrType::Bool => Value::Bool(false),
        IrType::String => Value::String(CbStringHandle::empty(string_api)),
        IrType::StructVal(name) => {
            if let Some(def) = struct_defs.iter().find(|d| d.name == *name) {
                let fields = def
                    .fields
                    .iter()
                    .map(|(fname, fty)| (*fname, default_value(fty, struct_defs, string_api)))
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
        IrType::RuntimeType(_) => Value::Null,
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
            Value::TypeInstance(id) => write!(f, "<TypeInstance#{}>", id.index),
            Value::Struct(_) => write!(f, "<Struct>"),
            Value::FnPtr(Some(id)) => write!(f, "<FnPtr#{}>", id.0),
            Value::FnPtr(None) => write!(f, "<FnPtr:Null>"),
            Value::OpaqueHandle(h) => write!(f, "<Handle#{h}>"),
            Value::Null => write!(f, "Null"),
            Value::Void => Ok(()),
        }
    }
}
