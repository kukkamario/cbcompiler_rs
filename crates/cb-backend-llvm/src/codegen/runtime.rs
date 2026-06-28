//! Lazy runtime-symbol declaration cache.
//!
//! The native backend calls two kinds of runtime functions:
//!   * fixed support helpers with hardcoded signatures — the String primitives
//!     (`from_literal`/`retain`/`release`/`len`/`concat`/`compare`), the
//!     number↔string conversions, and the AOT lifecycle (`cb_rt_exit`,
//!     `cb_rt_standalone_run`); and
//!   * catalog functions reached through `Call`, declared from the IR signature.
//!
//! Both are declared on demand and cached by symbol so a symbol is `declare`d at
//! most once per module. `size_t` is the 64-bit `i64` (the only supported
//! targets are 64-bit Windows/Linux).

use inkwell::AddressSpace;
use inkwell::module::Linkage;
use inkwell::types::FunctionType;
use inkwell::values::FunctionValue;

use cb_ir::FnSig;

use super::Codegen;
use super::types::fn_type;

impl<'a, 'ctx> Codegen<'a, 'ctx> {
    /// Declare `symbol` with `fty` (external linkage), caching so a repeated
    /// request returns the same `FunctionValue` rather than a duplicate decl.
    fn declare_rt(&self, symbol: &str, fty: FunctionType<'ctx>) -> FunctionValue<'ctx> {
        if let Some(f) = self.runtime.borrow().get(symbol) {
            return *f;
        }
        let f = self
            .module
            .add_function(symbol, fty, Some(Linkage::External));
        self.runtime.borrow_mut().insert(symbol.to_string(), f);
        f
    }

    pub(super) fn ptr_t(&self) -> inkwell::types::PointerType<'ctx> {
        self.ctx.ptr_type(AddressSpace::default())
    }

    // ── Fixed support helpers ───────────────────────────────────────────

    /// `CbString* cb_rt_string_from_literal(const u8*, size_t)`.
    pub(super) fn rt_string_from_literal(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = p.fn_type(&[p.into(), self.ctx.i64_type().into()], false);
        self.declare_rt("cb_rt_string_from_literal", fty)
    }

    /// `CbString* cb_rt_string_retain(CbString*)`.
    pub(super) fn rt_string_retain(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt("cb_rt_string_retain", p.fn_type(&[p.into()], false))
    }

    /// `void cb_rt_string_release(CbString*)`.
    pub(super) fn rt_string_release(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = self.ctx.void_type().fn_type(&[p.into()], false);
        self.declare_rt("cb_rt_string_release", fty)
    }

    /// `size_t cb_rt_string_char_len(const CbString*)` — the codepoint count
    /// (CB `Len(s$)`), matching the interpreter; NOT the byte length.
    pub(super) fn rt_string_char_len(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_string_char_len",
            self.ctx.i64_type().fn_type(&[p.into()], false),
        )
    }

    /// `CbString* cb_rt_string_concat(const CbString*, const CbString*)`.
    pub(super) fn rt_string_concat(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_string_concat",
            p.fn_type(&[p.into(), p.into()], false),
        )
    }

    /// `int32_t cb_rt_string_compare(const CbString*, const CbString*)`.
    pub(super) fn rt_string_compare(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = self.ctx.i32_type().fn_type(&[p.into(), p.into()], false);
        self.declare_rt("cb_rt_string_compare", fty)
    }

    /// `CbString* cb_rt_int_to_string(int32_t)`.
    pub(super) fn rt_int_to_string(&self) -> FunctionValue<'ctx> {
        let fty = self.ptr_t().fn_type(&[self.ctx.i32_type().into()], false);
        self.declare_rt("cb_rt_int_to_string", fty)
    }

    /// `CbString* cb_rt_long_to_string(int64_t)`.
    pub(super) fn rt_long_to_string(&self) -> FunctionValue<'ctx> {
        let fty = self.ptr_t().fn_type(&[self.ctx.i64_type().into()], false);
        self.declare_rt("cb_rt_long_to_string", fty)
    }

    /// `CbString* cb_rt_float_to_string(double)`.
    pub(super) fn rt_float_to_string(&self) -> FunctionValue<'ctx> {
        let fty = self.ptr_t().fn_type(&[self.ctx.f64_type().into()], false);
        self.declare_rt("cb_rt_float_to_string", fty)
    }

    /// `int64_t cb_rt_string_to_long(const CbString*)`.
    pub(super) fn rt_string_to_long(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_string_to_long",
            self.ctx.i64_type().fn_type(&[p.into()], false),
        )
    }

    /// `double cb_rt_string_to_float(const CbString*)`.
    pub(super) fn rt_string_to_float(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_string_to_float",
            self.ctx.f64_type().fn_type(&[p.into()], false),
        )
    }

    /// `void cb_rt_exit(int32_t)` — no-return clean exit.
    pub(super) fn rt_exit(&self) -> FunctionValue<'ctx> {
        let fty = self
            .ctx
            .void_type()
            .fn_type(&[self.ctx.i32_type().into()], false);
        self.declare_rt("cb_rt_exit", fty)
    }

    /// `int32_t cb_rt_standalone_run(void (*)(void))` — the AOT bootstrap.
    pub(super) fn rt_standalone_run(&self) -> FunctionValue<'ctx> {
        let fty = self.ctx.i32_type().fn_type(&[self.ptr_t().into()], false);
        self.declare_rt("cb_rt_standalone_run", fty)
    }

    /// `void cb_rt_trap_null_fnptr(void)` — no-return null-fn-ptr-call trap that
    /// raises the interpreter-matching stderr message, then exits 1.
    pub(super) fn rt_trap_null_fnptr(&self) -> FunctionValue<'ctx> {
        let fty = self.ctx.void_type().fn_type(&[], false);
        self.declare_rt("cb_rt_trap_null_fnptr", fty)
    }

    // ── Array heap helpers (cb_array.cpp) ────────────────

    /// `CbArray* cb_rt_array_new(int64_t rank, const int64_t* dims,
    ///                           int64_t elem_size, int32_t elem_is_ref)`.
    pub(super) fn rt_array_new(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let i64 = self.ctx.i64_type();
        let fty = p.fn_type(
            &[i64.into(), p.into(), i64.into(), self.ctx.i32_type().into()],
            false,
        );
        self.declare_rt("cb_rt_array_new", fty)
    }

    /// `void* cb_rt_array_elem_addr(CbArray*, const int64_t* indices, int64_t rank)`.
    pub(super) fn rt_array_elem_addr(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = p.fn_type(&[p.into(), p.into(), self.ctx.i64_type().into()], false);
        self.declare_rt("cb_rt_array_elem_addr", fty)
    }

    /// `void* cb_rt_array_elem_addr_flat(CbArray*, int64_t index)`.
    pub(super) fn rt_array_elem_addr_flat(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = p.fn_type(&[p.into(), self.ctx.i64_type().into()], false);
        self.declare_rt("cb_rt_array_elem_addr_flat", fty)
    }

    /// `int64_t cb_rt_array_total_len(const CbArray*)`.
    pub(super) fn rt_array_total_len(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_array_total_len",
            self.ctx.i64_type().fn_type(&[p.into()], false),
        )
    }

    /// `int64_t cb_rt_array_dim_len(const CbArray*, int64_t dim)`.
    pub(super) fn rt_array_dim_len(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = self
            .ctx
            .i64_type()
            .fn_type(&[p.into(), self.ctx.i64_type().into()], false);
        self.declare_rt("cb_rt_array_dim_len", fty)
    }

    // ── Type-instance heap + list helpers (cb_type.cpp) ──

    /// `void* cb_rt_type_new(int64_t type_def, int64_t size)`.
    pub(super) fn rt_type_new(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let i64 = self.ctx.i64_type();
        self.declare_rt(
            "cb_rt_type_new",
            p.fn_type(&[i64.into(), i64.into()], false),
        )
    }

    /// `void* cb_rt_type_check(void* node)`.
    pub(super) fn rt_type_check(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt("cb_rt_type_check", p.fn_type(&[p.into()], false))
    }

    /// `void* cb_rt_type_first(int64_t type_def)`.
    pub(super) fn rt_type_first(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_type_first",
            p.fn_type(&[self.ctx.i64_type().into()], false),
        )
    }

    /// `void* cb_rt_type_last(int64_t type_def)`.
    pub(super) fn rt_type_last(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt(
            "cb_rt_type_last",
            p.fn_type(&[self.ctx.i64_type().into()], false),
        )
    }

    /// `void* cb_rt_type_next(void* node)`.
    pub(super) fn rt_type_next(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt("cb_rt_type_next", p.fn_type(&[p.into()], false))
    }

    /// `void* cb_rt_type_previous(void* node)`.
    pub(super) fn rt_type_previous(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt("cb_rt_type_previous", p.fn_type(&[p.into()], false))
    }

    /// `void cb_rt_type_delete_rvalue(void* node)`.
    pub(super) fn rt_type_delete_rvalue(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        let fty = self.ctx.void_type().fn_type(&[p.into()], false);
        self.declare_rt("cb_rt_type_delete_rvalue", fty)
    }

    /// `void* cb_rt_type_delete_lvalue(void* node)`.
    pub(super) fn rt_type_delete_lvalue(&self) -> FunctionValue<'ctx> {
        let p = self.ptr_t();
        self.declare_rt("cb_rt_type_delete_lvalue", p.fn_type(&[p.into()], false))
    }

    // ── Catalog functions (Call to a Runtime callee) ────────────────────

    /// Declare a catalog runtime function from its IR signature, caching by
    /// `symbol`. Errors if any param/return type is out of scope (e.g. a
    /// runtime function taking an opaque handle), so such a call fails loudly.
    pub(super) fn rt_catalog(
        &self,
        symbol: &str,
        sig: &FnSig,
    ) -> Result<FunctionValue<'ctx>, String> {
        if let Some(f) = self.runtime.borrow().get(symbol) {
            return Ok(*f);
        }
        let fty =
            fn_type(self.ctx, &self.program.struct_defs, &sig.params, &sig.ret).map_err(|e| {
                format!("runtime function {symbol:?} has an unsupported signature: {e}")
            })?;
        let f = self
            .module
            .add_function(symbol, fty, Some(Linkage::External));
        self.runtime.borrow_mut().insert(symbol.to_string(), f);
        Ok(f)
    }
}
