// irp.rs
//
// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::future::Future;
use core::pin::Pin;
use core::ptr::{NonNull, null_mut};
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};

use crate::async_com::{self, AsyncOperationRaw};
use crate::allocator::{Allocator, KBox, PoolType, WdkAllocator};
use crate::iunknown::{
    NTSTATUS, STATUS_CANCELLED, STATUS_INSUFFICIENT_RESOURCES, STATUS_INVALID_PARAMETER,
    STATUS_PENDING,
};
#[cfg(any(not(driver_model__driver_type = "WDM"), miri))]
use crate::iunknown::STATUS_NOT_SUPPORTED;
use crate::refcount;
use crate::sync_state::CancelHandshake;
use crate::smart_ptr::ComRc;
use crate::ntddk::{KeAcquireSpinLockRaiseToDpc, KeInitializeSpinLock, KeReleaseSpinLock, KIRQL, KSPIN_LOCK};

use wdk_sys::{DEVICE_OBJECT, IO_STATUS_BLOCK, IRP, LARGE_INTEGER};
use wdk_sys::ntddk::{
    IoBuildAsynchronousFsdRequest, IoCancelIrp, IoFreeIrp, IofCallDriver, IoSetCompletionRoutineEx,
};

#[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
use crate::ntddk::{IoAllocateWorkItem, IoFreeWorkItem, IoQueueWorkItem, PIO_WORKITEM, PIO_WORKITEM_ROUTINE, WORK_QUEUE_TYPE};

const IRP_FUTURE_TAG: u32 = u32::from_ne_bytes(*b"irpf");
const STATUS_MORE_PROCESSING_REQUIRED: NTSTATUS = 0xC000_0016u32 as i32;

const STATE_INIT: u32 = 0;
const STATE_DISPATCHING: u32 = 1;
const STATE_DISPATCHED: u32 = 2;
/// `finish` has claimed completion but not yet published status/information.
const STATE_COMPLETING: u32 = 4;
/// Status and information are published; set only after they are stored.
const STATE_DONE: u32 = 3;

/// Result for an IRP-backed future.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IrpResult {
    pub status: NTSTATUS,
    pub information: usize,
}

/// How the IRP completion routine should wake the task.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IrpWakeMode {
    /// Call the waker directly from the completion routine.
    Direct,
    /// Queue a work item to wake at PASSIVE_LEVEL (WDM only).
    WorkItem,
}

/// Ownership of the IRP itself.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IrpOwnership {
    /// The future frees the IRP during completion.
    Future,
    /// The caller owns the IRP and must free it.
    Caller,
}

pub type IrpCompleteFn = unsafe fn(*mut c_void, *mut IRP);
pub type IrpCleanupFn = unsafe fn(*mut c_void);

/// Options for constructing an IRP future.
#[derive(Copy, Clone)]
pub struct IrpFutureOptions {
    pub context: *mut c_void,
    pub on_complete: Option<IrpCompleteFn>,
    pub on_cleanup: Option<IrpCleanupFn>,
    pub wake_mode: IrpWakeMode,
    pub ownership: IrpOwnership,
}

impl Default for IrpFutureOptions {
    fn default() -> Self {
        Self {
            context: null_mut(),
            on_complete: None,
            on_cleanup: None,
            wake_mode: IrpWakeMode::Direct,
            ownership: IrpOwnership::Future,
        }
    }
}

/// Future that resolves when an IRP completes.
///
/// # Safety
/// - The IRP must be fully initialized for dispatch and must remain valid until completion.
/// - The target device must remain valid for the lifetime of the IRP.
/// - The completion routine returns `STATUS_MORE_PROCESSING_REQUIRED`; this future must be the
///   final completion handler for the IRP.
/// - `poll` must be called at PASSIVE_LEVEL.
/// - The completion routine may run at DISPATCH_LEVEL; choose `IrpWakeMode::WorkItem`
///   to ensure the wake happens at PASSIVE_LEVEL (WDM only).
pub struct IrpFuture {
    inner: NonNull<IrpFutureInner>,
}

unsafe impl Send for IrpFuture {}

impl IrpFuture {
    /// Create a new IRP future.
    ///
    /// The IRP is dispatched on the first `poll` call.
    ///
    /// # Safety
    /// See type-level safety notes.
    pub unsafe fn new(
        target_device: *mut DEVICE_OBJECT,
        irp: *mut IRP,
        options: IrpFutureOptions,
    ) -> Result<Self, NTSTATUS> {
        if target_device.is_null() || irp.is_null() {
            return Err(STATUS_INVALID_PARAMETER);
        }

        #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
        let work_item = if options.wake_mode == IrpWakeMode::WorkItem {
            let item = unsafe { IoAllocateWorkItem(target_device) };
            if item.is_null() {
                return Err(STATUS_INSUFFICIENT_RESOURCES);
            }
            item
        } else {
            null_mut()
        };

        #[cfg(any(not(driver_model__driver_type = "WDM"), miri))]
        if options.wake_mode == IrpWakeMode::WorkItem {
            return Err(STATUS_NOT_SUPPORTED);
        }

        let alloc = WdkAllocator::new(PoolType::NonPagedNx, IRP_FUTURE_TAG);
        let layout = Layout::new::<IrpFutureInner>();
        let ptr = unsafe { alloc.alloc(layout) } as *mut IrpFutureInner;
        if ptr.is_null() {
            #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
            {
                if !work_item.is_null() {
                    unsafe { IoFreeWorkItem(work_item) };
                }
            }
            return Err(STATUS_INSUFFICIENT_RESOURCES);
        }

        unsafe {
            core::ptr::write(
                ptr,
                IrpFutureInner {
                    ref_count: AtomicU32::new(1),
                    state: AtomicU32::new(STATE_INIT),
                    status: AtomicI32::new(STATUS_PENDING),
                    information: AtomicUsize::new(0),
                    waker: SpinLock::new(None),
                    irp: AtomicPtr::new(irp),
                    cancel_target: irp,
                    cancel_handshake: CancelHandshake::new(),
                    target_device,
                    on_complete: options.on_complete,
                    on_cleanup: options.on_cleanup,
                    context: options.context,
                    wake_mode: options.wake_mode,
                    ownership: options.ownership,
                    #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
                    work_item: AtomicPtr::new(work_item),
                },
            );
        }

        Ok(Self {
            inner: unsafe { NonNull::new_unchecked(ptr) },
        })
    }

    #[inline]
    fn result(inner: &IrpFutureInner) -> IrpResult {
        IrpResult {
            status: inner.status.load(Ordering::Acquire),
            information: inner.information.load(Ordering::Acquire),
        }
    }
}

/// Helper for building an asynchronous FSD request IRP.
///
/// # Safety
/// The caller must ensure the buffer and `io_status` remain valid until the IRP completes.
pub unsafe fn build_async_fsd_request(
    major_function: u32,
    target_device: *mut DEVICE_OBJECT,
    buffer: *mut c_void,
    length: u32,
    starting_offset: *mut LARGE_INTEGER,
    io_status: *mut IO_STATUS_BLOCK,
) -> Result<*mut IRP, NTSTATUS> {
    if target_device.is_null() || io_status.is_null() {
        return Err(STATUS_INVALID_PARAMETER);
    }

    let irp = unsafe {
        IoBuildAsynchronousFsdRequest(
            major_function,
            target_device,
            buffer,
            length,
            starting_offset,
            io_status,
        )
    };
    if irp.is_null() {
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }
    Ok(irp)
}

/// Spawn an AsyncOperation backed by an IRP.
///
/// # Safety
/// See `IrpFuture::new` safety notes.
pub unsafe fn spawn_irp_operation_raw(
    target_device: *mut DEVICE_OBJECT,
    irp: *mut IRP,
    options: IrpFutureOptions,
) -> Result<*mut AsyncOperationRaw<IrpResult>, NTSTATUS> {
    let future = unsafe { IrpFuture::new(target_device, irp, options) }?;
    async_com::spawn_async_operation_raw(future)
}

/// Spawn a COM wrapper for an IRP-backed async operation.
///
/// # Safety
/// See `IrpFuture::new` safety notes.
pub unsafe fn spawn_irp_operation(
    target_device: *mut DEVICE_OBJECT,
    irp: *mut IRP,
    options: IrpFutureOptions,
) -> Result<ComRc<AsyncOperationRaw<IrpResult>>, NTSTATUS> {
    let ptr = unsafe { spawn_irp_operation_raw(target_device, irp, options) }?;
    Ok(unsafe { ComRc::from_raw_unchecked(ptr) })
}

impl Future for IrpFuture {
    type Output = IrpResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = unsafe { &*self.inner.as_ptr() };

        if inner.state.load(Ordering::Acquire) == STATE_DONE {
            return Poll::Ready(Self::result(inner));
        }

        {
            let mut guard = inner.waker.lock();
            *guard = Some(cx.waker().clone());
        }

        if inner
            .state
            .compare_exchange(STATE_INIT, STATE_DISPATCHING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            unsafe { IrpFutureInner::submit(self.inner) };
        }

        if inner.state.load(Ordering::Acquire) == STATE_DONE {
            Poll::Ready(Self::result(inner))
        } else {
            Poll::Pending
        }
    }
}

impl Drop for IrpFuture {
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner.as_ptr() };
        let state = inner.state.load(Ordering::Acquire);

        if state == STATE_INIT {
            let irp = inner.take_irp();
            unsafe { IrpFutureInner::finish(self.inner, irp, STATUS_CANCELLED, 0) };
        } else if state != STATE_DONE {
            // The completion routine may be freeing the IRP concurrently (and
            // IoCancelIrp can complete it synchronously on this thread). The
            // handshake defers that free until IoCancelIrp has returned.
            if inner.cancel_handshake.begin_cancel() {
                unsafe { IoCancelIrp(inner.cancel_target) };
                if let Some(irp) = inner.cancel_handshake.end_cancel() {
                    unsafe { IrpFutureInner::dispose_irp(inner, irp) };
                }
            }
        }

        unsafe { IrpFutureInner::release(self.inner) };
    }
}

struct IrpFutureInner {
    ref_count: AtomicU32,
    state: AtomicU32,
    status: AtomicI32,
    information: AtomicUsize,
    waker: SpinLock<Option<Waker>>,
    irp: AtomicPtr<IRP>,
    /// The IRP passed to `IoCancelIrp`. Only valid while `cancel_handshake`
    /// says it has not been disposed of.
    cancel_target: *mut IRP,
    cancel_handshake: CancelHandshake<IRP>,
    target_device: *mut DEVICE_OBJECT,
    on_complete: Option<IrpCompleteFn>,
    on_cleanup: Option<IrpCleanupFn>,
    context: *mut c_void,
    wake_mode: IrpWakeMode,
    ownership: IrpOwnership,
    #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
    work_item: AtomicPtr<PIO_WORKITEM>,
}

impl IrpFutureInner {
    #[inline]
    unsafe fn add_ref(ptr: NonNull<Self>) {
        let inner = unsafe { &*ptr.as_ptr() };
        let _ = refcount::add(&inner.ref_count);
    }

    unsafe fn release(ptr: NonNull<Self>) {
        let inner = unsafe { &*ptr.as_ptr() };
        let count = refcount::sub(&inner.ref_count);
        if count != 0 {
            return;
        }

        core::sync::atomic::fence(Ordering::Acquire);
        unsafe { Self::free(ptr) }
    }

    unsafe fn free(ptr: NonNull<Self>) {
        #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
        {
            let work_item = unsafe { &*ptr.as_ptr() }
                .work_item
                .swap(null_mut(), Ordering::AcqRel);
            if !work_item.is_null() {
                unsafe { IoFreeWorkItem(work_item) };
            }
        }

        let alloc = WdkAllocator::new(PoolType::NonPagedNx, IRP_FUTURE_TAG);
        let layout = Layout::new::<IrpFutureInner>();
        unsafe { drop(KBox::from_raw_parts(ptr, alloc, layout)) }
    }

    #[inline]
    fn take_irp(&self) -> *mut IRP {
        self.irp.swap(null_mut(), Ordering::AcqRel)
    }

    #[inline]
    unsafe fn submit(ptr: NonNull<Self>) {
        let inner = unsafe { &*ptr.as_ptr() };
        let irp = inner.irp.load(Ordering::Acquire);
        if irp.is_null() {
            unsafe { Self::finish(ptr, null_mut(), STATUS_INVALID_PARAMETER, 0) };
            return;
        }

        unsafe { Self::add_ref(ptr) };
        let status = unsafe {
            IoSetCompletionRoutineEx(
                inner.target_device,
                irp,
                Some(Self::completion_routine),
                ptr.as_ptr() as *mut c_void,
                1,
                1,
                1,
            )
        };
        if status < 0 {
            unsafe { Self::release(ptr) };
            unsafe { Self::finish(ptr, inner.take_irp(), status, 0) };
            return;
        }

        unsafe {
            let _ = IofCallDriver(inner.target_device, irp);
        }

        let _ = inner.state.compare_exchange(
            STATE_DISPATCHING,
            STATE_DISPATCHED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    unsafe fn finish(ptr: NonNull<Self>, irp: *mut IRP, status: NTSTATUS, information: usize) {
        let inner = unsafe { &*ptr.as_ptr() };
        // Claim completion first, publish the result, and only then mark the
        // future DONE: `poll` reads status/information as soon as it sees DONE.
        let mut state = inner.state.load(Ordering::Acquire);
        loop {
            if state == STATE_DONE || state == STATE_COMPLETING {
                return;
            }
            match inner.state.compare_exchange_weak(
                state,
                STATE_COMPLETING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }

        inner.status.store(status, Ordering::Relaxed);
        inner.information.store(information, Ordering::Relaxed);
        inner.state.store(STATE_DONE, Ordering::Release);

        unsafe { Self::wake(ptr) };

        if !irp.is_null() {
            if let Some(irp) = inner.cancel_handshake.complete(irp) {
                unsafe { Self::dispose_irp(inner, irp) };
            }
        }
    }

    /// Runs the cleanup callback and frees the IRP if the future owns it.
    unsafe fn dispose_irp(inner: &Self, irp: *mut IRP) {
        if irp.is_null() {
            return;
        }
        if let Some(cb) = inner.on_cleanup {
            unsafe { cb(inner.context) };
        }
        if inner.ownership == IrpOwnership::Future {
            unsafe { IoFreeIrp(irp) };
        }
    }

    #[inline]
    unsafe fn wake(ptr: NonNull<Self>) {
        let inner = unsafe { &*ptr.as_ptr() };
        match inner.wake_mode {
            IrpWakeMode::Direct => unsafe { Self::wake_direct(ptr) },
            IrpWakeMode::WorkItem => unsafe { Self::queue_work_item(ptr) },
        }
    }

    unsafe fn wake_direct(ptr: NonNull<Self>) {
        let inner = unsafe { &*ptr.as_ptr() };
        let waker = {
            let mut guard = inner.waker.lock();
            guard.take()
        };
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
    }

    #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
    unsafe fn queue_work_item(ptr: NonNull<Self>) {
        let inner = unsafe { &*ptr.as_ptr() };
        let work_item = inner.work_item.load(Ordering::Acquire);
        if work_item.is_null() {
            unsafe { Self::wake_direct(ptr) };
            return;
        }

        unsafe { Self::add_ref(ptr) };
        unsafe {
            IoQueueWorkItem(
                work_item,
                Some(Self::work_item_routine as PIO_WORKITEM_ROUTINE),
                WORK_QUEUE_TYPE::DelayedWorkQueue,
                ptr.as_ptr() as *mut c_void,
            );
        }
    }

    #[cfg(any(not(driver_model__driver_type = "WDM"), miri))]
    unsafe fn queue_work_item(ptr: NonNull<Self>) {
        unsafe { Self::wake_direct(ptr) };
    }

    #[cfg(all(driver_model__driver_type = "WDM", not(miri)))]
    unsafe extern "C" fn work_item_routine(_device: *mut DEVICE_OBJECT, context: *mut c_void) {
        let ptr = match NonNull::new(context as *mut IrpFutureInner) {
            Some(p) => p,
            None => return,
        };

        unsafe { Self::wake_direct(ptr) };

        let work_item = unsafe { &*ptr.as_ptr() }
            .work_item
            .swap(null_mut(), Ordering::AcqRel);
        if !work_item.is_null() {
            unsafe { IoFreeWorkItem(work_item) };
        }

        unsafe { Self::release(ptr) };
    }

    unsafe extern "C" fn completion_routine(
        _device: *mut DEVICE_OBJECT,
        irp: *mut IRP,
        context: *mut c_void,
    ) -> NTSTATUS {
        let ptr = match NonNull::new(context as *mut IrpFutureInner) {
            Some(p) => p,
            None => return STATUS_MORE_PROCESSING_REQUIRED,
        };

        let inner = unsafe { &*ptr.as_ptr() };

        if let Some(cb) = inner.on_complete {
            unsafe { cb(inner.context, irp) };
        }

        let status = unsafe { (*irp).IoStatus.__bindgen_anon_1.Status };
        let information = unsafe { (*irp).IoStatus.Information as usize };

        let irp = inner.take_irp();
        unsafe { Self::finish(ptr, irp, status, information) };

        unsafe { Self::release(ptr) };
        STATUS_MORE_PROCESSING_REQUIRED
    }
}

struct SpinLock<T> {
    lock: UnsafeCell<KSPIN_LOCK>,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    #[inline]
    fn new(value: T) -> Self {
        let mut lock = unsafe { core::mem::zeroed() };
        unsafe { KeInitializeSpinLock(&mut lock) };
        Self {
            lock: UnsafeCell::new(lock),
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    fn lock(&self) -> SpinLockGuard<'_, T> {
        let old_irql = unsafe { KeAcquireSpinLockRaiseToDpc(self.lock.get()) };
        SpinLockGuard {
            lock: self,
            old_irql,
        }
    }
}

struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    old_irql: KIRQL,
}

impl<T> core::ops::Deref for SpinLockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> core::ops::DerefMut for SpinLockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { KeReleaseSpinLock(self.lock.lock.get(), self.old_irql) };
    }
}
