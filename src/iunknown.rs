// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::ffi::c_void;

use crate::traits::ComImpl;
use crate::vtable::InterfaceVtable;
use crate::wrapper::{ComObject, ComObjectN, SecondaryComImpl, SecondaryList, SecondaryVtables};

pub type NTSTATUS = i32;

pub const STATUS_SUCCESS: NTSTATUS = 0;
pub const STATUS_PENDING: NTSTATUS = 0x0000_0103u32 as i32;
pub const STATUS_UNSUCCESSFUL: NTSTATUS = 0xC000_0001u32 as i32;
pub const STATUS_INVALID_PARAMETER: NTSTATUS = 0xC000_000Du32 as i32;
pub const STATUS_NOT_SUPPORTED: NTSTATUS = 0xC000_00BBu32 as i32;
pub const STATUS_CANCELLED: NTSTATUS = 0xC000_0120u32 as i32;
pub const STATUS_NOINTERFACE: NTSTATUS = 0xC000_02B9u32 as i32;
pub const STATUS_INSUFFICIENT_RESOURCES: NTSTATUS = 0xC000_009Au32 as i32;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct Status(pub NTSTATUS);

impl Status {
    pub const SUCCESS: Status = Status(STATUS_SUCCESS);
    pub const UNSUCCESSFUL: Status = Status(STATUS_UNSUCCESSFUL);
    pub const INVALID_PARAMETER: Status = Status(STATUS_INVALID_PARAMETER);
    pub const NOINTERFACE: Status = Status(STATUS_NOINTERFACE);
    pub const INSUFFICIENT_RESOURCES: Status = Status(STATUS_INSUFFICIENT_RESOURCES);
    pub const CANCELLED: Status = Status(STATUS_CANCELLED);
    pub const PENDING: Status = Status(STATUS_PENDING);

    #[inline]
    pub const fn from_raw(raw: NTSTATUS) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn into_raw(self) -> NTSTATUS {
        self.0
    }

    #[inline]
    pub const fn is_success(self) -> bool {
        self.0 >= 0
    }

    #[inline]
    pub const fn is_error(self) -> bool {
        self.0 < 0
    }

    #[inline]
    pub fn to_result(self) -> Result<(), Status> {
        if self.is_success() {
            Ok(())
        } else {
            Err(self)
        }
    }

    /// Converts this status into a result that distinguishes STATUS_PENDING.
    #[inline]
    pub fn to_pending_result(self) -> Result<PendingResult, Status> {
        if self.0 == STATUS_PENDING {
            Ok(PendingResult::Pending)
        } else if self.is_success() {
            Ok(PendingResult::Ready(()))
        } else {
            Err(self)
        }
    }
}

impl From<NTSTATUS> for Status {
    #[inline]
    fn from(value: NTSTATUS) -> Self {
        Status(value)
    }
}

impl From<Status> for NTSTATUS {
    #[inline]
    fn from(value: Status) -> Self {
        value.0
    }
}

pub type StatusResult<T = ()> = Result<T, Status>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingResult<T = ()> {
    Ready(T),
    Pending,
}

pub trait IntoNtStatus {
    fn into_ntstatus(self) -> NTSTATUS;
}

impl IntoNtStatus for NTSTATUS {
    #[inline]
    fn into_ntstatus(self) -> NTSTATUS {
        self
    }
}

impl IntoNtStatus for Status {
    #[inline]
    fn into_ntstatus(self) -> NTSTATUS {
        self.0
    }
}

impl<T, E> IntoNtStatus for Result<T, E>
where
    E: Into<NTSTATUS>,
{
    #[inline]
    fn into_ntstatus(self) -> NTSTATUS {
        match self {
            Ok(_) => STATUS_SUCCESS,
            Err(err) => err.into(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const IID_IUNKNOWN: GUID = GUID {
    data1: 0x0000_0000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
pub struct IUnknownVtbl {
    pub QueryInterface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> NTSTATUS,
    pub AddRef: unsafe extern "system" fn(*mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(*mut c_void) -> u32,
}

unsafe impl InterfaceVtable for IUnknownVtbl {}

impl IUnknownVtbl {
    /// Compile-time construction of the IUnknown vtable for a given COM type.
    pub const fn new<T, I>() -> Self
    where
        T: ComImpl<I>,
        I: InterfaceVtable,
    {
        Self {
            QueryInterface: ComObject::<T, I>::shim_query_interface,
            AddRef: ComObject::<T, I>::shim_add_ref,
            Release: ComObject::<T, I>::shim_release,
        }
    }

    /// Compile-time construction of the IUnknown vtable for a ComObjectN primary interface.
    pub const fn new_primary<T, P, S, A>() -> Self
    where
        T: ComImpl<P> + SecondaryComImpl<S>,
        P: InterfaceVtable,
        S: SecondaryVtables,
        S::Entries: SecondaryList,
        A: crate::allocator::Allocator + Send + Sync,
    {
        Self {
            QueryInterface: ComObjectN::<T, P, S, A>::shim_query_interface,
            AddRef: ComObjectN::<T, P, S, A>::shim_add_ref,
            Release: ComObjectN::<T, P, S, A>::shim_release,
        }
    }
}
