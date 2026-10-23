// async_com.rs
//
// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::future::Future;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};
#[cfg(test)]
use core::sync::atomic::AtomicUsize;

use crate::executor::{spawn_dpc_task_cancellable, CancelHandle};
#[cfg(all(
    feature = "driver",
    feature = "async-com-kernel",
    driver_model__driver_type = "WDM",
    not(miri)
))]
use crate::executor::spawn_task;
#[cfg(all(
    feature = "driver",
    feature = "async-com-kernel",
    driver_model__driver_type = "WDM",
    not(miri)
))]
use crate::ntddk::DEVICE_OBJECT;
use crate::iunknown::{
    GUID, IUnknownVtbl, NTSTATUS, STATUS_CANCELLED, STATUS_PENDING, STATUS_SUCCESS,
    STATUS_UNSUCCESSFUL,
};
use crate::GuardPtr;
use crate::smart_ptr::{ComInterface, ComRc};
use crate::traits::ComImpl;
use crate::vtable::InterfaceVtable;
use crate::wrapper::{ComObject, PanicGuard};

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
mod pool;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub use pool::{init_async_com_pool_for, set_async_com_pool_depth, set_async_com_pool_tag};
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub use pool::shutdown_async_com_pool_for;

#[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
mod fused;
#[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
pub use fused::init_async_com_slabs;

#[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
struct ReleaseGuard {
    ptr: GuardPtr,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
impl ReleaseGuard {
    #[inline]
    fn new(ptr: GuardPtr, release: unsafe extern "system" fn(*mut c_void) -> u32) -> Self {
        Self { ptr, release }
    }
}

#[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        unsafe { (self.release)(self.ptr.as_ptr()) };
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsyncStatus {
    Started = 0,
    Completed = 1,
    Canceled = 2,
    Error = 3,
}

impl AsyncStatus {
    #[inline]
    pub const fn as_raw(self) -> u32 {
        self as u32
    }

    #[inline]
    pub const fn from_raw(value: u32) -> Self {
        match value {
            0 => Self::Started,
            1 => Self::Completed,
            2 => Self::Canceled,
            _ => Self::Error,
        }
    }
}

pub trait AsyncValueType: Copy + Send + Sync + 'static {}

impl<T> AsyncValueType for T where T: Copy + Send + Sync + 'static {}

#[repr(C)]
pub struct AsyncOperationVtbl<T: AsyncValueType> {
    pub parent: IUnknownVtbl,
    pub get_status: unsafe extern "system" fn(*mut c_void, *mut AsyncStatus) -> NTSTATUS,
    pub get_result: unsafe extern "system" fn(*mut c_void, *mut T) -> NTSTATUS,
}

unsafe impl<T: AsyncValueType> InterfaceVtable for AsyncOperationVtbl<T> {}

impl<T: AsyncValueType> AsyncOperationVtbl<T> {
    pub const fn new<F>() -> Self
    where
        F: Future<Output = T> + Send + 'static,
    {
        Self {
            parent: IUnknownVtbl::new::<AsyncOperationTask<T, F>, Self>(),
            get_status: AsyncOperationTask::<T, F>::shim_get_status,
            get_result: AsyncOperationTask::<T, F>::shim_get_result,
        }
    }
}

#[repr(C)]
#[allow(non_snake_case)]
pub struct AsyncOperationRaw<T: AsyncValueType> {
    pub lpVtbl: *mut AsyncOperationVtbl<T>,
}

unsafe impl<T: AsyncValueType> ComInterface for AsyncOperationRaw<T> {}

impl<T: AsyncValueType> AsyncOperationRaw<T> {
    #[inline]
    pub unsafe fn get_status(&self) -> Result<AsyncStatus, NTSTATUS> {
        unsafe { Self::get_status_raw(self as *const _ as *mut Self) }
    }

    #[inline]
    pub unsafe fn get_result(&self) -> Result<T, NTSTATUS> {
        unsafe { Self::get_result_raw(self as *const _ as *mut Self) }
    }

    #[inline]
    pub unsafe fn get_status_raw(this: *mut Self) -> Result<AsyncStatus, NTSTATUS> {
        if this.is_null() {
            return Err(STATUS_UNSUCCESSFUL);
        }
        let vtbl = unsafe { (*this).lpVtbl };
        if vtbl.is_null() {
            return Err(STATUS_UNSUCCESSFUL);
        }
        let mut status = AsyncStatus::Started;
        let result = unsafe { ((*vtbl).get_status)(this as *mut c_void, &mut status as *mut AsyncStatus) };
        if result < 0 {
            Err(result)
        } else {
            Ok(status)
        }
    }

    #[inline]
    pub unsafe fn get_result_raw(this: *mut Self) -> Result<T, NTSTATUS> {
        if this.is_null() {
            return Err(STATUS_UNSUCCESSFUL);
        }
        let vtbl = unsafe { (*this).lpVtbl };
        if vtbl.is_null() {
            return Err(STATUS_UNSUCCESSFUL);
        }
        let mut out = core::mem::MaybeUninit::<T>::uninit();
        let result = unsafe { ((*vtbl).get_result)(this as *mut c_void, out.as_mut_ptr()) };
        if result == STATUS_SUCCESS {
            Ok(unsafe { out.assume_init() })
        } else {
            Err(result)
        }
    }
}

// Kernel builds draw task objects from a per-layout lookaside pool; host and
// Miri builds use the global allocator. (A generic alias that ignores `T` is
// rejected with E0091, so the alias is split on the object type instead.)
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
type TaskComObject<T, F> =
    ComObject<AsyncOperationTask<T, F>, AsyncOperationVtbl<T>, pool::AsyncComAlloc<T>>;
#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
type TaskComObject<T, F> =
    ComObject<AsyncOperationTask<T, F>, AsyncOperationVtbl<T>, crate::allocator::GlobalAllocator>;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
fn async_com_alloc<T: AsyncValueType>() -> pool::AsyncComAlloc<T> {
    pool::AsyncComAlloc::<T>::new()
}

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
#[inline]
fn async_com_alloc<T: AsyncValueType>() -> crate::allocator::GlobalAllocator {
    crate::allocator::GlobalAllocator
}

pub struct AsyncOperationTask<T, F>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    status: AtomicU32,
    error: AtomicI32,
    result: UnsafeCell<MaybeUninit<T>>,
    _marker: PhantomData<F>,
}

#[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
#[doc(hidden)]
pub fn spawn_async_operation_raw_with_init<T, F, A>(
    init: impl crate::allocator::InitBoxTrait<F, A, NTSTATUS>,
    guard_ptr: GuardPtr,
    release_fn: unsafe extern "system" fn(*mut c_void) -> u32,
) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
    A: crate::allocator::Allocator + Send + Sync,
{
    let guard = ReleaseGuard::new(guard_ptr, release_fn);
    fused::spawn_raw_with_init(init, Some(guard))
}

#[cfg(test)]
static ASYNC_OPERATION_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl<T, F> Sync for AsyncOperationTask<T, F>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
}

#[cfg(test)]
impl<T, F> Drop for AsyncOperationTask<T, F>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    fn drop(&mut self) {
        ASYNC_OPERATION_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

impl<T, F> AsyncOperationTask<T, F>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    #[inline]
    fn new_state() -> Self {
        Self {
            status: AtomicU32::new(AsyncStatus::Started.as_raw()),
            error: AtomicI32::new(STATUS_UNSUCCESSFUL),
            result: UnsafeCell::new(MaybeUninit::uninit()),
            _marker: PhantomData,
        }
    }

    #[inline]
    fn store_result(&self, value: T) {
        unsafe {
            (*self.result.get()).write(value);
        }
        self.error.store(STATUS_SUCCESS, Ordering::Release);
        self.status
            .store(AsyncStatus::Completed.as_raw(), Ordering::Release);
    }

    #[inline]
    fn store_error(&self, status: NTSTATUS) {
        self.error.store(status, Ordering::Release);
        self.status
            .store(AsyncStatus::Error.as_raw(), Ordering::Release);
    }

    #[inline]
    fn store_canceled(&self) {
        self.error.store(STATUS_CANCELLED, Ordering::Release);
        self.status
            .store(AsyncStatus::Canceled.as_raw(), Ordering::Release);
    }

    #[inline]
    fn load_status(&self) -> AsyncStatus {
        AsyncStatus::from_raw(self.status.load(Ordering::Acquire))
    }

    #[inline]
    fn load_error(&self) -> NTSTATUS {
        self.error.load(Ordering::Acquire)
    }

    #[inline]
    fn read_result(&self) -> T {
        unsafe { (*self.result.get()).assume_init() }
    }

    pub fn spawn_raw(future: F) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS> {
        let (ptr, _handle) = Self::spawn_raw_cancellable(future)?;
        Ok(ptr)
    }

    pub fn spawn_raw_cancellable(
        future: F,
    ) -> Result<(*mut AsyncOperationRaw<T>, CancelHandle), NTSTATUS> {
        let ptr = TaskComObject::<T, F>::new_in(Self::new_state(), async_com_alloc::<T>())?;

        // Hold a reference while the async task runs.
        unsafe {
            TaskComObject::<T, F>::shim_add_ref(ptr);
        }

        struct TaskGuard<T, F>
        where
            T: AsyncValueType,
            F: Future<Output = T> + Send + 'static,
        {
            ptr: GuardPtr,
            _marker: PhantomData<(T, F)>,
        }

        impl<T, F> Drop for TaskGuard<T, F>
        where
            T: AsyncValueType,
            F: Future<Output = T> + Send + 'static,
        {
            fn drop(&mut self) {
                unsafe {
                    TaskComObject::<T, F>::shim_release(
                        self.ptr.as_ptr(),
                    );
                }
            }
        }

        let task_ptr = GuardPtr::new(ptr);
        let task = async move {
            let _guard = TaskGuard::<T, F> {
                ptr: task_ptr,
                _marker: PhantomData,
            };

            let result = crate::task::try_finally(future, async {}).await;
            let wrapper = unsafe { TaskComObject::<T, F>::from_ptr(task_ptr.as_ptr()) };
            match result {
                Some(value) => wrapper.inner.store_result(value),
                None => wrapper.inner.store_canceled(),
            }
            STATUS_SUCCESS
        };

        let handle = match unsafe { spawn_dpc_task_cancellable(task) } {
            Ok(handle) => handle,
            Err(status) => {
                unsafe {
                    TaskComObject::<T, F>::shim_release(ptr);
                    TaskComObject::<T, F>::shim_release(ptr);
                }
                return Err(status);
            }
        };

        Ok((ptr as *mut AsyncOperationRaw<T>, handle))
    }

    #[cfg(all(
        feature = "driver",
        feature = "async-com-kernel",
        driver_model__driver_type = "WDM",
        not(miri)
    ))]
    pub fn spawn_raw_passive(
        device: *mut DEVICE_OBJECT,
        future: F,
    ) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS> {
        let ptr = TaskComObject::<T, F>::new_in(Self::new_state(), async_com_alloc::<T>())?;

        // Hold a reference while the async task runs.
        unsafe {
            TaskComObject::<T, F>::shim_add_ref(ptr);
        }

        struct TaskGuard<T, F>
        where
            T: AsyncValueType,
            F: Future<Output = T> + Send + 'static,
        {
            ptr: GuardPtr,
            _marker: PhantomData<(T, F)>,
        }

        impl<T, F> Drop for TaskGuard<T, F>
        where
            T: AsyncValueType,
            F: Future<Output = T> + Send + 'static,
        {
            fn drop(&mut self) {
                unsafe {
                    TaskComObject::<T, F>::shim_release(self.ptr.as_ptr());
                }
            }
        }

        let task_ptr = GuardPtr::new(ptr);
        let task = async move {
            let _guard = TaskGuard::<T, F> {
                ptr: task_ptr,
                _marker: PhantomData,
            };

            let result = crate::task::try_finally(future, async {}).await;
            let wrapper = unsafe { TaskComObject::<T, F>::from_ptr(task_ptr.as_ptr()) };
            match result {
                Some(value) => wrapper.inner.store_result(value),
                None => wrapper.inner.store_canceled(),
            }
            STATUS_SUCCESS
        };

        let status = spawn_task(device, task);
        if status < 0 {
            unsafe {
                TaskComObject::<T, F>::shim_release(ptr);
                TaskComObject::<T, F>::shim_release(ptr);
            }
            return Err(status);
        }

        Ok(ptr as *mut AsyncOperationRaw<T>)
    }

    pub fn spawn_error_raw(status: NTSTATUS) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS> {
        let task = Self::new_state();
        task.store_error(status);
        let ptr = TaskComObject::<T, F>::new_in(task, async_com_alloc::<T>())?;
        Ok(ptr as *mut AsyncOperationRaw<T>)
    }

    #[inline]
    pub fn spawn(future: F) -> Result<ComRc<AsyncOperationRaw<T>>, NTSTATUS> {
        let ptr = Self::spawn_raw(future)?;
        Ok(unsafe { ComRc::from_raw_unchecked(ptr) })
    }

    #[inline]
    pub fn spawn_cancellable(
        future: F,
    ) -> Result<(ComRc<AsyncOperationRaw<T>>, CancelHandle), NTSTATUS> {
        let (ptr, handle) = Self::spawn_raw_cancellable(future)?;
        let op = unsafe { ComRc::from_raw_unchecked(ptr) };
        Ok((op, handle))
    }

    #[allow(non_snake_case)]
    pub unsafe extern "system" fn shim_get_status(
        this: *mut c_void,
        out_status: *mut AsyncStatus,
    ) -> NTSTATUS {
        if this.is_null() || out_status.is_null() {
            return STATUS_UNSUCCESSFUL;
        }
        let guard = PanicGuard::new();
        let wrapper = unsafe { &*(this as *const TaskComObject<T, F>) };
        let status = wrapper.inner.load_status();
        unsafe {
            *out_status = status;
        }
        let result = STATUS_SUCCESS;
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    pub unsafe extern "system" fn shim_get_result(
        this: *mut c_void,
        out_result: *mut T,
    ) -> NTSTATUS {
        if this.is_null() || out_result.is_null() {
            return STATUS_UNSUCCESSFUL;
        }
        let guard = PanicGuard::new();
        let wrapper = unsafe { &*(this as *const TaskComObject<T, F>) };
        let result = match wrapper.inner.load_status() {
            AsyncStatus::Completed => {
                let value = wrapper.inner.read_result();
                unsafe {
                    out_result.write(value);
                }
                STATUS_SUCCESS
            }
            AsyncStatus::Started => STATUS_PENDING,
            AsyncStatus::Canceled | AsyncStatus::Error => wrapper.inner.load_error(),
        };
        core::mem::forget(guard);
        result
    }
}

impl<T, F> ComImpl<AsyncOperationVtbl<T>> for AsyncOperationTask<T, F>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    const VTABLE: &'static AsyncOperationVtbl<T> = &AsyncOperationVtbl::<T>::new::<F>();

    #[inline]
    fn query_interface(&self, _this: *mut c_void, _riid: &GUID) -> Option<*mut c_void> {
        None
    }
}

#[inline]
pub fn spawn_async_operation_raw<T, F>(future: F) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    #[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
    {
        return fused::spawn_raw(future);
    }
    #[cfg(any(not(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel")), miri))]
    {
        return AsyncOperationTask::<T, F>::spawn_raw(future);
    }
}

/// Spawn an async operation on the PASSIVE_LEVEL work-item executor (WDM).
#[cfg(all(
    feature = "driver",
    feature = "async-com-kernel",
    driver_model__driver_type = "WDM",
    not(miri)
))]
pub fn spawn_async_operation_raw_passive<T, F>(
    device: *mut DEVICE_OBJECT,
    future: F,
) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    AsyncOperationTask::<T, F>::spawn_raw_passive(device, future)
}

#[inline]
pub fn spawn_async_operation_raw_cancellable<T, F>(
    future: F,
) -> Result<(*mut AsyncOperationRaw<T>, CancelHandle), NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    AsyncOperationTask::<T, F>::spawn_raw_cancellable(future)
}

#[inline]
pub fn spawn_async_operation_error_raw<T, F>(status: NTSTATUS) -> Result<*mut AsyncOperationRaw<T>, NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    AsyncOperationTask::<T, F>::spawn_error_raw(status)
}

#[inline]
pub fn spawn_async_operation<T, F>(future: F) -> Result<ComRc<AsyncOperationRaw<T>>, NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    AsyncOperationTask::<T, F>::spawn(future)
}

/// Spawn an async operation on the PASSIVE_LEVEL work-item executor (WDM).
#[cfg(all(
    feature = "driver",
    feature = "async-com-kernel",
    driver_model__driver_type = "WDM",
    not(miri)
))]
pub fn spawn_async_operation_passive<T, F>(
    device: *mut DEVICE_OBJECT,
    future: F,
) -> Result<ComRc<AsyncOperationRaw<T>>, NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    let ptr = spawn_async_operation_raw_passive::<T, F>(device, future)?;
    Ok(unsafe { ComRc::from_raw_unchecked(ptr) })
}

#[inline]
pub fn spawn_async_operation_cancellable<T, F>(
    future: F,
) -> Result<(ComRc<AsyncOperationRaw<T>>, CancelHandle), NTSTATUS>
where
    T: AsyncValueType,
    F: Future<Output = T> + Send + 'static,
{
    AsyncOperationTask::<T, F>::spawn_cancellable(future)
}

#[inline]
pub fn spawn_async_operation_error<T>(status: NTSTATUS) -> Result<ComRc<AsyncOperationRaw<T>>, NTSTATUS>
where
    T: AsyncValueType,
{
    let ptr = spawn_async_operation_error_raw::<T, core::future::Ready<T>>(status)?;
    Ok(unsafe { ComRc::from_raw_unchecked(ptr) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(any(not(feature = "driver"), miri))]
    use core::sync::atomic::Ordering;

    #[test]
    fn ready_future_completes() {
        let _guard = TEST_LOCK.lock().unwrap();
        let op = spawn_async_operation(async { 11u32 }).expect("spawn async operation");
        unsafe {
            let status =
                AsyncOperationRaw::<u32>::get_status_raw(op.as_ptr()).expect("get status");
            assert_eq!(status, AsyncStatus::Completed);
            let result =
                AsyncOperationRaw::<u32>::get_result_raw(op.as_ptr()).expect("get result");
            assert_eq!(result, 11u32);
        }
    }

    #[test]
    fn error_operation_reports_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        let op = spawn_async_operation_error::<u32>(STATUS_UNSUCCESSFUL).expect("spawn error op");
        unsafe {
            let status =
                AsyncOperationRaw::<u32>::get_status_raw(op.as_ptr()).expect("get status");
            assert_eq!(status, AsyncStatus::Error);
            let result = AsyncOperationRaw::<u32>::get_result_raw(op.as_ptr());
            assert!(matches!(result, Err(STATUS_UNSUCCESSFUL)));
        }
    }

    #[cfg(any(not(feature = "driver"), miri))]
    #[test]
    fn pending_future_reports_pending() {
        let _guard = TEST_LOCK.lock().unwrap();
        let op = spawn_async_operation(core::future::pending::<u32>()).expect("spawn async operation");
        unsafe {
            let status =
                AsyncOperationRaw::<u32>::get_status_raw(op.as_ptr()).expect("get status");
            assert_eq!(status, AsyncStatus::Started);
            let result = AsyncOperationRaw::<u32>::get_result_raw(op.as_ptr());
            assert!(matches!(result, Err(STATUS_PENDING)));
        }
    }

    #[cfg(any(not(feature = "driver"), miri))]
    #[test]
    fn pending_future_drop_releases_task_ref() {
        let _guard = TEST_LOCK.lock().unwrap();
        ASYNC_OPERATION_DROP_COUNT.store(0, Ordering::Relaxed);
        let op = spawn_async_operation(core::future::pending::<u32>()).expect("spawn async operation");
        drop(op);
        assert_eq!(ASYNC_OPERATION_DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn null_out_ptrs_return_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        let op = spawn_async_operation(async { 1u32 }).expect("spawn async operation");
        unsafe {
            let raw = op.as_ptr();
            let vtbl = *(raw as *mut *mut AsyncOperationVtbl<u32>);
            let status = ((*vtbl).get_status)(raw as *mut c_void, core::ptr::null_mut());
            assert_eq!(status, STATUS_UNSUCCESSFUL);
            let status = ((*vtbl).get_result)(raw as *mut c_void, core::ptr::null_mut());
            assert_eq!(status, STATUS_UNSUCCESSFUL);
        }
    }
}
