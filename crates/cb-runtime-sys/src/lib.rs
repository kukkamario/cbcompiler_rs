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

pub const CB_CATALOG_VERSION: u32 = 2;
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

        // Type declarations
        assert_eq!(catalog.types.len(), 1);
        assert_eq!(catalog.types[0].name, "TestHandle");
        assert_eq!(catalog.types[0].tag, 10);

        // Functions
        assert_eq!(catalog.functions.len(), 5);

        assert_eq!(catalog.functions[0].name, "print");
        assert_eq!(catalog.functions[0].c_symbol, "cb_rt_print");
        assert_eq!(catalog.functions[0].params.len(), 1);
        assert_eq!(catalog.functions[0].params[0].ty, IrType::String);
        assert_eq!(catalog.functions[0].return_ty, IrType::Void);

        assert_eq!(catalog.functions[1].name, "abs");
        assert_eq!(catalog.functions[1].c_symbol, "cb_rt_abs_int");
        assert_eq!(catalog.functions[1].params[0].ty, IrType::Int);
        assert_eq!(catalog.functions[1].return_ty, IrType::Int);

        assert_eq!(catalog.functions[2].name, "abs");
        assert_eq!(catalog.functions[2].c_symbol, "cb_rt_abs_float");
        assert_eq!(catalog.functions[2].params[0].ty, IrType::Float);
        assert_eq!(catalog.functions[2].return_ty, IrType::Float);

        // Test handle functions use runtime type
        assert_eq!(catalog.functions[3].name, "createtesthandle");
        assert_eq!(catalog.functions[3].params.len(), 0);
        assert_eq!(
            catalog.functions[3].return_ty,
            IrType::RuntimeType("TestHandle".to_string())
        );

        assert_eq!(catalog.functions[4].name, "usetesthandle");
        assert_eq!(
            catalog.functions[4].params[0].ty,
            IrType::RuntimeType("TestHandle".to_string())
        );
        assert_eq!(catalog.functions[4].return_ty, IrType::Int);
    }
}
