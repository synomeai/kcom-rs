// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::iunknown::{IUnknownVtbl, Status, StatusResult};

/// Marker trait for types that are valid COM interfaces.
///
/// # Safety
/// Implementors guarantee that:
/// 1. The type is `Sized` (no fat pointers allowed).
/// 2. The type is `#[repr(C)]` or `#[repr(transparent)]` and has the same memory layout
///    as a COM interface pointer (the first field is a pointer to the vtable).
/// 3. The vtable begins with the `IUnknown` methods.
pub unsafe trait ComInterface: Sized {}

/// Marker trait for COM interfaces that are free-threaded and safe to share.
///
/// # Safety
/// Implementors guarantee that the underlying COM object supports concurrent
/// calls from multiple threads and that reference counting is thread-safe.
pub unsafe trait ThreadSafeComInterface: ComInterface {}

/// Reference-counted COM interface pointer.
///
/// # Safety
/// The pointer must be a valid COM interface pointer whose vtable begins with
/// `IUnknown` methods.
///
/// # Thread Safety
/// This type does not implement `Send` or `Sync` by default because many COM
/// interfaces are thread-affine. For free-threaded interfaces, implement
/// [`ThreadSafeComInterface`] and `ComRc` will become `Send + Sync`.
pub struct ComRc<T: ComInterface> {
    ptr: NonNull<T>,
    _phantom: PhantomData<T>,
}

unsafe impl<T: ThreadSafeComInterface> Send for ComRc<T> {}
unsafe impl<T: ThreadSafeComInterface> Sync for ComRc<T> {}

impl<T: ComInterface> ComRc<T> {
    /// Takes ownership of a raw COM pointer without calling `AddRef`.
    ///
    /// # Safety
    /// `ptr` must be a valid COM interface pointer.
    pub unsafe fn from_raw(ptr: *mut T) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self {
            ptr,
            _phantom: PhantomData,
        })
    }

    /// Takes ownership of a raw COM pointer without calling `AddRef`.
    ///
    /// # Safety
    /// `ptr` must be a valid COM interface pointer.
    pub unsafe fn try_from_raw(ptr: *mut T) -> Option<Self> {
        Self::from_raw(ptr)
    }

    /// Takes ownership of a non-null raw COM pointer without calling `AddRef`.
    ///
    /// # Safety
    /// `ptr` must be a valid, non-null COM interface pointer.
    pub unsafe fn from_raw_unchecked(ptr: *mut T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr),
            _phantom: PhantomData,
        }
    }

    /// Takes ownership of a raw COM pointer or returns `Status::NOINTERFACE` if null.
    ///
    /// # Safety
    /// `ptr` must be a valid COM interface pointer when non-null.
    pub unsafe fn from_raw_or_status(ptr: *mut T) -> StatusResult<Self> {
        NonNull::new(ptr)
            .map(|ptr| Self {
                ptr,
                _phantom: PhantomData,
            })
            .ok_or(Status::NOINTERFACE)
    }

    /// Takes ownership of a raw COM pointer and calls `AddRef` first.
    ///
    /// # Safety
    /// `ptr` must be a valid COM interface pointer.
    pub unsafe fn from_raw_addref(ptr: *mut T) -> Option<Self> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: caller guarantees `ptr` is a valid COM interface pointer.
        unsafe { add_ref(ptr) };
        // SAFETY: caller guarantees `ptr` is a valid COM interface pointer.
        unsafe { Self::from_raw(ptr) }
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    #[inline]
    pub fn into_raw(self) -> *mut T {
        let ptr = self.ptr.as_ptr();
        core::mem::forget(self);
        ptr
    }

    /// Queries for another COM interface and returns a smart pointer on success.
    pub fn query_interface<U>(&self) -> StatusResult<ComRc<U>>
    where
        U: ComInterface + crate::vtable::ComInterfaceInfo,
    {
        let mut out = core::ptr::null_mut();
        let vtbl = unsafe { *(self.ptr.as_ptr() as *mut *mut IUnknownVtbl) };
        let status = unsafe {
            ((*vtbl).QueryInterface)(
                self.ptr.as_ptr() as *mut c_void,
                &U::IID,
                &mut out,
            )
        };
        let status = Status::from_raw(status);
        if status.is_error() {
            return Err(status);
        }
        unsafe { ComRc::<U>::from_raw_or_status(out as *mut U) }
    }
}

impl<T: ComInterface> core::ops::Deref for ComRc<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ComInterface> Clone for ComRc<T> {
    fn clone(&self) -> Self {
        unsafe { add_ref(self.ptr.as_ptr()) };
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ComInterface> Drop for ComRc<T> {
    fn drop(&mut self) {
        unsafe { release(self.ptr.as_ptr()) };
    }
}

unsafe fn add_ref<T: ComInterface>(ptr: *mut T) -> u32 {
    let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
    unsafe { ((*vtbl).AddRef)(ptr as *mut c_void) }
}

unsafe fn release<T: ComInterface>(ptr: *mut T) -> u32 {
    let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
    unsafe { ((*vtbl).Release)(ptr as *mut c_void) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{declare_com_interface, impl_com_interface, impl_com_object, GUID, NTSTATUS, STATUS_SUCCESS};
    use crate::wrapper::ComObject;
    use core::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    static DROP_COUNT: AtomicU32 = AtomicU32::new(0);
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[repr(C)]
    #[allow(non_snake_case)]
    struct IUnknownRaw {
        #[allow(non_snake_case)]
        lpVtbl: *mut IUnknownVtbl,
    }

    unsafe impl ComInterface for IUnknownRaw {}

    struct Dummy;

    impl Drop for Dummy {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn from_raw_addref_balances_release() {
        let _guard = TEST_LOCK.lock().unwrap();
        DROP_COUNT.store(0, Ordering::Relaxed);
        let raw = ComObject::<Dummy, IUnknownVtbl>::new(Dummy).unwrap();

        let com = unsafe { ComRc::<IUnknownRaw>::from_raw_addref(raw as *mut IUnknownRaw).unwrap() };
        drop(com);

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 0);

        unsafe {
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(raw), 0);
        }

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn from_raw_consumes_reference() {
        let _guard = TEST_LOCK.lock().unwrap();
        DROP_COUNT.store(0, Ordering::Relaxed);
        let raw = ComObject::<Dummy, IUnknownVtbl>::new(Dummy).unwrap();

        let com = unsafe { ComRc::<IUnknownRaw>::from_raw(raw as *mut IUnknownRaw).unwrap() };
        drop(com);

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn clone_adds_reference() {
        let _guard = TEST_LOCK.lock().unwrap();
        DROP_COUNT.store(0, Ordering::Relaxed);
        let raw = ComObject::<Dummy, IUnknownVtbl>::new(Dummy).unwrap();

        let com = unsafe { ComRc::<IUnknownRaw>::from_raw_addref(raw as *mut IUnknownRaw).unwrap() };
        let com_clone = com.clone();
        drop(com);
        drop(com_clone);

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 0);

        unsafe {
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(raw), 0);
        }

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    declare_com_interface! {
        pub trait IFoo: IUnknown {
            const IID: GUID = GUID {
                data1: 0xAA55_AA55,
                data2: 0x1234,
                data3: 0x5678,
                data4: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            };

            fn ping(&self) -> NTSTATUS;
        }
    }

    impl IFoo for Dummy {
        fn ping(&self) -> NTSTATUS {
            STATUS_SUCCESS
        }
    }

    impl_com_interface! {
        impl Dummy: IFoo {
            parent = IUnknownVtbl,
            methods = [ping],
        }
    }

    impl_com_object!(Dummy, IFooVtbl);

    #[test]
    fn query_interface_returns_comrc() {
        let _guard = TEST_LOCK.lock().unwrap();
        let raw = Dummy::new_com(Dummy).unwrap();
        let com = unsafe { ComRc::<IFooRaw>::from_raw_addref(raw as *mut IFooRaw).unwrap() };

        let queried = com.query_interface::<IFooRaw>().unwrap();
        drop(queried);
        drop(com);

        unsafe {
            assert_eq!(ComObject::<Dummy, IFooVtbl>::shim_release(raw), 0);
        }
    }

    #[test]
    fn from_raw_or_status_rejects_null() {
        let err = unsafe { ComRc::<IUnknownRaw>::from_raw_or_status(core::ptr::null_mut()) };
        assert!(matches!(err, Err(Status::NOINTERFACE)));
    }
}
