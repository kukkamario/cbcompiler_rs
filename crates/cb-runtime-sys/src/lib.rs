//! Thin FFI bindings to the CoolBasic C runtime library.
//!
//! Compiles the C runtime via `build.rs` (using the `cc` crate), provides
//! `#[repr(C)]` mirror types for the catalog ABI, and a safe conversion
//! function that produces `Vec<FuncDesc>` for use by sema.

use std::ffi::CStr;

use cb_sema::{FuncDesc, FuncParamDesc, Type};

// ── C ABI mirror types ─────────────────────────────────────────────────

#[repr(C)]
pub struct CbParamDesc {
    pub name: *const std::ffi::c_char,
    pub ty: u32,
}

#[repr(C)]
pub struct CbFuncDesc {
    pub name: *const std::ffi::c_char,
    pub symbol: *const std::ffi::c_char,
    pub params: *const CbParamDesc,
    pub param_count: u32,
    pub return_type: u32,
    pub flags: u32,
}

#[repr(C)]
pub struct CbCatalog {
    pub version: u32,
    pub func_count: u32,
    pub funcs: *const CbFuncDesc,
}

// ── Constants ──────────────────────────────────────────────────────────

pub const CB_CATALOG_VERSION: u32 = 1;
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

unsafe extern "C" {
    pub fn cb_runtime_get_catalog() -> *const CbCatalog;
}

// ── Safe conversion ────────────────────────────────────────────────────

/// Load the runtime catalog and convert it to a `Vec<FuncDesc>` suitable
/// for passing to `cb_sema::analyze`.
pub fn load_catalog() -> Result<Vec<FuncDesc>, String> {
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

    let funcs_slice =
        unsafe { std::slice::from_raw_parts(catalog.funcs, catalog.func_count as usize) };

    let mut result = Vec::with_capacity(funcs_slice.len());
    for func in funcs_slice {
        let name = unsafe { CStr::from_ptr(func.name) }
            .to_str()
            .map_err(|e| format!("invalid UTF-8 in function name: {e}"))?
            .to_string();

        let symbol = unsafe { CStr::from_ptr(func.symbol) }
            .to_str()
            .map_err(|e| format!("invalid UTF-8 in symbol for {name}: {e}"))?
            .to_string();

        let params_slice =
            unsafe { std::slice::from_raw_parts(func.params, func.param_count as usize) };

        let params = params_slice
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
                    ty: type_tag_to_type(p.ty)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let return_ty = type_tag_to_type(func.return_type)?;

        result.push(FuncDesc {
            name,
            c_symbol: symbol,
            params,
            return_ty,
        });
    }

    Ok(result)
}

fn type_tag_to_type(tag: u32) -> Result<Type, String> {
    match tag {
        CB_TYPE_VOID => Ok(Type::Void),
        CB_TYPE_BYTE => Ok(Type::Byte),
        CB_TYPE_SHORT => Ok(Type::Short),
        CB_TYPE_INT => Ok(Type::Int),
        CB_TYPE_UINT => Ok(Type::UInt),
        CB_TYPE_LONG => Ok(Type::Long),
        CB_TYPE_ULONG => Ok(Type::ULong),
        CB_TYPE_FLOAT => Ok(Type::Float),
        CB_TYPE_BOOL => Ok(Type::Bool),
        CB_TYPE_STRING => Ok(Type::String),
        other => Err(format!("unknown type tag: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_catalog_returns_expected_entries() {
        let catalog = load_catalog().expect("catalog should load");
        assert_eq!(catalog.len(), 3);

        assert_eq!(catalog[0].name, "print");
        assert_eq!(catalog[0].c_symbol, "cb_rt_print");
        assert_eq!(catalog[0].params.len(), 1);
        assert_eq!(catalog[0].params[0].ty, Type::String);
        assert_eq!(catalog[0].return_ty, Type::Void);

        assert_eq!(catalog[1].name, "abs");
        assert_eq!(catalog[1].c_symbol, "cb_rt_abs_int");
        assert_eq!(catalog[1].params[0].ty, Type::Int);
        assert_eq!(catalog[1].return_ty, Type::Int);

        assert_eq!(catalog[2].name, "abs");
        assert_eq!(catalog[2].c_symbol, "cb_rt_abs_float");
        assert_eq!(catalog[2].params[0].ty, Type::Float);
        assert_eq!(catalog[2].return_ty, Type::Float);
    }
}
