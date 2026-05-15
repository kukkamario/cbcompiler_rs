//! Backend-agnostic intermediate representation for CoolBasic.
//!
//! Both [`cb_backend_interp`] and [`cb_backend_llvm`] consume this IR.
//! Do not leak backend-specific types (LLVM, etc.) into this crate.
