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

use cb_ir::IrType;

/// Map a scalar IR type to its inkwell representation. `String`/`Null` lower to
/// an opaque pointer (`CbString*` / a null reference). Errors on aggregate or
/// reference types (out of Phase-1 scope).
pub fn basic_type<'ctx>(ctx: &'ctx Context, ty: &IrType) -> Result<BasicTypeEnum<'ctx>, String> {
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
        other => {
            return Err(format!(
                "IR type {other:?} is out of scope for the Phase-1 LLVM backend \
                 (value structs, fn-pointers, and runtime handles \
                 are not lowered yet)"
            ));
        }
    })
}

/// Build the LLVM function type for `params` → `ret`. A `Void` return maps to
/// the LLVM `void` type; any other return maps through [`basic_type`].
pub fn fn_type<'ctx>(
    ctx: &'ctx Context,
    params: &[IrType],
    ret: &IrType,
) -> Result<FunctionType<'ctx>, String> {
    let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::with_capacity(params.len());
    for p in params {
        param_types.push(basic_type(ctx, p)?.into());
    }
    Ok(match ret {
        IrType::Void => ctx.void_type().fn_type(&param_types, false),
        other => basic_type(ctx, other)?.fn_type(&param_types, false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_types_map() {
        let ctx = Context::create();
        assert!(basic_type(&ctx, &IrType::Int).unwrap().is_int_type());
        assert!(basic_type(&ctx, &IrType::Long).unwrap().is_int_type());
        assert!(basic_type(&ctx, &IrType::Byte).unwrap().is_int_type());
        assert!(basic_type(&ctx, &IrType::Short).unwrap().is_int_type());
        assert!(basic_type(&ctx, &IrType::Float).unwrap().is_float_type());
        assert!(basic_type(&ctx, &IrType::String).unwrap().is_pointer_type());
        assert!(basic_type(&ctx, &IrType::Null).unwrap().is_pointer_type());
    }

    #[test]
    fn widths_are_correct() {
        let ctx = Context::create();
        assert_eq!(
            basic_type(&ctx, &IrType::Int)
                .unwrap()
                .into_int_type()
                .get_bit_width(),
            32
        );
        assert_eq!(
            basic_type(&ctx, &IrType::Long)
                .unwrap()
                .into_int_type()
                .get_bit_width(),
            64
        );
        assert_eq!(
            basic_type(&ctx, &IrType::Byte)
                .unwrap()
                .into_int_type()
                .get_bit_width(),
            8
        );
    }

    #[test]
    fn aggregate_types_error() {
        let ctx = Context::create();
        // An array handle lowers to an opaque pointer (FD-049 Phase 2)...
        assert!(
            basic_type(
                &ctx,
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
            basic_type(&ctx, &IrType::TypeRef(cb_diagnostics::Symbol::DUMMY))
                .unwrap()
                .is_pointer_type()
        );
        // ...while value structs and fn-pointers are still rejected (Phase 3b/3c).
        assert!(basic_type(&ctx, &IrType::StructVal(cb_diagnostics::Symbol::DUMMY)).is_err());
        assert!(
            basic_type(
                &ctx,
                &IrType::FnPtr(Box::new(cb_ir::FnSig {
                    params: vec![],
                    ret: Box::new(IrType::Void),
                }))
            )
            .is_err()
        );
    }

    #[test]
    fn fn_type_void_and_scalar() {
        let ctx = Context::create();
        let void_fn = fn_type(&ctx, &[IrType::Int, IrType::Float], &IrType::Void).unwrap();
        assert_eq!(void_fn.get_param_types().len(), 2);
        assert!(void_fn.get_return_type().is_none());

        let int_fn = fn_type(&ctx, &[], &IrType::Int).unwrap();
        assert!(int_fn.get_return_type().is_some());
    }
}
