// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(unsafe_code)]

use core::ffi::c_void;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MdlKind {
    NonPaged,
    UserLocked,
}

#[cfg(all(feature = "driver", not(miri)))]
use wdk_sys::{MDL_MAPPED_TO_SYSTEM_VA, MDL_PAGES_LOCKED, PMDL};
#[cfg(all(feature = "driver", not(miri)))]
use wdk_sys::ntddk::{
    IoAllocateMdl, IoFreeMdl, MmBuildMdlForNonPagedPool, MmUnmapLockedPages, MmUnlockPages,
};

#[cfg(any(not(feature = "driver"), miri))]
type PMDL = *mut c_void;

pub struct MdlGuard {
    mdl: PMDL,
    kind: MdlKind,
}

impl MdlGuard {
    /// # Safety
    /// `buffer` must be a valid nonpaged kernel pointer for `len` bytes.
    #[cfg(all(feature = "driver", not(miri)))]
    pub unsafe fn from_nonpaged(buffer: *mut c_void, len: u32) -> Option<Self> {
        let mdl = unsafe { IoAllocateMdl(buffer, len, 0, 0, core::ptr::null_mut()) };
        if mdl.is_null() {
            return None;
        }
        unsafe { MmBuildMdlForNonPagedPool(mdl) };
        Some(Self {
            mdl,
            kind: MdlKind::NonPaged,
        })
    }

    /// # Safety
    /// `mdl` must be a valid MDL pointer with ownership transferred to this guard.
    pub unsafe fn from_raw(mdl: PMDL, kind: MdlKind) -> Option<Self> {
        if mdl.is_null() {
            return None;
        }
        Some(Self { mdl, kind })
    }

    pub fn mdl(&self) -> PMDL {
        self.mdl
    }

    /// # Safety
    /// Releases the MDL using the configured `MdlKind` policy.
    pub unsafe fn release(mut self) {
        unsafe { free_mdl(self.mdl, self.kind) };
        self.mdl = core::ptr::null_mut();
    }
}

/// # Safety
/// `buffer` must be a valid nonpaged kernel pointer for `len` bytes.
#[cfg(all(feature = "driver", not(miri)))]
pub unsafe fn alloc_nonpaged_mdl(buffer: *mut c_void, len: u32) -> Option<PMDL> {
    let mdl = unsafe { IoAllocateMdl(buffer, len, 0, 0, core::ptr::null_mut()) };
    if mdl.is_null() {
        return None;
    }
    unsafe { MmBuildMdlForNonPagedPool(mdl) };
    Some(mdl)
}

#[cfg(any(not(feature = "driver"), miri))]
pub unsafe fn alloc_nonpaged_mdl(_buffer: *mut c_void, _len: u32) -> Option<PMDL> {
    None
}

/// # Safety
/// Normalizes an MDL that will be used for a nonpaged buffer, clearing any user-lock
/// state before rebuilding it for nonpaged pool access.
#[cfg(all(feature = "driver", not(miri)))]
pub unsafe fn normalize_nonpaged_mdl(mdl: PMDL) {
    if mdl.is_null() {
        return;
    }
    unsafe {
        if ((*mdl).MdlFlags as u32 & MDL_PAGES_LOCKED) != 0 {
            MmUnlockPages(mdl);
        }
        MmBuildMdlForNonPagedPool(mdl);
    }
}

#[cfg(any(not(feature = "driver"), miri))]
pub unsafe fn normalize_nonpaged_mdl(_mdl: PMDL) {}

/// # Safety
/// Frees an MDL, unmapping and unlocking as needed.
#[cfg(all(feature = "driver", not(miri)))]
pub unsafe fn free_mdl(mdl: PMDL, kind: MdlKind) {
    if mdl.is_null() {
        return;
    }
    unsafe {
        if ((*mdl).MdlFlags as u32 & MDL_MAPPED_TO_SYSTEM_VA) != 0 {
            let base = (*mdl).MappedSystemVa;
            if !base.is_null() {
                MmUnmapLockedPages(base, mdl);
            }
        }
        if kind == MdlKind::UserLocked || ((*mdl).MdlFlags as u32 & MDL_PAGES_LOCKED) != 0 {
            MmUnlockPages(mdl);
        }
        IoFreeMdl(mdl);
    }
}

#[cfg(any(not(feature = "driver"), miri))]
pub unsafe fn free_mdl(_mdl: PMDL, _kind: MdlKind) {}
