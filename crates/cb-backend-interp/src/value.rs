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

    /// Coerce any value to an `i64`, mirroring CoolBasic's implicit numeric
    /// conversions. Integers convert directly; `Float` truncates toward zero;
    /// a `String` parses a leading integer after trimming (`"3x"` → 3, 0 on no
    /// leading digits) — the one string→int rule used for array indices/dims
    /// and arithmetic alike. Non-numeric variants yield 0; under well-typed IR
    /// they never reach here (sema inserts a `Convert`). Single source of truth
    /// for both the interpreter and the FFI marshaller (II-V10).
    pub(crate) fn to_i64(&self) -> i64 {
        match self {
            Value::Byte(x) => *x as i64,
            Value::Short(x) => *x as i64,
            Value::Int(x) => *x as i64,
            Value::Long(x) => *x,
            Value::Float(x) => *x as i64,
            Value::String(s) => parse_leading_int(s.as_bytes()),
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
    /// `String` uses a strict full parse (a partial float prefix has no
    /// documented CB semantics), yielding 0.0 on any non-numeric content.
    /// Non-numeric variants yield 0.0 (unreachable under well-typed IR).
    /// Single source of truth for the interpreter and FFI marshaller (II-V10).
    pub(crate) fn to_f64(&self) -> f64 {
        match self {
            Value::Byte(x) => *x as f64,
            Value::Short(x) => *x as f64,
            Value::Int(x) => *x as f64,
            Value::Long(x) => *x as f64,
            Value::Float(x) => *x,
            Value::String(s) => std::str::from_utf8(s.as_bytes())
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
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

/// Parse a leading integer from UTF-8 bytes, mirroring `stoi(trim(s))`: skip
/// leading ASCII whitespace, accept an optional `+`/`-`, then consume digits
/// up to the first non-digit. Returns 0 when no digits lead (matching
/// cbEnchanted's `try { stoi } catch { 0 }`). Saturates rather than wrapping.
fn parse_leading_int(bytes: &[u8]) -> i64 {
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        neg = bytes[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut val: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        val = val
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if i == start {
        return 0; // no leading digits
    }
    if neg { -val } else { val }
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
