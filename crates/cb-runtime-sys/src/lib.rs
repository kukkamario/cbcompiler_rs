//! Thin FFI bindings to the CoolBasic C runtime library.
//!
//! Compiles the C runtime via `build.rs` (using the `cc` crate), provides
//! `#[repr(C)]` mirror types for the catalog ABI, and a safe conversion
//! function that produces a `RuntimeCatalog` for use by sema.

use std::collections::{BTreeSet, HashMap};
use std::ffi::CStr;

use cb_ir::types::IrType;
use cb_ir::{
    FuncDesc, FuncParamDesc, RuntimeCatalog, RuntimeConstDesc, RuntimeConstValue, RuntimeTypeDesc,
};

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
    /// Catalog flag bits (`CB_FUNC_CAN_TRAP`, …). Intentionally **not** decoded
    /// into `FuncDesc`: the interpreter drains the trap channel after every call
    /// regardless, so it has no consumer today. That rationale is interp-specific
    /// — a future LLVM backend that wants to skip the trap check on calls without
    /// `CB_FUNC_CAN_TRAP` (rather than draining unconditionally) will need to
    /// decode `flags` here. Kept in the `repr(C)` mirror so the layout matches C
    /// exactly; the static layout assert below pins it.
    pub flags: u32,
}

/// A runtime-defined global constant (FD-029, catalog v6). Mirrors C
/// `CbConstDesc`. The C side stores the value in a `union { int64_t i; double f; }`;
/// we mirror it as raw bits (`value_bits`) and decode per `tag`, which keeps
/// this module free of Rust union field access.
#[repr(C)]
pub struct CbConstDesc {
    pub name: *const std::ffi::c_char,
    pub tag: u32,
    pub value_bits: u64,
}

#[repr(C)]
pub struct CbCatalog {
    pub version: u32,
    pub type_count: u32,
    pub types: *const CbTypeDesc,
    pub func_count: u32,
    pub funcs: *const CbFuncDesc,
    pub const_count: u32,
    pub consts: *const CbConstDesc,
    /// Backend-only API for the primitive String type. Always non-null
    /// in catalog v4+. See `CbStringApi` below.
    pub strings: *const CbStringApi,
}

/// Opaque CbString handle. The C side knows the full layout; consumers
/// here read length and bytes through `CbStringApi::len` / `::data` and
/// never reach into the struct directly.
#[repr(C)]
pub struct CbString {
    _private: [u8; 0],
}

/// String primitive API exposed through `CbCatalog::strings`. Function
/// pointers are non-null in v4+; `empty` points to the global immortal
/// empty-string sentinel.
#[repr(C)]
pub struct CbStringApi {
    pub retain: unsafe extern "C" fn(*mut CbString) -> *mut CbString,
    pub release: unsafe extern "C" fn(*mut CbString),
    pub from_literal: unsafe extern "C" fn(*const u8, usize) -> *mut CbString,
    pub len: unsafe extern "C" fn(*const CbString) -> usize,
    pub data: unsafe extern "C" fn(*const CbString) -> *const u8,
    pub concat: unsafe extern "C" fn(*const CbString, *const CbString) -> *mut CbString,
    pub empty: *const CbString,
}

/// Host API delivered to the runtime at startup via [`runtime_init`] — the
/// Runtime Trap Channel (FD-015, catalog v5). The runtime calls these to ask
/// the host to terminate cleanly or raise a runtime error; each callback
/// records the intent and returns (it never unwinds the C frame). `size` /
/// `abi_version` are caller-set ABI guards. Mirrors C `CbHostApi`.
#[repr(C)]
pub struct CbHostApi {
    pub size: u32,
    pub abi_version: u32,
    pub request_exit: extern "C" fn(i32),
    pub raise_error: extern "C" fn(*const CbString),
}

/// Hook table the runtime returns from [`runtime_init`] — things the host
/// calls *on* the runtime. `about_to_exit` is reserved (null) for now.
/// Mirrors C `CbRuntimeHooks`.
#[repr(C)]
pub struct CbRuntimeHooks {
    pub size: u32,
    pub about_to_exit: Option<extern "C" fn()>,
}

// ── ABI layout pins (FD-024) ───────────────────────────────────────────
// Mirror the C `static_assert`s in runtime/cb_runtime_core.h. Any drift in
// either the Rust mirror or the C struct fails the build on its own side
// before a mismatched layout can cross the FFI boundary. Offsets guard the
// trailing fields most likely to drift (flags, the strings tail pointer).
const _: () = {
    assert!(std::mem::size_of::<CbHostApi>() == 24);
    assert!(std::mem::size_of::<CbRuntimeHooks>() == 16);
    assert!(std::mem::size_of::<CbFuncDesc>() == 48);
    assert!(std::mem::size_of::<CbCatalog>() == 56);
    assert!(std::mem::offset_of!(CbFuncDesc, flags) == 40);
    assert!(std::mem::offset_of!(CbCatalog, strings) == 48);
};

// ── Constants ──────────────────────────────────────────────────────────

pub const CB_CATALOG_VERSION: u32 = 6;

/// Whether the linked runtime includes the Allegro-backed graphics, text, and
/// input functions. `false` in the SDK-free build (FD-033), where `build.rs`
/// could not (or was told not to) build the full Allegro runtime and compiled
/// only the language-core TUs. Tests and tools use this to skip graphics-
/// dependent work cleanly instead of failing on absent catalog entries.
pub const HAS_GRAPHICS: bool = cfg!(not(cb_no_allegro));

/// Host trap-channel ABI version (FD-015/FD-024). Mirrors C `CB_HOST_ABI_VERSION`.
/// Versions the `CbHostApi`/`CbRuntimeHooks` handshake independently of the
/// catalog data format — see [`runtime_init`].
pub const CB_HOST_ABI_VERSION: u32 = 1;
const CB_TYPE_VOID: u32 = 0;
const CB_TYPE_BYTE: u32 = 1;
const CB_TYPE_SHORT: u32 = 2;
const CB_TYPE_INT: u32 = 3;
// UInt/ULong/Bool are reserved wire codes for types the language no longer
// supports (FD-035). Kept numerically stable for ABI compatibility with the
// C++ runtime header; decoding one is a hard error (no catalog entry uses them).
const CB_TYPE_UINT: u32 = 4;
const CB_TYPE_LONG: u32 = 5;
const CB_TYPE_ULONG: u32 = 6;
const CB_TYPE_FLOAT: u32 = 7;
const CB_TYPE_BOOL: u32 = 8;
const CB_TYPE_STRING: u32 = 9;

// ── Extern declarations ────────────────────────────────────────────────

// The catalog is the primary entry point Rust needs from the runtime library.
// Sema reads it as pure metadata ([`load_catalog`]); the interpreter
// additionally resolves each entry's `fn_ptr` ([`resolve_bindings`]) and
// dispatches through it via libffi. Splitting the two is what lets a
// metadata-only compiler avoid link-depending on the executable runtime (FD-045).
unsafe extern "C" {
    pub fn cb_runtime_get_catalog() -> *const CbCatalog;

    /// Metadata-only catalog entry point (FD-045): the same catalog data with
    /// null `fn_ptr`s, exported by a tiny Allegro-free object that references no
    /// runtime function body. [`load_catalog`] reads this, so sema / a native
    /// backend can type-check without linking the executable runtime.
    pub fn cb_runtime_get_catalog_meta() -> *const CbCatalog;

    /// Instrumentation hook — returns the current refcount of a CbString,
    /// or a negative value for the static-data sentinel. Used by tests to
    /// verify retain/release lifecycle; not part of `CbStringApi`.
    pub fn cb_rt_string_test_refcount(s: *const CbString) -> i32;

    /// Runtime Trap Channel handshake (FD-015): hand the runtime its host API
    /// and receive the hook table back. See [`runtime_init`] for the safe
    /// wrapper. Each plugin DLL exports this alongside `cb_runtime_get_catalog`.
    pub fn cb_runtime_init(host: *const CbHostApi) -> *const CbRuntimeHooks;
}

/// Fetch the catalog pointer the linked runtime exports and null-check it.
/// The single place the raw `cb_runtime_get_catalog()` FFI result is turned
/// into a safe reference; the full-catalog consumers ([`string_api`],
/// [`resolve_bindings`]) start here so the null-check (and its message) live
/// in exactly one spot (DR-R2). Sema's metadata path reads the sibling
/// [`fetch_catalog_meta`] instead (FD-045).
///
/// # Safety
/// Relies on the runtime returning either null or a pointer valid for
/// `'static` — the runtime's static catalog satisfies this.
fn fetch_catalog() -> Result<&'static CbCatalog, String> {
    let ptr = unsafe { cb_runtime_get_catalog() };
    if ptr.is_null() {
        return Err("cb_runtime_get_catalog() returned null".to_string());
    }
    Ok(unsafe { &*ptr })
}

/// Fetch the metadata-only catalog pointer (FD-045) and null-check it. Mirrors
/// [`fetch_catalog`] but reads `cb_runtime_get_catalog_meta()` — the catalog
/// compiled with null `fn_ptr`s and no executable-runtime link dependency. This
/// is the catalog sema sees.
fn fetch_catalog_meta() -> Result<&'static CbCatalog, String> {
    let ptr = unsafe { cb_runtime_get_catalog_meta() };
    if ptr.is_null() {
        return Err("cb_runtime_get_catalog_meta() returned null".to_string());
    }
    Ok(unsafe { &*ptr })
}

/// Validate a catalog's reported version against the version this crate was
/// built against. One message, shared by the fatal-by-panic [`string_api`]
/// startup path and the recoverable [`decode_catalog`] path so the two cannot
/// drift (DR-R3).
fn check_catalog_version(version: u32) -> Result<(), String> {
    if version != CB_CATALOG_VERSION {
        return Err(format!(
            "unsupported catalog version {version} (expected {CB_CATALOG_VERSION})"
        ));
    }
    Ok(())
}

/// Get the string API exposed by the loaded runtime catalog.
///
/// Unlike [`load_catalog`], which surfaces the same null/version conditions as
/// `Err` for the driver's diagnostic flow, this **panics**: a missing or
/// version-mismatched catalog at the point the interpreter needs the string
/// runtime is an unrecoverable startup misconfiguration (fatal-by-panic at
/// init, FD-024) that should surface immediately rather than be absorbed. The
/// version check is shared with [`load_catalog`]'s decode path via
/// [`check_catalog_version`] so the two cannot drift; the fetch reads the
/// **full** catalog ([`fetch_catalog`]) — the one carrying the string API —
/// whereas sema's [`load_catalog`] reads the metadata-only catalog (FD-045).
pub fn string_api() -> &'static CbStringApi {
    let catalog = fetch_catalog().unwrap_or_else(|e| panic!("{e}"));
    check_catalog_version(catalog.version).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !catalog.strings.is_null(),
        "catalog v{CB_CATALOG_VERSION} has null string API (runtime bug)"
    );
    unsafe { &*catalog.strings }
}

/// Deliver the host API to the runtime (the Runtime Trap Channel handshake,
/// FD-015) and return the hook table the runtime wants connected. Call once at
/// interpreter startup, before any runtime function runs. `host` must outlive
/// every runtime call — pass a `&'static`.
///
/// Returns `Err` if the runtime declines the handshake (`cb_runtime_init`
/// returns null — e.g. it rejected our `size`/`abi_version`) or hands back a
/// hook table too small for our [`CbRuntimeHooks`] mirror (ABI-incompatible).
/// Both are fatal startup misconfigurations the caller should surface, not
/// absorb.
pub fn runtime_init(host: &'static CbHostApi) -> Result<&'static CbRuntimeHooks, String> {
    let hooks = unsafe { cb_runtime_init(host) };
    if hooks.is_null() {
        return Err(
            "cb_runtime_init declined the handshake (returned null — host ABI rejected)"
                .to_string(),
        );
    }
    let hooks = unsafe { &*hooks };
    let min = std::mem::size_of::<CbRuntimeHooks>();
    if (hooks.size as usize) < min {
        return Err(format!(
            "runtime hook table too small: reported size {} < expected {min} (ABI mismatch)",
            hooks.size
        ));
    }
    Ok(hooks)
}

// ── Safe conversion ────────────────────────────────────────────────────

/// Load the runtime catalog (as **metadata only**, FD-045) and convert it to a
/// `RuntimeCatalog` suitable for passing to `cb_sema::analyze`. Thin wrapper:
/// fetch + null-check the metadata catalog pointer via [`fetch_catalog_meta`],
/// then hand it to [`decode_catalog`], which holds all version/layout validation
/// and is unit-testable against hand-built fixtures. The interpreter overlays
/// live function pointers separately via [`resolve_bindings`].
pub fn load_catalog() -> Result<RuntimeCatalog, String> {
    decode_catalog(fetch_catalog_meta()?)
}

/// Resolve the `symbol → fn_ptr` bindings the interpreter needs to dispatch
/// runtime calls (FD-045). Reads the **full** linked runtime catalog
/// (`cb_runtime_get_catalog()`), which carries live function pointers — unlike
/// the metadata-only catalog [`load_catalog`] decodes. Every entry must have a
/// non-null `fn_ptr`; a null here means the executable runtime was not linked,
/// a fatal interpreter-startup misconfiguration the caller should surface.
///
/// This is interp-only: a metadata-only compiler (sema, a native/AOT backend)
/// never calls it and so never link-depends on the executable runtime.
pub fn resolve_bindings() -> Result<HashMap<String, unsafe extern "C" fn()>, String> {
    let catalog = fetch_catalog()?;
    check_catalog_version(catalog.version)?;

    let funcs = if catalog.func_count > 0 {
        if catalog.funcs.is_null() {
            return Err("null funcs pointer with non-zero func_count".to_string());
        }
        unsafe { std::slice::from_raw_parts(catalog.funcs, catalog.func_count as usize) }
    } else {
        &[]
    };

    let mut bindings = HashMap::with_capacity(funcs.len());
    for func in funcs {
        if func.symbol.is_null() {
            return Err("null symbol in runtime catalog binding".to_string());
        }
        let symbol = unsafe { CStr::from_ptr(func.symbol) }
            .to_str()
            .map_err(|e| format!("invalid UTF-8 in symbol: {e}"))?
            .to_string();
        let fn_ptr = func.fn_ptr.ok_or_else(|| {
            format!("null fn_ptr for runtime function '{symbol}' (executable runtime not linked?)")
        })?;
        bindings.insert(symbol, fn_ptr);
    }
    Ok(bindings)
}

/// Like [`resolve_bindings`], but first reconciles the **metadata** catalog
/// ([`load_catalog`]'s source) against the **full** binding catalog and fails if
/// they have drifted (FD-045 Phase C drift guard).
///
/// Once metadata and bindings come from independently-built objects, the
/// compile-time `#fn` symbol↔pointer tie that `CB_FN` used to guarantee no
/// longer holds. The interpreter calls this at startup so any mismatch — a
/// metadata symbol with no binding (or vice-versa), or a signature that differs
/// between the two catalogs — aborts loudly rather than miscalling. Fatal-by-panic
/// at the call site, matching [`string_api`] / [`runtime_init`].
pub fn resolve_bindings_checked() -> Result<HashMap<String, unsafe extern "C" fn()>, String> {
    let meta = decode_catalog(fetch_catalog_meta()?)?;
    let full = decode_catalog(fetch_catalog()?)?;
    reconcile_catalogs(&meta, &full)?;
    resolve_bindings()
}

/// A stable, comparable key for one runtime function: its CB name, linker
/// symbol, parameter type tags, and return type tag. Two catalogs that produce
/// the same sorted key set describe identical signatures.
fn signature_keys(catalog: &RuntimeCatalog) -> Vec<String> {
    let mut keys: Vec<String> = catalog
        .functions
        .iter()
        .map(|f| {
            let params: Vec<String> = f.params.iter().map(|p| format!("{:?}", p.ty)).collect();
            format!(
                "{}|{}|{}|{:?}",
                f.name,
                f.c_symbol,
                params.join(","),
                f.return_ty
            )
        })
        .collect();
    keys.sort();
    keys
}

/// Structurally compare the metadata catalog against the binding (full) catalog
/// (FD-045 Phase C). Returns `Err` describing the first drift found: a symbol
/// present in one catalog but not the other, or a function whose signature
/// differs between them. Pure over its inputs so it is unit-testable with
/// hand-built catalogs (the live catalogs match by construction).
fn reconcile_catalogs(meta: &RuntimeCatalog, full: &RuntimeCatalog) -> Result<(), String> {
    // Symbol-set reconciliation: every metadata symbol must have a binding and
    // vice-versa.
    let meta_syms: BTreeSet<&str> = meta.functions.iter().map(|f| f.c_symbol.as_str()).collect();
    let full_syms: BTreeSet<&str> = full.functions.iter().map(|f| f.c_symbol.as_str()).collect();
    if meta_syms != full_syms {
        let missing: Vec<&str> = meta_syms.difference(&full_syms).copied().collect();
        let extra: Vec<&str> = full_syms.difference(&meta_syms).copied().collect();
        return Err(format!(
            "catalog drift (FD-045): metadata and runtime symbol sets differ — \
             metadata-only symbols: {missing:?}, runtime-only symbols: {extra:?}"
        ));
    }

    // Signature reconciliation: identical (name, symbol, params, return) tuples.
    // Catches drift the symbol-set check alone misses (e.g. a changed param type).
    let meta_keys = signature_keys(meta);
    let full_keys = signature_keys(full);
    if meta_keys != full_keys {
        let diff = meta_keys
            .iter()
            .zip(&full_keys)
            .find(|(a, b)| a != b)
            .map(|(a, b)| format!("metadata `{a}` vs runtime `{b}`"))
            .unwrap_or_else(|| "function count differs between catalogs".to_string());
        return Err(format!("catalog signature drift (FD-045): {diff}"));
    }

    Ok(())
}

/// Decode a `CbCatalog` into a `RuntimeCatalog`, validating version, pointers,
/// tags, UTF-8, and type-tag uniqueness (function names, symbols, and constant
/// names are deliberately allowed to collide). Split out from [`load_catalog`] so the
/// defensive branches can be exercised by tests that build malformed catalogs
/// directly (the real linked runtime always returns a valid one).
///
/// # Safety-adjacent contract
/// `catalog` and every pointer it transitively references (name strings, the
/// type/func/const/param arrays) must be valid for the duration of the call.
/// The real caller satisfies this with the runtime's static catalog; tests
/// satisfy it by keeping their fixture backing data alive across the call.
fn decode_catalog(catalog: &CbCatalog) -> Result<RuntimeCatalog, String> {
    check_catalog_version(catalog.version)?;

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
            if tag_to_name.insert(td.tag, name.clone()).is_some() {
                return Err(format!(
                    "duplicate runtime type tag {} (type '{name}' collides with an earlier type)",
                    td.tag
                ));
            }
            custom_types.push(RuntimeTypeDesc { name, tag: td.tag });
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
                                .map_err(|e| {
                                    format!(
                                        "invalid UTF-8 in param name for function '{name}': {e}"
                                    )
                                })?
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

    // Read constant declarations (catalog v6+).
    let mut constants = Vec::new();
    if catalog.const_count > 0 {
        if catalog.consts.is_null() {
            return Err("null consts pointer with non-zero const_count".to_string());
        }
        let consts_slice =
            unsafe { std::slice::from_raw_parts(catalog.consts, catalog.const_count as usize) };
        for (i, cd) in consts_slice.iter().enumerate() {
            if cd.name.is_null() {
                return Err(format!("null constant name at index {i}"));
            }
            let name = unsafe { CStr::from_ptr(cd.name) }
                .to_str()
                .map_err(|e| format!("invalid UTF-8 in constant name at index {i}: {e}"))?
                .to_string();
            // The C union holds either an int64 or a double; decode by tag.
            // Only Int and Float are supported (see CbConstDesc / FD-029).
            let (ty, value) = match cd.tag {
                CB_TYPE_INT => (IrType::Int, RuntimeConstValue::Int(cd.value_bits as i64)),
                CB_TYPE_FLOAT => (
                    IrType::Float,
                    RuntimeConstValue::Float(f64::from_bits(cd.value_bits)),
                ),
                other => {
                    return Err(format!(
                        "constant '{name}' has unsupported type tag {other} (only Int/Float)"
                    ));
                }
            };
            constants.push(RuntimeConstDesc { name, ty, value });
        }
    }

    Ok(RuntimeCatalog {
        types: custom_types,
        functions,
        constants,
    })
}

fn type_tag_to_ir_type(tag: u32, custom_types: &HashMap<u32, String>) -> Result<IrType, String> {
    match tag {
        CB_TYPE_VOID => Ok(IrType::Void),
        CB_TYPE_BYTE => Ok(IrType::Byte),
        CB_TYPE_SHORT => Ok(IrType::Short),
        CB_TYPE_INT => Ok(IrType::Int),
        CB_TYPE_LONG => Ok(IrType::Long),
        CB_TYPE_FLOAT => Ok(IrType::Float),
        CB_TYPE_STRING => Ok(IrType::String),
        // Reserved, unsupported types (FD-035). The wire codes stay stable for
        // ABI compatibility, but no catalog entry uses them, so decoding one is
        // a hard error rather than a silent fallback.
        CB_TYPE_UINT | CB_TYPE_ULONG | CB_TYPE_BOOL => {
            Err(format!("type tag {tag} is a reserved, unsupported type"))
        }
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

        // Type declarations: TestHandle (tag 10), the Image graphics handle
        // (tag 11, FD-013 Batch 4), and the Font handle (tag 12, FD-018).
        let types_by_name: std::collections::HashMap<&str, &RuntimeTypeDesc> =
            catalog.types.iter().map(|t| (t.name.as_str(), t)).collect();
        assert_eq!(types_by_name["TestHandle"].tag, 10);
        // Memblock (FD-039, tag 15) and File (FD-040, tag 16) are Allegro-free,
        // so they are advertised in BOTH the full and SDK-free catalogs (unlike
        // the graphics handles below).
        assert_eq!(types_by_name["Memblock"].tag, 15);
        assert_eq!(types_by_name["File"].tag, 16);
        // Image/Font (and the graphics/input functions below) exist only in the
        // full Allegro build; the SDK-free catalog (FD-033) advertises just
        // TestHandle, Memblock, File, and the language-core functions.
        #[cfg(not(cb_no_allegro))]
        {
            assert_eq!(catalog.types.len(), 9);
            assert_eq!(types_by_name["Image"].tag, 11);
            assert_eq!(types_by_name["Font"].tag, 12);
            // Object is tag 13 (FD-036 Phase 4); Map is tag 14 (Phase 3).
            assert_eq!(types_by_name["Object"].tag, 13);
            assert_eq!(types_by_name["Map"].tag, 14);
            // Sound is tag 17, SoundChannel tag 18 (FD-041). Both Allegro-
            // dependent, so they exist only in the full build. The CB-visible
            // name is "SoundChannel", not "Channel".
            assert_eq!(types_by_name["Sound"].tag, 17);
            assert_eq!(types_by_name["SoundChannel"].tag, 18);
        }
        #[cfg(cb_no_allegro)]
        assert_eq!(catalog.types.len(), 3);

        // Metadata carries no fn_ptr (FD-045); sanity-check the symbol instead.
        for func in &catalog.functions {
            assert!(
                !func.c_symbol.is_empty(),
                "entry '{}' has empty c_symbol",
                func.name
            );
        }

        // The interpreter's binding overlay must resolve a non-null fn_ptr for
        // every catalog symbol — the C++ CB_FN macro ties `#fn` to the pointer,
        // and resolve_bindings() reads them from the linked executable runtime.
        let bindings = resolve_bindings().expect("resolve_bindings");
        for func in &catalog.functions {
            assert!(
                bindings.contains_key(&func.c_symbol),
                "no fn_ptr binding for symbol '{}'",
                func.c_symbol
            );
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

        // Memory blocks (FD-039). Allegro-free, so present in BOTH builds. The
        // `Memblock` handle is the tag-15 opaque type; Peek*/Poke* take it +
        // an Int offset, Float peek/poke use the CB Float (f64) end.
        let memblock_ty = IrType::RuntimeType("Memblock".to_string());

        let make_mb = by_symbol["cb_rt_make_memblock"];
        assert_eq!(make_mb.name, "makememblock");
        assert_eq!(make_mb.params.len(), 1);
        assert_eq!(make_mb.params[0].ty, IrType::Int);
        assert_eq!(make_mb.return_ty, memblock_ty);

        let mb_size = by_symbol["cb_rt_memblock_size"];
        assert_eq!(mb_size.name, "memblocksize");
        assert_eq!(mb_size.params[0].ty, memblock_ty);
        assert_eq!(mb_size.return_ty, IrType::Int);

        let mem_copy = by_symbol["cb_rt_mem_copy"];
        assert_eq!(mem_copy.name, "memcopy");
        assert_eq!(mem_copy.params.len(), 5);
        assert_eq!(mem_copy.params[0].ty, memblock_ty);
        assert_eq!(mem_copy.params[1].ty, IrType::Int);
        assert_eq!(mem_copy.params[2].ty, memblock_ty);
        assert_eq!(mem_copy.return_ty, IrType::Void);

        let peek_byte = by_symbol["cb_rt_peek_byte"];
        assert_eq!(peek_byte.name, "peekbyte");
        assert_eq!(peek_byte.params[0].ty, memblock_ty);
        assert_eq!(peek_byte.params[1].ty, IrType::Int);
        assert_eq!(peek_byte.return_ty, IrType::Int);

        let peek_float = by_symbol["cb_rt_peek_float"];
        assert_eq!(peek_float.name, "peekfloat");
        assert_eq!(peek_float.return_ty, IrType::Float);

        let poke_int = by_symbol["cb_rt_poke_int"];
        assert_eq!(poke_int.name, "pokeint");
        assert_eq!(poke_int.params.len(), 3);
        assert_eq!(poke_int.params[2].ty, IrType::Int);
        assert_eq!(poke_int.return_ty, IrType::Void);

        let poke_float = by_symbol["cb_rt_poke_float"];
        assert_eq!(poke_float.name, "pokefloat");
        assert_eq!(poke_float.params[2].ty, IrType::Float);
        assert_eq!(poke_float.return_ty, IrType::Void);

        // All 13 entry points must be registered (the remaining ones share the
        // shapes asserted above).
        for sym in [
            "cb_rt_delete_memblock",
            "cb_rt_resize_memblock",
            "cb_rt_peek_short",
            "cb_rt_peek_int",
            "cb_rt_poke_byte",
            "cb_rt_poke_short",
        ] {
            assert!(by_symbol.contains_key(sym), "missing memblock entry {sym}");
        }

        // File I/O (FD-040). Allegro-free, so present in BOTH builds. `File` is
        // the tag-16 opaque handle; OpenTo* return it, the read/write/query funcs
        // take it + Int/Float/String, and the filesystem funcs take String paths.
        let file_ty = IrType::RuntimeType("File".to_string());

        let open_read = by_symbol["cb_rt_open_to_read"];
        assert_eq!(open_read.name, "opentoread");
        assert_eq!(open_read.params.len(), 1);
        assert_eq!(open_read.params[0].ty, IrType::String);
        assert_eq!(open_read.return_ty, file_ty);

        let read_byte = by_symbol["cb_rt_read_byte"];
        assert_eq!(read_byte.name, "readbyte");
        assert_eq!(read_byte.params[0].ty, file_ty);
        assert_eq!(read_byte.return_ty, IrType::Int);

        let read_float = by_symbol["cb_rt_read_float"];
        assert_eq!(read_float.name, "readfloat");
        assert_eq!(read_float.return_ty, IrType::Float);

        let read_string = by_symbol["cb_rt_read_string"];
        assert_eq!(read_string.name, "readstring");
        assert_eq!(read_string.params[0].ty, file_ty);
        assert_eq!(read_string.return_ty, IrType::String);

        let write_int = by_symbol["cb_rt_write_int"];
        assert_eq!(write_int.name, "writeint");
        assert_eq!(write_int.params.len(), 2);
        assert_eq!(write_int.params[0].ty, file_ty);
        assert_eq!(write_int.params[1].ty, IrType::Int);
        assert_eq!(write_int.return_ty, IrType::Void);

        let write_string = by_symbol["cb_rt_write_string"];
        assert_eq!(write_string.name, "writestring");
        assert_eq!(write_string.params[1].ty, IrType::String);

        let copy_file = by_symbol["cb_rt_copy_file"];
        assert_eq!(copy_file.name, "copyfile");
        assert_eq!(copy_file.params.len(), 2);
        assert_eq!(copy_file.params[0].ty, IrType::String);
        assert_eq!(copy_file.params[1].ty, IrType::String);

        let current_dir = by_symbol["cb_rt_current_dir"];
        assert_eq!(current_dir.name, "currentdir");
        assert_eq!(current_dir.params.len(), 0);
        assert_eq!(current_dir.return_ty, IrType::String);

        let find_file = by_symbol["cb_rt_find_file"];
        assert_eq!(find_file.name, "findfile");
        assert_eq!(find_file.return_ty, IrType::String);

        // All 31 entry points must be registered (the rest share shapes above).
        for sym in [
            "cb_rt_open_to_write",
            "cb_rt_open_to_edit",
            "cb_rt_close_file",
            "cb_rt_seek_file",
            "cb_rt_file_offset",
            "cb_rt_eof",
            "cb_rt_read_short",
            "cb_rt_read_int",
            "cb_rt_read_line",
            "cb_rt_write_byte",
            "cb_rt_write_short",
            "cb_rt_write_float",
            "cb_rt_write_line",
            "cb_rt_file_exists",
            "cb_rt_is_directory",
            "cb_rt_file_size",
            "cb_rt_chdir",
            "cb_rt_make_dir",
            "cb_rt_delete_file",
            "cb_rt_execute",
            "cb_rt_start_search",
            "cb_rt_end_search",
        ] {
            assert!(by_symbol.contains_key(sym), "missing file entry {sym}");
        }

        // Graphics + input entries are present only in the full Allegro build.
        #[cfg(not(cb_no_allegro))]
        {
            let screen = by_symbol["cb_rt_screen"];
            assert_eq!(screen.name, "screen");
            assert_eq!(screen.params.len(), 2);

            assert_eq!(by_symbol["cb_rt_drawscreen"].name, "drawscreen");
            assert_eq!(by_symbol["cb_rt_color"].params.len(), 3);
            assert_eq!(by_symbol["cb_rt_line"].params.len(), 4);

            // Game loop (FD-036 Phase 5): UpdateGame/DrawGame are 0-arg commands.
            let update_game = by_symbol["cb_rt_update_game"];
            assert_eq!(update_game.name, "updategame");
            assert_eq!(update_game.params.len(), 0);
            assert_eq!(update_game.return_ty, IrType::Void);
            let draw_game = by_symbol["cb_rt_draw_game"];
            assert_eq!(draw_game.name, "drawgame");
            assert_eq!(draw_game.params.len(), 0);
            assert_eq!(draw_game.return_ty, IrType::Void);

            // Graphics: Image opaque-handle plumbing (FD-013 Batch 4).
            let make_image = by_symbol["cb_rt_make_image"];
            assert_eq!(make_image.name, "makeimage");
            assert_eq!(
                make_image.return_ty,
                IrType::RuntimeType("Image".to_string())
            );

            let get_pixel = by_symbol["cb_rt_get_pixel"];
            assert_eq!(get_pixel.name, "getpixel");
            assert_eq!(
                get_pixel.params[0].ty,
                IrType::RuntimeType("Image".to_string())
            );
            assert_eq!(get_pixel.return_ty, IrType::Int);

            // FD-036 multi-frame sprite sheets. Each optional `frame`/`useMask`
            // arg is its own arity overload (the catalog has no default-arg
            // mechanism); LoadAnimImage returns the existing `Image` type.
            let image_ty = IrType::RuntimeType("Image".to_string());
            let load_anim = by_symbol["cb_rt_load_anim_image"];
            assert_eq!(load_anim.name, "loadanimimage");
            assert_eq!(load_anim.params.len(), 5);
            assert_eq!(load_anim.params[0].ty, IrType::String);
            assert_eq!(load_anim.params[1].ty, IrType::Int);
            assert_eq!(load_anim.return_ty, image_ty);

            let make_frames = by_symbol["cb_rt_make_image_frames"];
            assert_eq!(make_frames.name, "makeimage");
            assert_eq!(make_frames.params.len(), 3);
            assert_eq!(make_frames.return_ty, image_ty);

            // drawimage: 3-arg (existing), 4-arg (frame), 5-arg (frame+useMask).
            let draw_frame = by_symbol["cb_rt_draw_image_frame"];
            assert_eq!(draw_frame.name, "drawimage");
            assert_eq!(draw_frame.params.len(), 4);
            assert_eq!(draw_frame.params[0].ty, image_ty);
            assert_eq!(draw_frame.params[3].ty, IrType::Int);
            assert_eq!(draw_frame.return_ty, IrType::Void);

            let draw_frame_mask = by_symbol["cb_rt_draw_image_frame_mask"];
            assert_eq!(draw_frame_mask.name, "drawimage");
            assert_eq!(draw_frame_mask.params.len(), 5);

            let ghost_frame = by_symbol["cb_rt_draw_ghost_image_frame"];
            assert_eq!(ghost_frame.name, "drawghostimage");
            assert_eq!(ghost_frame.params.len(), 5);
            assert_eq!(ghost_frame.params[3].ty, IrType::Int);
            assert_eq!(ghost_frame.params[4].ty, IrType::Float);

            let box_frame = by_symbol["cb_rt_draw_image_box_frame"];
            assert_eq!(box_frame.name, "drawimagebox");
            assert_eq!(box_frame.params.len(), 8);
            assert_eq!(box_frame.params[7].ty, IrType::Int);

            let box_frame_mask = by_symbol["cb_rt_draw_image_box_frame_mask"];
            assert_eq!(box_frame_mask.name, "drawimagebox");
            assert_eq!(box_frame_mask.params.len(), 9);

            // Input: keyboard + mouse queries (FD-013 Batch 5). All are
            // Int->Int or ()->Int catalog entries dispatched generically.
            let key_down = by_symbol["cb_rt_key_down"];
            assert_eq!(key_down.name, "keydown");
            assert_eq!(key_down.params.len(), 1);
            assert_eq!(key_down.params[0].ty, IrType::Int);
            assert_eq!(key_down.return_ty, IrType::Int);

            let escape_key = by_symbol["cb_rt_escape_key"];
            assert_eq!(escape_key.name, "escapekey");
            assert_eq!(escape_key.params.len(), 0);
            assert_eq!(escape_key.return_ty, IrType::Int);

            let mouse_hit = by_symbol["cb_rt_mouse_hit"];
            assert_eq!(mouse_hit.name, "mousehit");
            assert_eq!(mouse_hit.params.len(), 1);
            assert_eq!(mouse_hit.params[0].ty, IrType::Int);
            assert_eq!(mouse_hit.return_ty, IrType::Int);

            assert_eq!(by_symbol["cb_rt_mouse_move_x"].name, "mousemovex");
            assert_eq!(by_symbol["cb_rt_mouse_move_x"].params.len(), 0);
            assert_eq!(by_symbol["cb_rt_mouse_z"].return_ty, IrType::Int);

            // Camera transform core (FD-036 Phase 2). No new opaque type — all
            // Float/Int params and Float/Void returns. RotateCamera/TurnCamera
            // take two angle args (logical, render) feeding two independent
            // fields; DrawToWorld's three flags are Int.
            let position_camera = by_symbol["cb_rt_position_camera"];
            assert_eq!(position_camera.name, "positioncamera");
            assert_eq!(position_camera.params.len(), 3);
            assert_eq!(position_camera.params[0].ty, IrType::Float);
            assert_eq!(position_camera.return_ty, IrType::Void);

            assert_eq!(by_symbol["cb_rt_move_camera"].name, "movecamera");
            assert_eq!(by_symbol["cb_rt_move_camera"].params.len(), 3);
            assert_eq!(by_symbol["cb_rt_translate_camera"].name, "translatecamera");
            assert_eq!(by_symbol["cb_rt_translate_camera"].params.len(), 3);

            let rotate_camera = by_symbol["cb_rt_rotate_camera"];
            assert_eq!(rotate_camera.name, "rotatecamera");
            assert_eq!(rotate_camera.params.len(), 2);
            assert_eq!(rotate_camera.params[0].ty, IrType::Float);
            assert_eq!(rotate_camera.params[1].ty, IrType::Float);
            assert_eq!(rotate_camera.return_ty, IrType::Void);

            let turn_camera = by_symbol["cb_rt_turn_camera"];
            assert_eq!(turn_camera.name, "turncamera");
            assert_eq!(turn_camera.params.len(), 2);
            assert_eq!(turn_camera.params[1].ty, IrType::Float);

            let camera_x = by_symbol["cb_rt_camera_x"];
            assert_eq!(camera_x.name, "camerax");
            assert_eq!(camera_x.params.len(), 0);
            assert_eq!(camera_x.return_ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_camera_y"].return_ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_camera_angle"].name, "cameraangle");
            assert_eq!(by_symbol["cb_rt_camera_angle"].return_ty, IrType::Float);

            let draw_to_world = by_symbol["cb_rt_draw_to_world"];
            assert_eq!(draw_to_world.name, "drawtoworld");
            assert_eq!(draw_to_world.params.len(), 3);
            assert_eq!(draw_to_world.params[0].ty, IrType::Int);
            assert_eq!(draw_to_world.params[2].ty, IrType::Int);
            assert_eq!(draw_to_world.return_ty, IrType::Void);

            let mouse_wx = by_symbol["cb_rt_mouse_wx"];
            assert_eq!(mouse_wx.name, "mousewx");
            assert_eq!(mouse_wx.params.len(), 0);
            assert_eq!(mouse_wx.return_ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_mouse_wy"].name, "mousewy");
            assert_eq!(by_symbol["cb_rt_mouse_wy"].return_ty, IrType::Float);

            // Tile maps (FD-036 Phase 3). `Map` is the opaque return of
            // LoadMap/MakeMap (and EditMap's ignored first param). GetMap takes
            // world Floats; GetMap2/EditMap take 1-based Int grid coords. SetTile
            // has a 2- and 3-arg arity overload sharing the CB name.
            let map_ty = IrType::RuntimeType("Map".to_string());
            let load_map = by_symbol["cb_rt_load_map"];
            assert_eq!(load_map.name, "loadmap");
            assert_eq!(load_map.params.len(), 2);
            assert_eq!(load_map.params[0].ty, IrType::String);
            assert_eq!(load_map.params[1].ty, IrType::String);
            assert_eq!(load_map.return_ty, map_ty);

            let make_map = by_symbol["cb_rt_make_map"];
            assert_eq!(make_map.name, "makemap");
            assert_eq!(make_map.params.len(), 4);
            assert_eq!(make_map.params[0].ty, IrType::Int);
            assert_eq!(make_map.return_ty, map_ty);

            assert_eq!(by_symbol["cb_rt_map_width"].name, "mapwidth");
            assert_eq!(by_symbol["cb_rt_map_width"].params.len(), 0);
            assert_eq!(by_symbol["cb_rt_map_width"].return_ty, IrType::Int);
            assert_eq!(by_symbol["cb_rt_map_height"].name, "mapheight");
            assert_eq!(by_symbol["cb_rt_map_height"].return_ty, IrType::Int);

            let get_map = by_symbol["cb_rt_get_map"];
            assert_eq!(get_map.name, "getmap");
            assert_eq!(get_map.params.len(), 3);
            assert_eq!(get_map.params[0].ty, IrType::Int);
            assert_eq!(get_map.params[1].ty, IrType::Float);
            assert_eq!(get_map.params[2].ty, IrType::Float);
            assert_eq!(get_map.return_ty, IrType::Int);

            let get_map2 = by_symbol["cb_rt_get_map2"];
            assert_eq!(get_map2.name, "getmap2");
            assert_eq!(get_map2.params.len(), 3);
            assert_eq!(get_map2.params[2].ty, IrType::Int);
            assert_eq!(get_map2.return_ty, IrType::Int);

            let edit_map = by_symbol["cb_rt_edit_map"];
            assert_eq!(edit_map.name, "editmap");
            assert_eq!(edit_map.params.len(), 5);
            assert_eq!(edit_map.params[0].ty, map_ty); // ignored, but typed Map
            assert_eq!(edit_map.params[1].ty, IrType::Int);
            assert_eq!(edit_map.return_ty, IrType::Void);

            assert_eq!(by_symbol["cb_rt_set_map"].name, "setmap");
            assert_eq!(by_symbol["cb_rt_set_map"].params.len(), 2);

            let set_tile = by_symbol["cb_rt_set_tile"];
            assert_eq!(set_tile.name, "settile");
            assert_eq!(set_tile.params.len(), 2);
            let set_tile_slow = by_symbol["cb_rt_set_tile_slow"];
            assert_eq!(set_tile_slow.name, "settile");
            assert_eq!(set_tile_slow.params.len(), 3);

            // Objects / sprites (FD-036 Phase 4). `Object` is the opaque tag-13
            // handle that the creation funcs return and the rest borrow. Spot-
            // check the overload families: the z/rotQuality arity overloads, the
            // dual getter/setter slots, and PaintObject's three type-distinct
            // rows (Object×Image, Object×Object, Map×Image).
            let object_ty = IrType::RuntimeType("Object".to_string());
            let image_ty2 = IrType::RuntimeType("Image".to_string());
            let map_ty2 = IrType::RuntimeType("Map".to_string());

            let load_object = by_symbol["cb_rt_load_object"];
            assert_eq!(load_object.name, "loadobject");
            assert_eq!(load_object.params.len(), 1);
            assert_eq!(load_object.params[0].ty, IrType::String);
            assert_eq!(load_object.return_ty, object_ty);
            // 2-arg LoadObject: rotQuality overload (Int), same CB name.
            let load_object_rq = by_symbol["cb_rt_load_object_rq"];
            assert_eq!(load_object_rq.name, "loadobject");
            assert_eq!(load_object_rq.params.len(), 2);
            assert_eq!(load_object_rq.params[1].ty, IrType::Int);

            let load_anim = by_symbol["cb_rt_load_anim_object"];
            assert_eq!(load_anim.name, "loadanimobject");
            assert_eq!(load_anim.params.len(), 5);
            assert_eq!(load_anim.return_ty, object_ty);
            assert_eq!(
                by_symbol["cb_rt_load_anim_object_rq"].name,
                "loadanimobject"
            );
            assert_eq!(by_symbol["cb_rt_load_anim_object_rq"].params.len(), 6);

            let make_object = by_symbol["cb_rt_make_object"];
            assert_eq!(make_object.name, "makeobject");
            assert_eq!(make_object.params.len(), 0);
            assert_eq!(make_object.return_ty, object_ty);
            assert_eq!(by_symbol["cb_rt_make_object_floor"].name, "makeobjectfloor");

            let clone_object = by_symbol["cb_rt_clone_object"];
            assert_eq!(clone_object.name, "cloneobject");
            assert_eq!(clone_object.params[0].ty, object_ty);
            assert_eq!(clone_object.return_ty, object_ty);

            // PositionObject: 3-arg primary + 4-arg z-ignored overload.
            let pos_object = by_symbol["cb_rt_position_object"];
            assert_eq!(pos_object.name, "positionobject");
            assert_eq!(pos_object.params.len(), 3);
            assert_eq!(pos_object.params[0].ty, object_ty);
            assert_eq!(pos_object.params[1].ty, IrType::Float);
            assert_eq!(pos_object.return_ty, IrType::Void);
            assert_eq!(by_symbol["cb_rt_position_object_z"].name, "positionobject");
            assert_eq!(by_symbol["cb_rt_position_object_z"].params.len(), 4);

            let object_x = by_symbol["cb_rt_object_x"];
            assert_eq!(object_x.name, "objectx");
            assert_eq!(object_x.params[0].ty, object_ty);
            assert_eq!(object_x.return_ty, IrType::Float);

            // GetAngle2/Distance2: two Object args, Float return.
            let get_angle2 = by_symbol["cb_rt_get_angle2"];
            assert_eq!(get_angle2.name, "getangle2");
            assert_eq!(get_angle2.params.len(), 2);
            assert_eq!(get_angle2.params[0].ty, object_ty);
            assert_eq!(get_angle2.params[1].ty, object_ty);
            assert_eq!(get_angle2.return_ty, IrType::Float);

            // PaintObject: three overloads disambiguated by param type.
            let paint_img = by_symbol["cb_rt_paint_object_image"];
            assert_eq!(paint_img.name, "paintobject");
            assert_eq!(paint_img.params[0].ty, object_ty);
            assert_eq!(paint_img.params[1].ty, image_ty2);
            let paint_obj = by_symbol["cb_rt_paint_object_object"];
            assert_eq!(paint_obj.name, "paintobject");
            assert_eq!(paint_obj.params[0].ty, object_ty);
            assert_eq!(paint_obj.params[1].ty, object_ty);
            let paint_map = by_symbol["cb_rt_paint_object_map"];
            assert_eq!(paint_map.name, "paintobject");
            assert_eq!(paint_map.params[0].ty, map_ty2);
            assert_eq!(paint_map.params[1].ty, image_ty2);

            // MoveObject arity family: 2-arg (forward only), 3-arg (forward,
            // side), 4-arg (+ ignored z) — all Object-first.
            let move2 = by_symbol["cb_rt_move_object_fwd"];
            assert_eq!(move2.name, "moveobject");
            assert_eq!(move2.params.len(), 2);
            assert_eq!(move2.params[0].ty, object_ty);
            assert_eq!(move2.params[1].ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_move_object"].params.len(), 3);
            assert_eq!(by_symbol["cb_rt_move_object_z"].params.len(), 4);

            // PlayObject arity family 1/3/4/5.
            assert_eq!(by_symbol["cb_rt_play_object"].name, "playobject");
            assert_eq!(by_symbol["cb_rt_play_object"].params.len(), 1);
            assert_eq!(by_symbol["cb_rt_play_object3"].params.len(), 3);
            assert_eq!(by_symbol["cb_rt_play_object4"].params.len(), 4);
            let play5 = by_symbol["cb_rt_play_object5"];
            assert_eq!(play5.name, "playobject");
            assert_eq!(play5.params.len(), 5);
            assert_eq!(play5.params[3].ty, IrType::Float);
            assert_eq!(play5.params[4].ty, IrType::Int);

            // PlayObject also accepts a Map (start tile animation), the same
            // 1/3/4/5 arity family, disambiguated by the Map first param.
            assert_eq!(by_symbol["cb_rt_play_map"].name, "playobject");
            assert_eq!(by_symbol["cb_rt_play_map"].params.len(), 1);
            assert_eq!(by_symbol["cb_rt_play_map"].params[0].ty, map_ty2);
            let play_map4 = by_symbol["cb_rt_play_map4"];
            assert_eq!(play_map4.name, "playobject");
            assert_eq!(play_map4.params.len(), 4);
            assert_eq!(play_map4.params[0].ty, map_ty2);
            assert_eq!(play_map4.params[3].ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_play_map5"].params.len(), 5);

            // ObjectInteger get(1)/set(2) share one CB name; ObjectString get
            // returns String. ObjectLife set marks usingLife.
            let int_get = by_symbol["cb_rt_object_integer_get"];
            assert_eq!(int_get.name, "objectinteger");
            assert_eq!(int_get.params.len(), 1);
            assert_eq!(int_get.return_ty, IrType::Int);
            let int_set = by_symbol["cb_rt_object_integer_set"];
            assert_eq!(int_set.name, "objectinteger");
            assert_eq!(int_set.params.len(), 2);
            assert_eq!(int_set.params[1].ty, IrType::Int);
            assert_eq!(int_set.return_ty, IrType::Void);
            assert_eq!(
                by_symbol["cb_rt_object_string_get"].return_ty,
                IrType::String
            );
            assert_eq!(by_symbol["cb_rt_object_life_set"].name, "objectlife");
            assert_eq!(by_symbol["cb_rt_object_life_set"].params.len(), 2);

            // ObjectSizeX/Y return Int; ObjectFrame returns Float.
            assert_eq!(by_symbol["cb_rt_object_size_x"].return_ty, IrType::Int);
            assert_eq!(by_symbol["cb_rt_object_frame"].return_ty, IrType::Float);

            // Enumeration: NextObject is a 0-arg Object return (Null at end).
            let next_object = by_symbol["cb_rt_next_object"];
            assert_eq!(next_object.name, "nextobject");
            assert_eq!(next_object.params.len(), 0);
            assert_eq!(next_object.return_ty, object_ty);
            assert_eq!(by_symbol["cb_rt_init_object_list"].params.len(), 0);

            // Collision (FD-036 Phase 5). SetupCollision is two type-distinct
            // overloads — object-object (param[1] Object) and the type-4 Map form
            // (param[1] Map) — like PaintObject. ObjectRange/ObjectsOverlap have an
            // optional-arg arity overload. GetCollision returns an Object handle.
            let setup = by_symbol["cb_rt_setup_collision"];
            assert_eq!(setup.name, "setupcollision");
            assert_eq!(setup.params.len(), 5);
            assert_eq!(setup.params[0].ty, object_ty);
            assert_eq!(setup.params[1].ty, object_ty);
            assert_eq!(setup.params[2].ty, IrType::Int);
            assert_eq!(setup.return_ty, IrType::Void);
            let setup_map = by_symbol["cb_rt_setup_collision_map"];
            assert_eq!(setup_map.name, "setupcollision");
            assert_eq!(setup_map.params[1].ty, map_ty2);

            let range = by_symbol["cb_rt_object_range"];
            assert_eq!(range.name, "objectrange");
            assert_eq!(range.params.len(), 2);
            assert_eq!(range.params[0].ty, object_ty);
            assert_eq!(by_symbol["cb_rt_object_range3"].name, "objectrange");
            assert_eq!(by_symbol["cb_rt_object_range3"].params.len(), 3);

            assert_eq!(
                by_symbol["cb_rt_reset_object_collision"].name,
                "resetobjectcollision"
            );
            assert_eq!(by_symbol["cb_rt_clear_collisions"].params.len(), 0);

            let count = by_symbol["cb_rt_count_collisions"];
            assert_eq!(count.name, "countcollisions");
            assert_eq!(count.params[0].ty, object_ty);
            assert_eq!(count.return_ty, IrType::Int);

            // GetCollision: (Object, Int) -> Object handle (Null at miss).
            let get_col = by_symbol["cb_rt_get_collision"];
            assert_eq!(get_col.name, "getcollision");
            assert_eq!(get_col.params.len(), 2);
            assert_eq!(get_col.params[0].ty, object_ty);
            assert_eq!(get_col.params[1].ty, IrType::Int);
            assert_eq!(get_col.return_ty, object_ty);

            assert_eq!(by_symbol["cb_rt_collision_x"].return_ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_collision_y"].return_ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_collision_angle"].return_ty, IrType::Float);

            let overlap = by_symbol["cb_rt_objects_overlap"];
            assert_eq!(overlap.name, "objectsoverlap");
            assert_eq!(overlap.params.len(), 2);
            assert_eq!(overlap.params[0].ty, object_ty);
            assert_eq!(overlap.return_ty, IrType::Int);
            assert_eq!(by_symbol["cb_rt_objects_overlap3"].name, "objectsoverlap");
            assert_eq!(by_symbol["cb_rt_objects_overlap3"].params.len(), 3);

            // Picking & line of sight (FD-036 Phase 5). PickedObject returns an
            // Object handle; PixelPick is a no-op stub (1-/2-arg); ObjectSight
            // returns Int. ScreenPositionObject is (Object, Float, Float).
            let pickable = by_symbol["cb_rt_object_pickable"];
            assert_eq!(pickable.name, "objectpickable");
            assert_eq!(pickable.params.len(), 2);
            assert_eq!(pickable.params[0].ty, object_ty);
            assert_eq!(pickable.params[1].ty, IrType::Int);
            let pick = by_symbol["cb_rt_object_pick"];
            assert_eq!(pick.name, "objectpick");
            assert_eq!(pick.params.len(), 1);
            assert_eq!(pick.params[0].ty, object_ty);
            assert_eq!(pick.return_ty, IrType::Void);
            assert_eq!(by_symbol["cb_rt_pixel_pick"].name, "pixelpick");
            assert_eq!(by_symbol["cb_rt_pixel_pick"].params.len(), 1);
            assert_eq!(by_symbol["cb_rt_pixel_pick_acc"].name, "pixelpick");
            assert_eq!(by_symbol["cb_rt_pixel_pick_acc"].params.len(), 2);
            let picked = by_symbol["cb_rt_picked_object"];
            assert_eq!(picked.name, "pickedobject");
            assert_eq!(picked.params.len(), 0);
            assert_eq!(picked.return_ty, object_ty);
            assert_eq!(by_symbol["cb_rt_picked_x"].return_ty, IrType::Float);
            assert_eq!(by_symbol["cb_rt_picked_angle"].return_ty, IrType::Float);
            let sight = by_symbol["cb_rt_object_sight"];
            assert_eq!(sight.name, "objectsight");
            assert_eq!(sight.params.len(), 2);
            assert_eq!(sight.params[0].ty, object_ty);
            assert_eq!(sight.return_ty, IrType::Int);
            let spo = by_symbol["cb_rt_screen_position_object"];
            assert_eq!(spo.name, "screenpositionobject");
            assert_eq!(spo.params.len(), 3);
            assert_eq!(spo.params[0].ty, object_ty);
            assert_eq!(spo.params[1].ty, IrType::Float);

            // Object-aware camera (FD-036 Phase 5). PointCamera/CameraFollow/
            // Clone* take an Object; CameraPick takes two screen Floats.
            let point_cam = by_symbol["cb_rt_point_camera"];
            assert_eq!(point_cam.name, "pointcamera");
            assert_eq!(point_cam.params.len(), 1);
            assert_eq!(point_cam.params[0].ty, object_ty);
            let follow = by_symbol["cb_rt_camera_follow"];
            assert_eq!(follow.name, "camerafollow");
            assert_eq!(follow.params.len(), 3);
            assert_eq!(follow.params[0].ty, object_ty);
            assert_eq!(follow.params[1].ty, IrType::Int);
            assert_eq!(follow.params[2].ty, IrType::Float);
            assert_eq!(
                by_symbol["cb_rt_clone_camera_position"].params[0].ty,
                object_ty
            );
            assert_eq!(
                by_symbol["cb_rt_clone_camera_orientation"].name,
                "clonecameraorientation"
            );
            let cam_pick = by_symbol["cb_rt_camera_pick"];
            assert_eq!(cam_pick.name, "camerapick");
            assert_eq!(cam_pick.params.len(), 2);
            assert_eq!(cam_pick.params[0].ty, IrType::Float);

            // Particle emitters (FD-038). MakeEmitter returns the Object handle
            // (no new type); ParticleMovement is a 3-arg + 4-arg (accel) overload
            // pair; the Particle* commands all take an Object (the emitter).
            let make_emitter = by_symbol["cb_rt_make_emitter"];
            assert_eq!(make_emitter.name, "makeemitter");
            assert_eq!(make_emitter.params.len(), 2);
            assert_eq!(make_emitter.params[0].ty, image_ty2);
            assert_eq!(make_emitter.params[1].ty, IrType::Int);
            assert_eq!(make_emitter.return_ty, object_ty);
            let pmove = by_symbol["cb_rt_particle_movement"];
            assert_eq!(pmove.name, "particlemovement");
            assert_eq!(pmove.params.len(), 3);
            assert_eq!(pmove.params[0].ty, object_ty);
            assert_eq!(pmove.params[1].ty, IrType::Float);
            assert_eq!(pmove.return_ty, IrType::Void);
            assert_eq!(
                by_symbol["cb_rt_particle_movement_acc"].name,
                "particlemovement"
            );
            assert_eq!(by_symbol["cb_rt_particle_movement_acc"].params.len(), 4);
            let pemit = by_symbol["cb_rt_particle_emission"];
            assert_eq!(pemit.name, "particleemission");
            assert_eq!(pemit.params.len(), 4);
            assert_eq!(pemit.params[0].ty, object_ty);
            assert_eq!(pemit.params[1].ty, IrType::Int);
            assert_eq!(pemit.params[3].ty, IrType::Int);
            let panim = by_symbol["cb_rt_particle_animation"];
            assert_eq!(panim.name, "particleanimation");
            assert_eq!(panim.params.len(), 2);
            assert_eq!(panim.params[0].ty, object_ty);
            assert_eq!(panim.params[1].ty, IrType::Int);

            // Sound (FD-041). `Sound` (tag 17) is the loaded sample; `SoundChannel`
            // (tag 18) is a playing channel. LoadSound/DeleteSound take a Sound;
            // PlaySound is two source-typed overloads (preloaded Sound vs filename
            // String), each with a 1/2/3/4-arg arity family supplying the optional
            // volume/balance/frequency — all returning SoundChannel. SetSound has a
            // 2/3/4/5-arg family over SoundChannel; StopSound/SoundPlaying take a
            // SoundChannel. The naming trap: Set/Stop/SoundPlaying say "Sound" but
            // take a SoundChannel.
            let sound_ty = IrType::RuntimeType("Sound".to_string());
            let channel_ty = IrType::RuntimeType("SoundChannel".to_string());

            let load_sound = by_symbol["cb_rt_load_sound"];
            assert_eq!(load_sound.name, "loadsound");
            assert_eq!(load_sound.params.len(), 1);
            assert_eq!(load_sound.params[0].ty, IrType::String);
            assert_eq!(load_sound.return_ty, sound_ty);

            // PlaySound — preloaded Sound source, 1/2/3/4-arg arity family.
            let play1 = by_symbol["cb_rt_play_sound"];
            assert_eq!(play1.name, "playsound");
            assert_eq!(play1.params.len(), 1);
            assert_eq!(play1.params[0].ty, sound_ty);
            assert_eq!(play1.return_ty, channel_ty);
            assert_eq!(by_symbol["cb_rt_play_sound2"].params.len(), 2);
            assert_eq!(by_symbol["cb_rt_play_sound3"].params.len(), 3);
            let play4 = by_symbol["cb_rt_play_sound4"];
            assert_eq!(play4.name, "playsound");
            assert_eq!(play4.params.len(), 4);
            assert_eq!(play4.params[0].ty, sound_ty);
            assert_eq!(play4.params[1].ty, IrType::Float);
            assert_eq!(play4.params[2].ty, IrType::Float);
            assert_eq!(play4.params[3].ty, IrType::Int);
            assert_eq!(play4.return_ty, channel_ty);

            // PlaySound — filename String source, same arity family, also returns
            // a SoundChannel (the streamed-file "music" path).
            let playf1 = by_symbol["cb_rt_play_sound_file"];
            assert_eq!(playf1.name, "playsound");
            assert_eq!(playf1.params.len(), 1);
            assert_eq!(playf1.params[0].ty, IrType::String);
            assert_eq!(playf1.return_ty, channel_ty);
            assert_eq!(by_symbol["cb_rt_play_sound_file2"].params.len(), 2);
            assert_eq!(by_symbol["cb_rt_play_sound_file3"].params.len(), 3);
            let playf4 = by_symbol["cb_rt_play_sound_file4"];
            assert_eq!(playf4.params.len(), 4);
            assert_eq!(playf4.params[0].ty, IrType::String);
            assert_eq!(playf4.params[3].ty, IrType::Int);
            assert_eq!(playf4.return_ty, channel_ty);

            // SetSound 2/3/4/5-arg family over SoundChannel (the naming trap:
            // takes a SoundChannel despite the "Sound" name).
            let set2 = by_symbol["cb_rt_set_sound"];
            assert_eq!(set2.name, "setsound");
            assert_eq!(set2.params.len(), 2);
            assert_eq!(set2.params[0].ty, channel_ty);
            assert_eq!(set2.params[1].ty, IrType::Int);
            assert_eq!(set2.return_ty, IrType::Void);
            assert_eq!(by_symbol["cb_rt_set_sound3"].params.len(), 3);
            assert_eq!(by_symbol["cb_rt_set_sound4"].params.len(), 4);
            let set5 = by_symbol["cb_rt_set_sound5"];
            assert_eq!(set5.name, "setsound");
            assert_eq!(set5.params.len(), 5);
            assert_eq!(set5.params[0].ty, channel_ty);
            assert_eq!(set5.params[2].ty, IrType::Float);
            assert_eq!(set5.params[4].ty, IrType::Int);

            let stop = by_symbol["cb_rt_stop_sound"];
            assert_eq!(stop.name, "stopsound");
            assert_eq!(stop.params.len(), 1);
            assert_eq!(stop.params[0].ty, channel_ty);
            assert_eq!(stop.return_ty, IrType::Void);

            let playing = by_symbol["cb_rt_sound_playing"];
            assert_eq!(playing.name, "soundplaying");
            assert_eq!(playing.params[0].ty, channel_ty);
            assert_eq!(playing.return_ty, IrType::Int);

            let del_sound = by_symbol["cb_rt_delete_sound"];
            assert_eq!(del_sound.name, "deletesound");
            assert_eq!(del_sound.params.len(), 1);
            assert_eq!(del_sound.params[0].ty, sound_ty);
            assert_eq!(del_sound.return_ty, IrType::Void);
        }

        let create = by_symbol["cb_rt_create_test_handle"];
        assert_eq!(create.name, "createtesthandle");
        assert_eq!(create.params.len(), 0);
        assert_eq!(
            create.return_ty,
            IrType::RuntimeType("TestHandle".to_string())
        );

        let use_h = by_symbol["cb_rt_use_test_handle"];
        assert_eq!(use_h.name, "usetesthandle");
        assert_eq!(
            use_h.params[0].ty,
            IrType::RuntimeType("TestHandle".to_string())
        );
        assert_eq!(use_h.return_ty, IrType::Int);

        // Constants (FD-029): On/Off/PI plus the cbKey* scancode family.
        let consts_by_name: std::collections::HashMap<&str, &RuntimeConstDesc> = catalog
            .constants
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect();

        let on = consts_by_name["On"];
        assert_eq!(on.ty, IrType::Int);
        assert_eq!(on.value, RuntimeConstValue::Int(1));
        assert_eq!(consts_by_name["Off"].value, RuntimeConstValue::Int(0));

        let pi = consts_by_name["PI"];
        assert_eq!(pi.ty, IrType::Float);
        match &pi.value {
            RuntimeConstValue::Float(v) => assert!((v - std::f64::consts::PI).abs() < 1e-9),
            other => panic!("PI should be Float, got {other:?}"),
        }

        // A representative key constant, and the 69/197 Pause/NumLock fix
        // (scancode values match real CoolBasic / DirectInput).
        assert_eq!(consts_by_name["cbKeyEsc"].value, RuntimeConstValue::Int(1));
        assert_eq!(consts_by_name["cbKeyA"].value, RuntimeConstValue::Int(30));
        assert_eq!(
            consts_by_name["cbKeyNumlock"].value,
            RuntimeConstValue::Int(69)
        );
        assert_eq!(
            consts_by_name["cbKeyPause"].value,
            RuntimeConstValue::Int(197)
        );
    }

    #[test]
    fn string_api_roundtrip() {
        let api = string_api();

        // Empty sentinel: immortal (refcount < 0), zero-length, retain/release no-op.
        assert!(!api.empty.is_null());
        let empty_rc = unsafe { cb_rt_string_test_refcount(api.empty) };
        assert!(
            empty_rc < 0,
            "empty sentinel refcount must be negative, got {empty_rc}"
        );
        assert_eq!(unsafe { (api.len)(api.empty) }, 0);

        // retain/release on the sentinel are no-ops and don't perturb refcount.
        let same = unsafe { (api.retain)(api.empty as *mut CbString) };
        assert_eq!(same as *const _, api.empty);
        unsafe { (api.release)(api.empty as *mut CbString) };
        assert_eq!(unsafe { cb_rt_string_test_refcount(api.empty) }, empty_rc);

        // from_literal: fresh handle, refcount = 1, bytes round-trip.
        let bytes = b"hello, cb";
        let h = unsafe { (api.from_literal)(bytes.as_ptr(), bytes.len()) };
        assert!(!h.is_null());
        assert_eq!(unsafe { cb_rt_string_test_refcount(h) }, 1);
        assert_eq!(unsafe { (api.len)(h) }, bytes.len());
        let read = unsafe { std::slice::from_raw_parts((api.data)(h), (api.len)(h)) };
        assert_eq!(read, bytes);

        // retain bumps refcount; release brings it back; final release frees.
        let _h2 = unsafe { (api.retain)(h) };
        assert_eq!(unsafe { cb_rt_string_test_refcount(h) }, 2);
        unsafe { (api.release)(h) };
        assert_eq!(unsafe { cb_rt_string_test_refcount(h) }, 1);

        // Concat — both halves non-empty.
        let a = unsafe { (api.from_literal)(b"foo".as_ptr(), 3) };
        let b = unsafe { (api.from_literal)(b"bar".as_ptr(), 3) };
        let ab = unsafe { (api.concat)(a, b) };
        assert_eq!(unsafe { (api.len)(ab) }, 6);
        let read = unsafe { std::slice::from_raw_parts((api.data)(ab), (api.len)(ab)) };
        assert_eq!(read, b"foobar");
        unsafe {
            (api.release)(a);
            (api.release)(b);
            (api.release)(ab);
            (api.release)(h); // final release for the earlier handle
        }

        // Concat with empty operand returns a retained handle of the other side
        // (no allocation), and concat-of-empties returns the sentinel.
        let s = unsafe { (api.from_literal)(b"x".as_ptr(), 1) };
        let s_plus_empty = unsafe { (api.concat)(s, api.empty) };
        assert_eq!(s_plus_empty, s); // same pointer, retain bumped
        assert_eq!(unsafe { cb_rt_string_test_refcount(s) }, 2);
        unsafe {
            (api.release)(s_plus_empty);
            (api.release)(s);
        }
        let empty_concat = unsafe { (api.concat)(api.empty, api.empty) };
        assert_eq!(empty_concat as *const _, api.empty);
    }

    #[test]
    fn runtime_init_roundtrip() {
        // FD-015: handing the runtime a host API returns a non-null hook table
        // whose size guard matches our mirror struct; about_to_exit is
        // reserved (null) for now.
        extern "C" fn noop_exit(_code: i32) {}
        extern "C" fn noop_error(_msg: *const CbString) {}
        static HOST: CbHostApi = CbHostApi {
            size: std::mem::size_of::<CbHostApi>() as u32,
            abi_version: CB_HOST_ABI_VERSION,
            request_exit: noop_exit,
            raise_error: noop_error,
        };
        let hooks = runtime_init(&HOST).expect("runtime_init should return a hook table");
        assert_eq!(hooks.size as usize, std::mem::size_of::<CbRuntimeHooks>());
        assert!(hooks.about_to_exit.is_none());
    }

    #[test]
    fn runtime_init_rejects_abi_mismatch() {
        // FD-024: a host advertising a different host ABI is declined by the C
        // `cb_runtime_init` (returns null), surfaced as Err — never stored. The
        // rejection path leaves g_host untouched, so this is safe to run beside
        // the happy-path roundtrip test.
        extern "C" fn noop_exit(_code: i32) {}
        extern "C" fn noop_error(_msg: *const CbString) {}
        static BAD_HOST: CbHostApi = CbHostApi {
            size: std::mem::size_of::<CbHostApi>() as u32,
            abi_version: CB_HOST_ABI_VERSION + 1,
            request_exit: noop_exit,
            raise_error: noop_error,
        };
        match runtime_init(&BAD_HOST) {
            Err(e) => assert!(e.contains("declined"), "got: {e}"),
            Ok(_) => panic!("expected the runtime to decline an ABI-mismatched host"),
        }
    }

    // ── type_tag_to_ir_type ─────────────────────────────────────────────

    #[test]
    fn type_tag_maps_each_primitive() {
        let empty = HashMap::new();
        let cases = [
            (CB_TYPE_VOID, IrType::Void),
            (CB_TYPE_BYTE, IrType::Byte),
            (CB_TYPE_SHORT, IrType::Short),
            (CB_TYPE_INT, IrType::Int),
            (CB_TYPE_LONG, IrType::Long),
            (CB_TYPE_FLOAT, IrType::Float),
            (CB_TYPE_STRING, IrType::String),
        ];
        for (tag, expected) in cases {
            assert_eq!(type_tag_to_ir_type(tag, &empty).unwrap(), expected);
        }
    }

    #[test]
    fn reserved_type_tags_are_rejected() {
        // UInt/ULong/Bool wire codes are kept for ABI stability but are no
        // longer supported types (FD-035) — decoding one must be an error.
        let empty = HashMap::new();
        for tag in [CB_TYPE_UINT, CB_TYPE_ULONG, CB_TYPE_BOOL] {
            assert!(type_tag_to_ir_type(tag, &empty).is_err());
        }
    }

    #[test]
    fn type_tag_custom_hit_and_unknown_miss() {
        let mut custom = HashMap::new();
        custom.insert(42u32, "Widget".to_string());
        assert_eq!(
            type_tag_to_ir_type(42, &custom).unwrap(),
            IrType::RuntimeType("Widget".to_string())
        );
        let err = type_tag_to_ir_type(99, &custom).unwrap_err();
        assert!(err.contains("unknown type tag"), "got: {err}");
    }

    // ── decode_catalog fixtures ─────────────────────────────────────────
    //
    // The crate sets `unsafe_code = "allow"`, so tests can hand `decode_catalog`
    // hand-built `CbCatalog`s with raw pointers into test-owned backing data.
    // Every fixture keeps its CStrings / arrays in locals that outlive the call.

    use std::ffi::{CString, c_char};

    unsafe extern "C" fn dummy_fn() {}

    fn empty_catalog() -> CbCatalog {
        CbCatalog {
            version: CB_CATALOG_VERSION,
            type_count: 0,
            types: std::ptr::null(),
            func_count: 0,
            funcs: std::ptr::null(),
            const_count: 0,
            consts: std::ptr::null(),
            strings: std::ptr::null(),
        }
    }

    #[test]
    fn decode_accepts_empty_catalog() {
        let cat = decode_catalog(&empty_catalog()).expect("empty catalog is valid");
        assert!(cat.types.is_empty() && cat.functions.is_empty() && cat.constants.is_empty());
    }

    #[test]
    fn decode_rejects_version_mismatch() {
        let mut c = empty_catalog();
        c.version = CB_CATALOG_VERSION + 1;
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("unsupported catalog version"), "got: {err}");
    }

    #[test]
    fn decode_rejects_null_types_ptr() {
        let mut c = empty_catalog();
        c.type_count = 1; // pointer stays null
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("null types pointer"), "got: {err}");
    }

    #[test]
    fn decode_rejects_null_type_name() {
        let types = [CbTypeDesc {
            name: std::ptr::null(),
            tag: 10,
        }];
        let mut c = empty_catalog();
        c.type_count = 1;
        c.types = types.as_ptr();
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("null type name"), "got: {err}");
    }

    #[test]
    fn decode_rejects_reserved_tag() {
        let name = CString::new("Bad").unwrap();
        let types = [CbTypeDesc {
            name: name.as_ptr(),
            tag: 9,
        }];
        let mut c = empty_catalog();
        c.type_count = 1;
        c.types = types.as_ptr();
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("reserved tag"), "got: {err}");
    }

    #[test]
    fn decode_rejects_bad_utf8_type_name() {
        // 0xFF is not valid UTF-8; NUL-terminated so CStr reads exactly [0xFF].
        let bad: [u8; 2] = [0xFF, 0x00];
        let types = [CbTypeDesc {
            name: bad.as_ptr() as *const c_char,
            tag: 10,
        }];
        let mut c = empty_catalog();
        c.type_count = 1;
        c.types = types.as_ptr();
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("invalid UTF-8 in type name"), "got: {err}");
    }

    #[test]
    fn decode_rejects_duplicate_tag() {
        let a = CString::new("Alpha").unwrap();
        let b = CString::new("Beta").unwrap();
        let types = [
            CbTypeDesc {
                name: a.as_ptr(),
                tag: 10,
            },
            CbTypeDesc {
                name: b.as_ptr(),
                tag: 10,
            },
        ];
        let mut c = empty_catalog();
        c.type_count = 2;
        c.types = types.as_ptr();
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("duplicate runtime type tag"), "got: {err}");
    }

    #[test]
    fn decode_allows_null_fn_ptr() {
        // FD-045: a null fn_ptr is valid *metadata* — the metadata-only catalog
        // carries no pointers. Decoding must accept it (the interpreter resolves
        // pointers separately via resolve_bindings()).
        let name = CString::new("foo").unwrap();
        let sym = CString::new("cb_rt_foo").unwrap();
        let funcs = [CbFuncDesc {
            name: name.as_ptr(),
            symbol: sym.as_ptr(),
            fn_ptr: None,
            params: std::ptr::null(),
            param_count: 0,
            return_type: CB_TYPE_VOID,
            flags: 0,
        }];
        let mut c = empty_catalog();
        c.func_count = 1;
        c.funcs = funcs.as_ptr();
        let decoded = decode_catalog(&c).expect("null fn_ptr is valid metadata");
        assert_eq!(decoded.functions.len(), 1);
        assert_eq!(decoded.functions[0].c_symbol, "cb_rt_foo");
    }

    // ── FD-045 drift guard (reconcile_catalogs) ────────────────────────────

    fn rt_desc(name: &str, sym: &str, params: &[IrType], ret: IrType) -> FuncDesc {
        FuncDesc {
            name: name.to_string(),
            c_symbol: sym.to_string(),
            params: params
                .iter()
                .map(|ty| FuncParamDesc {
                    name: None,
                    ty: ty.clone(),
                })
                .collect(),
            return_ty: ret,
        }
    }

    fn rt_catalog(functions: Vec<FuncDesc>) -> RuntimeCatalog {
        RuntimeCatalog {
            types: Vec::new(),
            functions,
            constants: Vec::new(),
        }
    }

    #[test]
    fn reconcile_accepts_matching_catalogs() {
        let mk = || {
            rt_catalog(vec![
                rt_desc("abs", "cb_rt_abs_int", &[IrType::Int], IrType::Int),
                rt_desc("print", "cb_rt_print", &[IrType::String], IrType::Void),
            ])
        };
        reconcile_catalogs(&mk(), &mk()).expect("identical catalogs must reconcile");
    }

    #[test]
    fn reconcile_rejects_missing_symbol() {
        let meta = rt_catalog(vec![
            rt_desc("a", "cb_rt_a", &[], IrType::Void),
            rt_desc("b", "cb_rt_b", &[], IrType::Void),
        ]);
        let full = rt_catalog(vec![rt_desc("a", "cb_rt_a", &[], IrType::Void)]);
        let err = reconcile_catalogs(&meta, &full).unwrap_err();
        assert!(err.contains("symbol sets differ"), "got: {err}");
        assert!(err.contains("cb_rt_b"), "got: {err}");
    }

    #[test]
    fn reconcile_rejects_extra_binding_symbol() {
        let meta = rt_catalog(vec![rt_desc("a", "cb_rt_a", &[], IrType::Void)]);
        let full = rt_catalog(vec![
            rt_desc("a", "cb_rt_a", &[], IrType::Void),
            rt_desc("z", "cb_rt_z", &[], IrType::Void),
        ]);
        let err = reconcile_catalogs(&meta, &full).unwrap_err();
        assert!(err.contains("symbol sets differ"), "got: {err}");
        assert!(err.contains("cb_rt_z"), "got: {err}");
    }

    #[test]
    fn reconcile_rejects_signature_drift() {
        // Same symbol set, but one parameter type differs — the case the
        // symbol-set check alone would miss.
        let meta = rt_catalog(vec![rt_desc("f", "cb_rt_f", &[IrType::Int], IrType::Void)]);
        let full = rt_catalog(vec![rt_desc(
            "f",
            "cb_rt_f",
            &[IrType::Float],
            IrType::Void,
        )]);
        let err = reconcile_catalogs(&meta, &full).unwrap_err();
        assert!(err.contains("signature drift"), "got: {err}");
    }

    #[test]
    fn decode_allows_overloaded_names() {
        // Same name, distinct symbols — the legal `abs` overload shape.
        let name = CString::new("abs").unwrap();
        let s1 = CString::new("cb_rt_abs_int").unwrap();
        let s2 = CString::new("cb_rt_abs_float").unwrap();
        let funcs = [
            CbFuncDesc {
                name: name.as_ptr(),
                symbol: s1.as_ptr(),
                fn_ptr: Some(dummy_fn),
                params: std::ptr::null(),
                param_count: 0,
                return_type: CB_TYPE_INT,
                flags: 0,
            },
            CbFuncDesc {
                name: name.as_ptr(),
                symbol: s2.as_ptr(),
                fn_ptr: Some(dummy_fn),
                params: std::ptr::null(),
                param_count: 0,
                return_type: CB_TYPE_FLOAT,
                flags: 0,
            },
        ];
        let mut c = empty_catalog();
        c.func_count = 2;
        c.funcs = funcs.as_ptr();
        let cat = decode_catalog(&c).expect("overloaded names are valid");
        assert_eq!(cat.functions.len(), 2);
        assert!(cat.functions.iter().all(|f| f.name == "abs"));
    }

    #[test]
    fn decode_allows_shared_symbol_aliases() {
        // Two distinct CB names backed by ONE C symbol — the real catalog's
        // putpixel/putpixel2 → cb_rt_put_pixel_argb shape. Dispatch is by
        // fn_ptr, not symbol, so this is legal and must not be rejected.
        let n1 = CString::new("putpixel").unwrap();
        let n2 = CString::new("putpixel2").unwrap();
        let sym = CString::new("cb_rt_put_pixel_argb").unwrap();
        let mk = |name: &CString| CbFuncDesc {
            name: name.as_ptr(),
            symbol: sym.as_ptr(),
            fn_ptr: Some(dummy_fn),
            params: std::ptr::null(),
            param_count: 0,
            return_type: CB_TYPE_VOID,
            flags: 0,
        };
        let funcs = [mk(&n1), mk(&n2)];
        let mut c = empty_catalog();
        c.func_count = 2;
        c.funcs = funcs.as_ptr();
        let cat = decode_catalog(&c).expect("shared-symbol aliases are valid");
        assert_eq!(cat.functions.len(), 2);
        assert!(
            cat.functions
                .iter()
                .all(|f| f.c_symbol == "cb_rt_put_pixel_argb")
        );
    }

    #[test]
    fn decode_rejects_bad_utf8_param_name() {
        let name = CString::new("foo").unwrap();
        let sym = CString::new("cb_rt_foo").unwrap();
        let bad: [u8; 2] = [0xFF, 0x00];
        let params = [CbParamDesc {
            name: bad.as_ptr() as *const c_char,
            ty: CB_TYPE_INT,
        }];
        let funcs = [CbFuncDesc {
            name: name.as_ptr(),
            symbol: sym.as_ptr(),
            fn_ptr: Some(dummy_fn),
            params: params.as_ptr(),
            param_count: 1,
            return_type: CB_TYPE_VOID,
            flags: 0,
        }];
        let mut c = empty_catalog();
        c.func_count = 1;
        c.funcs = funcs.as_ptr();
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("invalid UTF-8 in param name"), "got: {err}");
    }

    #[test]
    fn decode_rejects_unsupported_const_tag() {
        let name = CString::new("Greeting").unwrap();
        let consts = [CbConstDesc {
            name: name.as_ptr(),
            tag: CB_TYPE_STRING,
            value_bits: 0,
        }];
        let mut c = empty_catalog();
        c.const_count = 1;
        c.consts = consts.as_ptr();
        let err = decode_catalog(&c).unwrap_err();
        assert!(err.contains("unsupported type tag"), "got: {err}");
    }

    #[test]
    fn decode_accepts_int_and_float_consts() {
        let on = CString::new("On").unwrap();
        let pi = CString::new("PI").unwrap();
        let consts = [
            CbConstDesc {
                name: on.as_ptr(),
                tag: CB_TYPE_INT,
                value_bits: 1,
            },
            CbConstDesc {
                name: pi.as_ptr(),
                tag: CB_TYPE_FLOAT,
                value_bits: std::f64::consts::PI.to_bits(),
            },
        ];
        let mut c = empty_catalog();
        c.const_count = 2;
        c.consts = consts.as_ptr();
        let cat = decode_catalog(&c).expect("int/float consts are valid");
        assert_eq!(cat.constants.len(), 2);
        assert_eq!(cat.constants[0].value, RuntimeConstValue::Int(1));
        match cat.constants[1].value {
            RuntimeConstValue::Float(v) => assert!((v - std::f64::consts::PI).abs() < 1e-12),
            ref other => panic!("expected Float, got {other:?}"),
        }
    }
}
