//! RAII wrapper around a runtime `CbString*` handle.
//!
//! `Value::String` stores one of these instead of `Rc<str>`. `Clone` retains,
//! `Drop` releases — so handle lifetime tracking falls out naturally from
//! ordinary Rust ownership. The thin Rust shell never reaches into the
//! runtime's struct layout; everything goes through the function pointers
//! on `CbStringApi`.

#![allow(unsafe_code)]

use std::fmt;

use cb_runtime_sys::{CbString, CbStringApi};

/// Refcounted handle to a runtime-managed UTF-8 string. The `'static`
/// lifetime on `api` is fine — `CbStringApi` lives in `.rodata` of the
/// loaded runtime library, never moves, never drops.
pub struct CbStringHandle {
    ptr: *mut CbString,
    api: &'static CbStringApi,
}

impl CbStringHandle {
    /// Wrap a freshly-returned `CbString*` (refcount already at 1).
    /// Does NOT call retain — assumes the caller transfers ownership.
    /// Used at FFI return-value sites.
    pub fn from_raw(api: &'static CbStringApi, ptr: *mut CbString) -> Self {
        Self { ptr, api }
    }

    /// Construct a handle from a byte slice — allocates a new runtime
    /// string with refcount = 1 and copies the bytes inline. Used for
    /// CB string literals and numeric-to-string coercions.
    pub fn from_bytes(api: &'static CbStringApi, bytes: &[u8]) -> Self {
        let ptr = unsafe { (api.from_literal)(bytes.as_ptr(), bytes.len()) };
        Self { ptr, api }
    }

    /// Handle to the canonical empty-string sentinel. retain/release are
    /// no-ops on the sentinel, so this is allocation-free and the same
    /// pointer value every time — useful for default-init of `String`
    /// locals and the `""` literal.
    pub fn empty(api: &'static CbStringApi) -> Self {
        // The sentinel's refcount is negative; retain is a documented
        // no-op. We still call it for symmetry with the Drop path.
        let ptr = unsafe { (api.retain)(api.empty as *mut CbString) };
        Self { ptr, api }
    }

    pub fn as_ptr(&self) -> *mut CbString {
        self.ptr
    }

    pub fn len(&self) -> usize {
        unsafe { (self.api.len)(self.ptr) }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Borrow the inline bytes. The slice is valid for as long as this
    /// handle keeps the underlying string alive — i.e., the borrow is
    /// tied to `&self`.
    pub fn as_bytes(&self) -> &[u8] {
        let len = self.len();
        if len == 0 {
            return &[];
        }
        let data = unsafe { (self.api.data)(self.ptr) };
        unsafe { std::slice::from_raw_parts(data, len) }
    }

    /// Best-effort UTF-8 view of the bytes. Lossy on invalid UTF-8.
    pub fn as_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.as_bytes())
    }

    /// Concatenate two handles, producing a fresh handle owned by the
    /// caller (refcount = 1, or a retained sentinel for the empty case).
    pub fn concat(&self, other: &Self) -> Self {
        let ptr = unsafe { (self.api.concat)(self.ptr, other.ptr) };
        Self { ptr, api: self.api }
    }
}

impl Clone for CbStringHandle {
    fn clone(&self) -> Self {
        unsafe { (self.api.retain)(self.ptr) };
        Self {
            ptr: self.ptr,
            api: self.api,
        }
    }
}

impl Drop for CbStringHandle {
    fn drop(&mut self) {
        unsafe { (self.api.release)(self.ptr) };
    }
}

impl fmt::Debug for CbStringHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CbStringHandle({:?})", self.as_str_lossy())
    }
}

impl fmt::Display for CbStringHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cb_runtime_sys::cb_rt_string_test_refcount;

    #[test]
    fn empty_handle_uses_sentinel() {
        let api = cb_runtime_sys::string_api();
        let h = CbStringHandle::empty(api);
        assert_eq!(h.as_ptr() as *const _, api.empty);
        assert_eq!(h.len(), 0);
        assert!(h.is_empty());
        assert_eq!(h.as_bytes(), b"");

        // Sentinel refcount is negative and never changes.
        let rc = unsafe { cb_rt_string_test_refcount(h.as_ptr()) };
        assert!(
            rc < 0,
            "empty handle should wrap the static sentinel, got refcount {rc}"
        );
        drop(h);
        // After drop, sentinel refcount unchanged (release was a no-op).
        assert_eq!(unsafe { cb_rt_string_test_refcount(api.empty) }, rc);
    }

    #[test]
    fn from_bytes_then_clone_then_drop_balances_refcount() {
        let api = cb_runtime_sys::string_api();
        let h = CbStringHandle::from_bytes(api, b"hello, cb");
        assert_eq!(h.as_bytes(), b"hello, cb");
        let raw = h.as_ptr();
        assert_eq!(unsafe { cb_rt_string_test_refcount(raw) }, 1);

        // Clone bumps the refcount; drop brings it back.
        let h2 = h.clone();
        assert_eq!(unsafe { cb_rt_string_test_refcount(raw) }, 2);
        assert_eq!(h2.as_bytes(), b"hello, cb");
        drop(h2);
        assert_eq!(unsafe { cb_rt_string_test_refcount(raw) }, 1);

        // h still keeps the underlying alive; dropping it frees the block.
        // (We can't probe refcount after free without UB; trust the C-side
        // primitives' tests in cb-runtime-sys to cover that.)
        drop(h);
    }

    #[test]
    fn concat_via_wrapper() {
        let api = cb_runtime_sys::string_api();
        let a = CbStringHandle::from_bytes(api, b"foo");
        let b = CbStringHandle::from_bytes(api, b"bar");
        let ab = a.concat(&b);
        assert_eq!(ab.as_bytes(), b"foobar");
        assert_eq!(ab.len(), 6);

        // Concat with empty returns retained copy of non-empty side.
        let empty = CbStringHandle::empty(api);
        let a_plus_empty = a.concat(&empty);
        assert_eq!(a_plus_empty.as_ptr(), a.as_ptr());
        // a was at refcount 1 before, plus we cloned via concat → 2.
        assert_eq!(unsafe { cb_rt_string_test_refcount(a.as_ptr()) }, 2);
    }
}
