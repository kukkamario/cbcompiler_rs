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
//!   - `IrType::Bool` passes as `u8` (Rust's `bool` ABI).
//!   - `IrType::String` passes as `*const c_char` (null-terminated UTF-8).
//!   - `IrType::RuntimeType` passes as a generic pointer; the runtime
//!     reinterprets the bits via `(uintptr_t)` casts.

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_void};

use cb_ir::FnSig;
use cb_ir::types::IrType;
use libffi::middle::{Arg, Cif, CodePtr, Type};

use crate::value::Value;

/// Owned representation of a marshaled argument. Kept alive for the
/// duration of the libffi call so the address passed via `Arg::new` stays
/// valid until `cif.call` returns.
enum Marshaled {
    I8(i8),
    I16(i16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(u8),
    /// Owned C string + the raw pointer libffi reads. The CString must
    /// outlive the call.
    Str { _owner: CString, ptr: *const c_char },
    Ptr(*const c_void),
}

impl Marshaled {
    fn as_arg(&self) -> Arg<'_> {
        match self {
            Marshaled::I8(v) => Arg::new(v),
            Marshaled::I16(v) => Arg::new(v),
            Marshaled::I32(v) => Arg::new(v),
            Marshaled::U32(v) => Arg::new(v),
            Marshaled::I64(v) => Arg::new(v),
            Marshaled::U64(v) => Arg::new(v),
            Marshaled::F64(v) => Arg::new(v),
            Marshaled::Bool(v) => Arg::new(v),
            Marshaled::Str { ptr, .. } => Arg::new(ptr),
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
        IrType::UInt => Type::u32(),
        IrType::Long => Type::i64(),
        IrType::ULong => Type::u64(),
        IrType::Float => Type::f64(),
        IrType::Bool => Type::u8(),
        IrType::String | IrType::RuntimeType(_) => Type::pointer(),
        other => panic!("unsupported runtime ABI type: {other:?}"),
    }
}

fn value_as_i64(v: &Value) -> i64 {
    match v {
        Value::Byte(x) => *x as i64,
        Value::Short(x) => *x as i64,
        Value::Int(x) => *x as i64,
        Value::UInt(x) => *x as i64,
        Value::Long(x) => *x,
        Value::ULong(x) => *x as i64,
        Value::Float(x) => *x as i64,
        Value::Bool(true) => 1,
        Value::Bool(false) => 0,
        _ => 0,
    }
}

fn value_as_f64(v: &Value) -> f64 {
    match v {
        Value::Byte(x) => *x as f64,
        Value::Short(x) => *x as f64,
        Value::Int(x) => *x as f64,
        Value::UInt(x) => *x as f64,
        Value::Long(x) => *x as f64,
        Value::ULong(x) => *x as f64,
        Value::Float(x) => *x,
        Value::Bool(true) => 1.0,
        Value::Bool(false) => 0.0,
        _ => 0.0,
    }
}

fn marshal(value: &Value, ty: &IrType) -> Marshaled {
    match ty {
        IrType::Byte => Marshaled::I8(value_as_i64(value) as i8),
        IrType::Short => Marshaled::I16(value_as_i64(value) as i16),
        IrType::Int => Marshaled::I32(value_as_i64(value) as i32),
        IrType::UInt => Marshaled::U32(value_as_i64(value) as u32),
        IrType::Long => Marshaled::I64(value_as_i64(value)),
        IrType::ULong => Marshaled::U64(value_as_i64(value) as u64),
        IrType::Float => Marshaled::F64(value_as_f64(value)),
        IrType::Bool => Marshaled::Bool(if value.is_truthy() { 1 } else { 0 }),
        IrType::String => {
            let s = value.as_string();
            let owner = CString::new(s.as_bytes()).unwrap_or_default();
            let ptr = owner.as_ptr();
            Marshaled::Str { _owner: owner, ptr }
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
) -> Value {
    let arg_types: Vec<Type> = sig.params.iter().map(ir_to_ffi_type).collect();
    let ret_type = ir_to_ffi_type(&sig.ret);
    let cif = Cif::new(arg_types, ret_type);

    // Marshaled buffer must outlive `arg_refs` so the pointers remain valid.
    let marshaled: Vec<Marshaled> = sig
        .params
        .iter()
        .zip(args.iter())
        .map(|(t, v)| marshal(v, t))
        .collect();
    let arg_refs: Vec<Arg> = marshaled.iter().map(|m| m.as_arg()).collect();

    let code = CodePtr(fn_ptr as *mut c_void);

    match sig.ret.as_ref() {
        IrType::Void => {
            unsafe { cif.call::<()>(code, &arg_refs) };
            Value::Void
        }
        IrType::Byte => Value::Byte(unsafe { cif.call::<i8>(code, &arg_refs) } as u8),
        IrType::Short => Value::Short(unsafe { cif.call::<i16>(code, &arg_refs) }),
        IrType::Int => Value::Int(unsafe { cif.call::<i32>(code, &arg_refs) }),
        IrType::UInt => Value::UInt(unsafe { cif.call::<u32>(code, &arg_refs) }),
        IrType::Long => Value::Long(unsafe { cif.call::<i64>(code, &arg_refs) }),
        IrType::ULong => Value::ULong(unsafe { cif.call::<u64>(code, &arg_refs) }),
        IrType::Float => Value::Float(unsafe { cif.call::<f64>(code, &arg_refs) }),
        IrType::Bool => Value::Bool(unsafe { cif.call::<u8>(code, &arg_refs) } != 0),
        IrType::String => {
            let p = unsafe { cif.call::<*const c_char>(code, &arg_refs) };
            if p.is_null() {
                Value::String("".into())
            } else {
                let s = unsafe { std::ffi::CStr::from_ptr(p) }
                    .to_string_lossy()
                    .into_owned();
                Value::String(s.into())
            }
        }
        IrType::RuntimeType(_) => {
            let p = unsafe { cif.call::<*const c_void>(code, &arg_refs) };
            Value::OpaqueHandle(p as usize as u64)
        }
        other => panic!("unsupported runtime return type: {other:?}"),
    }
}
