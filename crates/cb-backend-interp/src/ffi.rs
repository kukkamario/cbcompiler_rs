//! libffi-based dispatch for runtime function calls.
//!
//! Given a function pointer from the runtime catalog and an `FnSig` from the
//! IR, this module marshals interpreter `Value`s to C ABI arguments, invokes
//! the function through libffi, and demarshals the return value back into a
//! `Value`. This lets adding a new runtime function require zero interpreter
//! changes — only a new `CB_FN` entry in `runtime/catalog.cpp`.
//!
//! ABI conventions assumed by the marshalling here:
//!   - `IrType::Float` always passes/returns as `double` at the C boundary.
//!     CB Float is conceptually 32-bit at the language level, but every C
//!     runtime function takes/returns `double` so libffi can dispatch with
//!     one uniform float ABI.
//!   - `IrType::String` passes as `*mut CbString` — the opaque refcounted
//!     handle defined in `cb_string.cpp`. Inputs are borrowed (the handle
//!     stays owned by the caller for the duration of the call); returned
//!     handles are owned (refcount = 1; `CbStringHandle::from_raw` takes
//!     ownership without an extra retain).
//!   - `IrType::RuntimeType` passes as a generic pointer; the runtime
//!     reinterprets the bits via `(uintptr_t)` casts.

#![allow(unsafe_code)]

use std::ffi::c_void;

use cb_ir::FnSig;
use cb_ir::types::IrType;
use cb_runtime_sys::{CbString, CbStringApi};
use libffi::middle::{Arg, Cif, CodePtr, Type};

use crate::string_handle::CbStringHandle;
use crate::value::Value;

/// Owned representation of a marshaled argument. Kept alive for the
/// duration of the libffi call so the address passed via `Arg::new` stays
/// valid until `cif.call` returns.
enum Marshaled {
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F64(f64),
    /// Owned string handle plus the raw pointer libffi reads. Holding the
    /// handle here keeps the refcount > 0 across the call even if the
    /// source value was a coercion (e.g. `print(5)` → `Convert(Int, String)`
    /// → marshal would otherwise drop the freshly-allocated handle before
    /// `cif.call` dereferences it).
    CbStringArg {
        _owner: CbStringHandle,
        ptr: *const c_void,
    },
    Ptr(*const c_void),
}

impl Marshaled {
    fn as_arg(&self) -> Arg<'_> {
        match self {
            Marshaled::I8(v) => Arg::new(v),
            Marshaled::I16(v) => Arg::new(v),
            Marshaled::I32(v) => Arg::new(v),
            Marshaled::I64(v) => Arg::new(v),
            Marshaled::F64(v) => Arg::new(v),
            Marshaled::CbStringArg { ptr, .. } => Arg::new(ptr),
            Marshaled::Ptr(p) => Arg::new(p),
        }
    }
}

fn ir_to_ffi_type(ty: &IrType) -> Type {
    match ty {
        IrType::Void => Type::void(),
        IrType::Byte => Type::i8(),
        IrType::Short => Type::i16(),
        IrType::Int => Type::i32(),
        IrType::Long => Type::i64(),
        IrType::Float => Type::f64(),
        IrType::String | IrType::RuntimeType(_) => Type::pointer(),
        other => panic!("unsupported runtime ABI type: {other:?}"),
    }
}

fn marshal(value: &Value, ty: &IrType, string_api: &'static CbStringApi) -> Marshaled {
    // Numeric marshalling shares the interpreter's single coercion source of
    // truth, `Value::to_i64` / `to_f64` (II-V10). Under well-typed IR these
    // only ever see numeric variants — a `Value::String` reaching a numeric
    // slot would mean sema failed to insert a `Convert`; the helpers' non-
    // numeric arms keep that a quiet 0 rather than a panic (see II-V11).
    match ty {
        IrType::Byte => Marshaled::I8(value.to_i64() as i8),
        IrType::Short => Marshaled::I16(value.to_i64() as i16),
        IrType::Int => Marshaled::I32(value.to_i64() as i32),
        IrType::Long => Marshaled::I64(value.to_i64()),
        IrType::Float => Marshaled::F64(value.to_f64()),
        IrType::String => {
            // Always go through `as_cb_string` — for Value::String this is
            // a free retain; for anything else it allocates a coerced handle
            // (matches the old `as_string()` behavior, just without the
            // `CString::new` allocation).
            let handle = value.as_cb_string(string_api);
            let ptr = handle.as_ptr() as *const c_void;
            Marshaled::CbStringArg {
                _owner: handle,
                ptr,
            }
        }
        IrType::RuntimeType(_) => {
            let h = match value {
                Value::OpaqueHandle(h) => *h,
                Value::Null => 0,
                _ => 0,
            };
            Marshaled::Ptr(h as usize as *const c_void)
        }
        other => panic!("unsupported runtime arg type: {other:?}"),
    }
}

/// Dispatch a runtime call through libffi.
///
/// # Safety
///
/// `fn_ptr` must point to a function whose C ABI matches `sig` exactly. The
/// catalog loader (`cb_runtime_sys::load_catalog`) guarantees this for every
/// entry in `runtime/catalog.cpp` because the `CB_FN` macro derives the
/// parameter and return types from the function's own signature.
pub unsafe fn call(
    fn_ptr: unsafe extern "C" fn(),
    sig: &FnSig,
    args: &[Value],
    string_api: &'static CbStringApi,
) -> Value {
    let arg_types: Vec<Type> = sig.params.iter().map(ir_to_ffi_type).collect();
    let ret_type = ir_to_ffi_type(&sig.ret);
    let cif = Cif::new(arg_types, ret_type);

    // Marshaled buffer must outlive `arg_refs` so the pointers remain valid.
    let marshaled: Vec<Marshaled> = sig
        .params
        .iter()
        .zip(args.iter())
        .map(|(t, v)| marshal(v, t, string_api))
        .collect();
    let arg_refs: Vec<Arg> = marshaled.iter().map(|m| m.as_arg()).collect();

    let code = CodePtr(fn_ptr as *mut c_void);

    match sig.ret.as_ref() {
        IrType::Void => {
            unsafe { cif.call::<()>(code, &arg_refs) };
            Value::Void
        }
        IrType::Byte => Value::Byte(unsafe { cif.call::<i8>(code, &arg_refs) } as u8),
        IrType::Short => Value::Short(unsafe { cif.call::<i16>(code, &arg_refs) } as u16),
        IrType::Int => Value::Int(unsafe { cif.call::<i32>(code, &arg_refs) }),
        IrType::Long => Value::Long(unsafe { cif.call::<i64>(code, &arg_refs) }),
        IrType::Float => Value::Float(unsafe { cif.call::<f64>(code, &arg_refs) }),
        IrType::String => {
            let p = unsafe { cif.call::<*mut CbString>(code, &arg_refs) };
            // Per ABI, runtime string-returning functions never produce null —
            // they yield the empty sentinel instead. Treat null defensively as
            // the empty handle so a runtime bug doesn't crash the interpreter.
            let handle = if p.is_null() {
                CbStringHandle::empty(string_api)
            } else {
                CbStringHandle::from_raw(string_api, p)
            };
            Value::String(handle)
        }
        IrType::RuntimeType(_) => {
            let p = unsafe { cif.call::<*const c_void>(code, &arg_refs) };
            // A null handle is the CB null value, not OpaqueHandle(0): runtime
            // functions document "0 on failure" (e.g. LoadFont / LoadImage), and
            // CB compares failed handles against `Null`. Wrapping null as a
            // distinct OpaqueHandle(0) would make `h = Null` always false.
            if p.is_null() {
                Value::Null
            } else {
                Value::OpaqueHandle(p as usize as u64)
            }
        }
        other => panic!("unsupported runtime return type: {other:?}"),
    }
}
