// traits.rs

// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::ffi::c_void;
use crate::iunknown::{GUID, IUnknownVtbl, IID_IUNKNOWN};
use crate::vtable::{ComInterfaceInfo, InterfaceVtable};

/// Implementation logic for a COM interface.
///
/// # Safety
/// The wrapper shims trust both items without further checks:
/// - `VTABLE` must be a vtable of layout `I` whose IUnknown slots are the
///   `ComObject`/`ComObjectN` shims for `Self` (or equivalent), since the
///   wrapper hands it out as the object's interface pointer.
/// - `query_interface` must only return live interface pointers of this object
///   (or of an object it keeps alive) whose vtable matches `riid`; the wrapper
///   calls `AddRef` through the returned pointer and gives it to the caller.
///
/// The `impl_com_interface!` family of macros upholds this.
pub unsafe trait ComImpl<I: InterfaceVtable>: Sized + Sync + 'static {
    /// The VTable instance that delegates to `ComObject` shims.
    const VTABLE: &'static I;

    /// Checks if this object supports the interface `riid`.
    /// Returns a *stable COM interface pointer* if supported.
    ///
    /// The returned pointer must reference a vtable matching `riid`.
    /// The `this` pointer is expected to be the canonical primary interface
    /// pointer (vtable at offset 0). Returning `this` is only valid for the
    /// primary interface whose vtable begins at offset 0 within the object.
    /// For other interfaces, return explicit tear-offs or aggregated pointers.
    /// The `ComObject` wrapper performs `AddRef` on returned pointers.

    fn query_interface(&self, this: *mut c_void, riid: &GUID) -> Option<*mut c_void>;
}

/// Marker trait for any type that can be a COM object inner.
pub trait IUnknown {}
impl<T: ?Sized> IUnknown for T {}

pub struct IUnknownInterface;

impl ComInterfaceInfo for IUnknownInterface {
    type Vtable = IUnknownVtbl;
    const IID: GUID = IID_IUNKNOWN;
    const IID_STR: &'static str = "IID_IUNKNOWN";
}

// Default implementation for IUnknown logic on the inner type.
// SAFETY: the vtable is the ComObject shims and no interface is returned.
unsafe impl<T> ComImpl<IUnknownVtbl> for T
where
    T: IUnknown + Sync + 'static,
{
    const VTABLE: &'static IUnknownVtbl = &IUnknownVtbl::new::<T, IUnknownVtbl>();

    #[inline]
    fn query_interface(&self, _this: *mut c_void, _riid: &GUID) -> Option<*mut c_void> {
        // FIX: Never return `self` here. The inner object T is NOT a COM pointer.
        // The wrapper handles IID_IUNKNOWN.
        None
    }
}
