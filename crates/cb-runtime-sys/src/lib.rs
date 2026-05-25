//! Thin FFI bindings to the CoolBasic C runtime library.
//!
//! Compiles the C runtime via `build.rs` (using the `cc` crate), provides
//! `#[repr(C)]` mirror types for the catalog ABI, and a safe conversion
//! function that produces a `RuntimeCatalog` for use by sema.

use std::collections::HashMap;
use std::ffi::CStr;

use cb_ir::types::IrType;
use cb_ir::{FuncDesc, FuncParamDesc, RuntimeCatalog, RuntimeTypeDesc};

// ── C ABI mirror types ─────────────────────────────────────────────────

#[repr(C)]
pub struct CbTypeDesc {
    pub name: *const std::ffi::c_char,
    pub tag: u32,
}

#[repr(C)]
pub struct CbParamDesc {
    pub name: *const std::ffi::c_char,
    pub ty: u32,
}

#[repr(C)]
pub struct CbFuncDesc {
    pub name: *const std::ffi::c_char,
    pub symbol: *const std::ffi::c_char,
    /// Statically-linked address of the runtime function. Stored as `Option`
    /// so the FFI struct mirrors C's nullable `void (*)(void)` exactly
    /// (Rust's `unsafe extern "C" fn` is non-null). `load_catalog` checks
    /// for null and returns a clear error.
    pub fn_ptr: Option<unsafe extern "C" fn()>,
    pub params: *const CbParamDesc,
    pub param_count: u32,
    pub return_type: u32,
    pub flags: u32,
}

#[repr(C)]
pub struct CbCatalog {
    pub version: u32,
    pub type_count: u32,
    pub types: *const CbTypeDesc,
    pub func_count: u32,
    pub funcs: *const CbFuncDesc,
}

// ── Constants ──────────────────────────────────────────────────────────

pub const CB_CATALOG_VERSION: u32 = 3;
const CB_TYPE_VOID: u32 = 0;
const CB_TYPE_BYTE: u32 = 1;
const CB_TYPE_SHORT: u32 = 2;
const CB_TYPE_INT: u32 = 3;
const CB_TYPE_UINT: u32 = 4;
const CB_TYPE_LONG: u32 = 5;
const CB_TYPE_ULONG: u32 = 6;
const CB_TYPE_FLOAT: u32 = 7;
const CB_TYPE_BOOL: u32 = 8;
const CB_TYPE_STRING: u32 = 9;

// ── Extern declarations ────────────────────────────────────────────────

// The catalog is the only entry point Rust needs from the runtime
// library — every other function is reached through the `fn_ptr` field
// on each catalog entry, dispatched via libffi by the interpreter.
unsafe extern "C" {
    pub fn cb_runtime_get_catalog() -> *const CbCatalog;
}

// ── Safe conversion ────────────────────────────────────────────────────

/// Load the runtime catalog and convert it to a `RuntimeCatalog` suitable
/// for passing to `cb_sema::analyze`.
pub fn load_catalog() -> Result<RuntimeCatalog, String> {
    let catalog_ptr = unsafe { cb_runtime_get_catalog() };
    if catalog_ptr.is_null() {
        return Err("cb_runtime_get_catalog() returned null".to_string());
    }

    let catalog = unsafe { &*catalog_ptr };
    if catalog.version != CB_CATALOG_VERSION {
        return Err(format!(
            "unsupported catalog version {} (expected {})",
            catalog.version, CB_CATALOG_VERSION
        ));
    }

    // Read type declarations first — needed to resolve tags in function signatures.
    let mut custom_types = Vec::new();
    let mut tag_to_name: HashMap<u32, String> = HashMap::new();

    if catalog.type_count > 0 {
        if catalog.types.is_null() {
            return Err("null types pointer with non-zero type_count".to_string());
        }
        let types_slice =
            unsafe { std::slice::from_raw_parts(catalog.types, catalog.type_count as usize) };

        for (i, td) in types_slice.iter().enumerate() {
            if td.name.is_null() {
                return Err(format!("null type name at index {i}"));
            }
            let name = unsafe { CStr::from_ptr(td.name) }
                .to_str()
                .map_err(|e| format!("invalid UTF-8 in type name at index {i}: {e}"))?
                .to_string();
            if td.tag < 10 {
                return Err(format!(
                    "runtime type '{name}' has reserved tag {} (must be >= 10)",
                    td.tag
                ));
            }
            tag_to_name.insert(td.tag, name.clone());
            custom_types.push(RuntimeTypeDesc {
                name,
                tag: td.tag,
            });
        }
    }

    // Read function descriptors.
    let funcs_slice = if catalog.func_count > 0 {
        if catalog.funcs.is_null() {
            return Err("null funcs pointer with non-zero func_count".to_string());
        }
        unsafe { std::slice::from_raw_parts(catalog.funcs, catalog.func_count as usize) }
    } else {
        &[]
    };

    let mut functions = Vec::with_capacity(funcs_slice.len());
    for func in funcs_slice {
        if func.name.is_null() {
            return Err(format!("null function name at index {}", functions.len()));
        }
        let name = unsafe { CStr::from_ptr(func.name) }
            .to_str()
            .map_err(|e| format!("invalid UTF-8 in function name: {e}"))?
            .to_string();

        if func.symbol.is_null() {
            return Err(format!("null symbol for function '{name}'"));
        }
        let symbol = unsafe { CStr::from_ptr(func.symbol) }
            .to_str()
            .map_err(|e| format!("invalid UTF-8 in symbol for {name}: {e}"))?
            .to_string();

        let fn_ptr = func
            .fn_ptr
            .ok_or_else(|| format!("null fn_ptr for function '{name}' (symbol: {symbol})"))?;

        let params = if func.param_count > 0 {
            if func.params.is_null() {
                return Err(format!("null params pointer for function '{name}'"));
            }
            let params_slice =
                unsafe { std::slice::from_raw_parts(func.params, func.param_count as usize) };
            params_slice
                .iter()
                .map(|p| {
                    let param_name = if p.name.is_null() {
                        None
                    } else {
                        Some(
                            unsafe { CStr::from_ptr(p.name) }
                                .to_str()
                                .unwrap_or("_")
                                .to_string(),
                        )
                    };
                    Ok(FuncParamDesc {
                        name: param_name,
                        ty: type_tag_to_ir_type(p.ty, &tag_to_name)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            Vec::new()
        };

        let return_ty = type_tag_to_ir_type(func.return_type, &tag_to_name)?;

        functions.push(FuncDesc {
            name,
            c_symbol: symbol,
            fn_ptr,
            params,
            return_ty,
        });
    }

    Ok(RuntimeCatalog {
        types: custom_types,
        functions,
    })
}

fn type_tag_to_ir_type(tag: u32, custom_types: &HashMap<u32, String>) -> Result<IrType, String> {
    match tag {
        CB_TYPE_VOID => Ok(IrType::Void),
        CB_TYPE_BYTE => Ok(IrType::Byte),
        CB_TYPE_SHORT => Ok(IrType::Short),
        CB_TYPE_INT => Ok(IrType::Int),
        CB_TYPE_UINT => Ok(IrType::UInt),
        CB_TYPE_LONG => Ok(IrType::Long),
        CB_TYPE_ULONG => Ok(IrType::ULong),
        CB_TYPE_FLOAT => Ok(IrType::Float),
        CB_TYPE_BOOL => Ok(IrType::Bool),
        CB_TYPE_STRING => Ok(IrType::String),
        other => {
            if let Some(name) = custom_types.get(&other) {
                Ok(IrType::RuntimeType(name.clone()))
            } else {
                Err(format!("unknown type tag: {other}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_catalog_returns_expected_entries() {
        let catalog = load_catalog().expect("catalog should load");

        // Type declarations.
        assert_eq!(catalog.types.len(), 1);
        assert_eq!(catalog.types[0].name, "TestHandle");
        assert_eq!(catalog.types[0].tag, 10);

        // Every entry must have a non-null fn_ptr; the C++ CB_FN macro
        // makes this a linker-checked invariant.
        for func in &catalog.functions {
            let _: unsafe extern "C" fn() = func.fn_ptr; // type-asserts non-null
            assert!(!func.c_symbol.is_empty(), "entry '{}' has empty c_symbol", func.name);
        }

        // Look up by c_symbol so adding new runtime functions doesn't break
        // existing assertions on position.
        let by_symbol: std::collections::HashMap<&str, &FuncDesc> = catalog
            .functions
            .iter()
            .map(|f| (f.c_symbol.as_str(), f))
            .collect();

        let print = by_symbol["cb_rt_print"];
        assert_eq!(print.name, "print");
        assert_eq!(print.params.len(), 1);
        assert_eq!(print.params[0].ty, IrType::String);
        assert_eq!(print.return_ty, IrType::Void);

        let abs_int = by_symbol["cb_rt_abs_int"];
        assert_eq!(abs_int.name, "abs");
        assert_eq!(abs_int.params[0].ty, IrType::Int);
        assert_eq!(abs_int.return_ty, IrType::Int);

        let abs_float = by_symbol["cb_rt_abs_float"];
        assert_eq!(abs_float.name, "abs");
        assert_eq!(abs_float.params[0].ty, IrType::Float);
        assert_eq!(abs_float.return_ty, IrType::Float);

        let screen = by_symbol["cb_rt_screen"];
        assert_eq!(screen.name, "screen");
        assert_eq!(screen.params.len(), 2);

        assert_eq!(by_symbol["cb_rt_drawscreen"].name, "drawscreen");
        assert_eq!(by_symbol["cb_rt_color"].params.len(), 3);
        assert_eq!(by_symbol["cb_rt_line"].params.len(), 4);

        let create = by_symbol["cb_rt_create_test_handle"];
        assert_eq!(create.name, "createtesthandle");
        assert_eq!(create.params.len(), 0);
        assert_eq!(create.return_ty, IrType::RuntimeType("TestHandle".to_string()));

        let use_h = by_symbol["cb_rt_use_test_handle"];
        assert_eq!(use_h.name, "usetesthandle");
        assert_eq!(use_h.params[0].ty, IrType::RuntimeType("TestHandle".to_string()));
        assert_eq!(use_h.return_ty, IrType::Int);
    }
}
