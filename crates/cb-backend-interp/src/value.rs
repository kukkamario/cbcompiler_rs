#![allow(unsafe_code)]

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use cb_ir::types::IrType;
use cb_ir::{FuncId, StructDefInfo};
use cb_runtime_sys::{
    CbString, CbStringApi, cb_rt_float_to_string, cb_rt_int_to_string, cb_rt_long_to_string,
    cb_rt_string_to_float, cb_rt_string_to_long,
};

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
    /// the numeric types the formatting is delegated to the shared core-runtime
    /// `cb_rt_*_to_string` symbols so the interpreter and a future
    /// native backend cannot diverge — most importantly on `Float` (CB's
    /// 6-significant-digit format). Other types format inline (debug-only forms).
    /// Used by `Convert(String)` and any other site that needs a string
    /// view of a non-string value.
    pub fn as_cb_string(&self, api: &'static CbStringApi) -> CbStringHandle {
        match self {
            Value::String(s) => s.clone(),
            // Byte/Short widen to i32 first (lossless), matching the runtime.
            Value::Int(v) => wrap_owned(api, unsafe { cb_rt_int_to_string(*v) }),
            Value::Long(v) => wrap_owned(api, unsafe { cb_rt_long_to_string(*v) }),
            Value::Float(v) => wrap_owned(api, unsafe { cb_rt_float_to_string(*v) }),
            Value::Byte(v) => wrap_owned(api, unsafe { cb_rt_int_to_string(*v as i32) }),
            Value::Short(v) => wrap_owned(api, unsafe { cb_rt_int_to_string(*v as i32) }),
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

    /// Coerce any value to an `i64`, mirroring CoolBasic's implicit numeric
    /// conversions. Integers convert directly; `Float` truncates toward zero;
    /// a `String` parses a leading integer after trimming (`"3x"` → 3, 0 on no
    /// leading digits) — delegated to the shared core-runtime
    /// `cb_rt_string_to_long`, the one string→int rule used for array
    /// indices/dims and arithmetic alike. Non-numeric variants yield 0; under
    /// well-typed IR they never reach here (sema inserts a `Convert`). Single
    /// source of truth for both the interpreter and the FFI marshaller (II-V10).
    pub(crate) fn to_i64(&self) -> i64 {
        match self {
            Value::Byte(x) => *x as i64,
            Value::Short(x) => *x as i64,
            Value::Int(x) => *x as i64,
            Value::Long(x) => *x,
            Value::Float(x) => *x as i64,
            Value::String(s) => unsafe { cb_rt_string_to_long(s.as_ptr()) },
            // Non-numeric, non-string variants never reach here under
            // well-typed IR (sema inserts a Convert); flag the broken
            // invariant in debug, fall back to 0 in release.
            _ => {
                debug_assert!(false, "Value::to_i64 called on non-numeric Value: {self:?}");
                0
            }
        }
    }

    /// Coerce any value to an `f64`. Integers/floats convert directly; a
    /// `String` uses a lenient `strtod`-style prefix parse — skip leading
    /// whitespace, optional sign, parse a float incl. exponent, stop at the
    /// first invalid char, 0.0 on no valid prefix (`"22yo"` → 22.0) — delegated
    /// to the shared core-runtime `cb_rt_string_to_float` (matching the
    /// real CoolBasic). Non-numeric variants yield 0.0 (unreachable under
    /// well-typed IR). Single source of truth for the interpreter and FFI
    /// marshaller (II-V10).
    pub(crate) fn to_f64(&self) -> f64 {
        match self {
            Value::Byte(x) => *x as f64,
            Value::Short(x) => *x as f64,
            Value::Int(x) => *x as f64,
            Value::Long(x) => *x as f64,
            Value::Float(x) => *x,
            Value::String(s) => unsafe { cb_rt_string_to_float(s.as_ptr()) },
            // Non-numeric, non-string variants never reach here under
            // well-typed IR (sema inserts a Convert); flag the broken
            // invariant in debug, fall back to 0.0 in release.
            _ => {
                debug_assert!(false, "Value::to_f64 called on non-numeric Value: {self:?}");
                0.0
            }
        }
    }
}

/// Wrap an owning `CbString*` (refcount 1) returned by a `cb_rt_*_to_string`
/// conversion symbol into a handle. The C side never returns null (allocation
/// failure aborts there), but map null to the empty sentinel defensively.
fn wrap_owned(api: &'static CbStringApi, ptr: *mut CbString) -> CbStringHandle {
    if ptr.is_null() {
        CbStringHandle::empty(api)
    } else {
        CbStringHandle::from_raw(api, ptr)
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
        // Reference-like types default to Null per CB semantics: an
        // uninitialized array, type instance, runtime handle, etc. is Null
        // until assigned. This is the intended default, not a fallback —
        // unlike the unknown-`StructVal` case above, which panics.
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
