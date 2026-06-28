//! `IrType` → `inkwell` type mapping and function-type construction (FD-049).
//!
//! Scalar Phase-1 surface plus array handles (FD-049 Phase 2): an `Array`
//! lowers to an opaque `CbArray*` pointer. The remaining reference IR types
//! (`TypeRef`, `StructVal`, `FnPtr`, `RuntimeType`) are deliberately rejected
//! with an error rather than guessed at — they are out of scope, so a program
//! that reaches one fails the codegen loudly instead of miscompiling.

use inkwell::AddressSpace;
use inkwell::context::Context;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType};

use cb_ir::{IrType, StructDefInfo};

/// Map an IR type to its inkwell representation. Scalars map directly;
/// `String`/`Null`, an array handle, and a user-`Type` instance lower to an
/// opaque pointer; a value `StructVal` lowers to an inline LLVM `StructType`
/// built recursively from `struct_defs` (value semantics — copied by load/store,
/// CB forbids self-containment so the recursion terminates). Errors on the
/// remaining reference types (`FnPtr` until Phase 3c, `RuntimeType`, `Void`).
pub fn basic_type<'ctx>(
    ctx: &'ctx Context,
    struct_defs: &[StructDefInfo],
    ty: &IrType,
) -> Result<BasicTypeEnum<'ctx>, String> {
    Ok(match ty {
        IrType::Byte => ctx.i8_type().into(),
        IrType::Short => ctx.i16_type().into(),
        IrType::Int => ctx.i32_type().into(),
        IrType::Long => ctx.i64_type().into(),
        IrType::Float => ctx.f64_type().into(),
        // CbString* (opaque) and a null reference both lower to `ptr`.
        IrType::String | IrType::Null => ctx.ptr_type(AddressSpace::default()).into(),
        // An array handle is an opaque `CbArray*` (FD-049 Phase 2), like String.
        IrType::Array { .. } => ctx.ptr_type(AddressSpace::default()).into(),
        // A user-`Type` instance is an opaque `CbTypeHeader*` (FD-049 Phase 3a);
        // its inline fields are GEP'd through a per-type node struct in func.rs.
        IrType::TypeRef(_) => ctx.ptr_type(AddressSpace::default()).into(),
        // A value struct is an inline aggregate (FD-049 Phase 3b).
        IrType::StructVal(name) => {
            let def = struct_defs
                .iter()
                .find(|d| d.name == *name)
                .ok_or_else(|| format!("unknown value struct {name:?}"))?;
            let mut elems: Vec<BasicTypeEnum<'ctx>> = Vec::with_capacity(def.fields.len());
            for (_, fty) in &def.fields {
                elems.push(basic_type(ctx, struct_defs, fty)?);
            }
            ctx.struct_type(&elems, false).into()
        }
        // A function pointer is an opaque `ptr` (FD-049 Phase 3c); the callee
        // function type is rebuilt from its `FnSig` at the `CallIndirect` site.
        IrType::FnPtr(_) => ctx.ptr_type(AddressSpace::default()).into(),
        other => {
            return Err(format!(
                "IR type {other:?} is out of scope for the LLVM backend \
                 (runtime handles are not lowered yet)"
            ));
        }
    })
}

/// Build the LLVM function type for `params` → `ret`. A `Void` return maps to
/// the LLVM `void` type; any other return maps through [`basic_type`]. User
/// functions may take/return value structs by value, so `struct_defs` threads in.
pub fn fn_type<'ctx>(
    ctx: &'ctx Context,
    struct_defs: &[StructDefInfo],
    params: &[IrType],
    ret: &IrType,
) -> Result<FunctionType<'ctx>, String> {
    let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::with_capacity(params.len());
    for p in params {
        param_types.push(basic_type(ctx, struct_defs, p)?.into());
    }
    Ok(match ret {
        IrType::Void => ctx.void_type().fn_type(&param_types, false),
        other => basic_type(ctx, struct_defs, other)?.fn_type(&param_types, false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_types_map() {
        let ctx = Context::create();
        assert!(basic_type(&ctx, &[], &IrType::Int).unwrap().is_int_type());
        assert!(basic_type(&ctx, &[], &IrType::Long).unwrap().is_int_type());
        assert!(basic_type(&ctx, &[], &IrType::Byte).unwrap().is_int_type());
        assert!(basic_type(&ctx, &[], &IrType::Short).unwrap().is_int_type());
        assert!(
            basic_type(&ctx, &[], &IrType::Float)
                .unwrap()
                .is_float_type()
        );
        assert!(
            basic_type(&ctx, &[], &IrType::String)
                .unwrap()
                .is_pointer_type()
        );
        assert!(
            basic_type(&ctx, &[], &IrType::Null)
                .unwrap()
                .is_pointer_type()
        );
    }

    #[test]
    fn widths_are_correct() {
        let ctx = Context::create();
        assert_eq!(
            basic_type(&ctx, &[], &IrType::Int)
                .unwrap()
                .into_int_type()
                .get_bit_width(),
            32
        );
        assert_eq!(
            basic_type(&ctx, &[], &IrType::Long)
                .unwrap()
                .into_int_type()
                .get_bit_width(),
            64
        );
        assert_eq!(
            basic_type(&ctx, &[], &IrType::Byte)
                .unwrap()
                .into_int_type()
                .get_bit_width(),
            8
        );
    }

    #[test]
    fn aggregate_types_error() {
        use cb_diagnostics::Symbol;
        let ctx = Context::create();
        // An array handle lowers to an opaque pointer (FD-049 Phase 2)...
        assert!(
            basic_type(
                &ctx,
                &[],
                &IrType::Array {
                    elem: Box::new(IrType::Int),
                    rank: 1
                }
            )
            .unwrap()
            .is_pointer_type()
        );
        // ...as does a user-`Type` instance handle (FD-049 Phase 3a)...
        assert!(
            basic_type(&ctx, &[], &IrType::TypeRef(Symbol::DUMMY))
                .unwrap()
                .is_pointer_type()
        );
        // ...a value struct lowers to an inline aggregate (FD-049 Phase 3b)...
        let def = StructDefInfo {
            name: Symbol::DUMMY,
            fields: vec![(Symbol::DUMMY, IrType::Int), (Symbol::DUMMY, IrType::Float)],
        };
        let st = basic_type(
            &ctx,
            std::slice::from_ref(&def),
            &IrType::StructVal(Symbol::DUMMY),
        )
        .unwrap();
        assert!(st.is_struct_type());
        assert_eq!(st.into_struct_type().count_fields(), 2);
        // ...and a function pointer lowers to an opaque pointer (FD-049 Phase 3c);
        // only runtime handles remain rejected.
        assert!(
            basic_type(
                &ctx,
                &[],
                &IrType::FnPtr(Box::new(cb_ir::FnSig {
                    params: vec![],
                    ret: Box::new(IrType::Void),
                }))
            )
            .unwrap()
            .is_pointer_type()
        );
        assert!(basic_type(&ctx, &[], &IrType::RuntimeType("Handle".into())).is_err());
    }

    #[test]
    fn fn_type_void_and_scalar() {
        let ctx = Context::create();
        let void_fn = fn_type(&ctx, &[], &[IrType::Int, IrType::Float], &IrType::Void).unwrap();
        assert_eq!(void_fn.get_param_types().len(), 2);
        assert!(void_fn.get_return_type().is_none());

        let int_fn = fn_type(&ctx, &[], &[], &IrType::Int).unwrap();
        assert!(int_fn.get_return_type().is_some());
    }
}
