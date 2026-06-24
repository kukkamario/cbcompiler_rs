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
    Short(u16),
    Int(i32),
    Long(i64),
    Float(f64),
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
            Value::Int(v) => *v != 0,
            Value::Long(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Byte(v) => *v != 0,
            Value::Short(v) => *v != 0,
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
            Value::Byte(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
            Value::Short(v) => CbStringHandle::from_bytes(api, v.to_string().as_bytes()),
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
        IrType::Long => Value::Long(0),
        IrType::Float => Value::Float(0.0),
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
                // A `StructVal` whose definition is missing from `struct_defs`
                // is an internal lowering inconsistency, not a runtime
                // condition. The interpreter is the reference implementation
                // and must fail loudly here rather than fabricate a `Null` that
                // surfaces later as a misleading `NullDeref` far from the cause.
                panic!("internal: default_value for unknown struct {name:?}");
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
            Value::Long(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cb_diagnostics::Symbol;

    // II-V20: an unknown struct definition is an internal inconsistency and
    // must fail loudly rather than silently return Null.
    #[test]
    #[should_panic(expected = "unknown struct")]
    fn default_value_unknown_struct_panics() {
        let api = cb_runtime_sys::string_api();
        let _ = default_value(&IrType::StructVal(Symbol::DUMMY), &[], api);
    }
}
