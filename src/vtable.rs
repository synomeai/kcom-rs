// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::ffi::c_void;

use crate::iunknown::GUID;

/// Trait marking a struct as a VTable layout.
/// # Safety
/// The struct must have the same memory layout as the COM interface VTable.
pub unsafe trait InterfaceVtable: Sized + 'static {}

/// Metadata associated with a COM interface (IID, VTable type).
pub trait ComInterfaceInfo {
    type Vtable: InterfaceVtable;
    const IID: GUID;
    const IID_STR: &'static str;
}

/// Returns `Some(ptr)` when `riid` matches `T::IID`, otherwise `None`.
///
/// This helper reduces boilerplate in manual `query_interface` implementations
/// and ensures null pointers are not returned on match.
#[inline]
pub fn match_interface_ptr<T: ComInterfaceInfo>(riid: &GUID, ptr: *mut c_void) -> Option<*mut c_void> {
    if *riid != T::IID {
        return None;
    }
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}
