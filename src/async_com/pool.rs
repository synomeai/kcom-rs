// async_com/pool.rs
//
// Lookaside-backed allocator for async COM objects (driver builds).

#![cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]

use core::alloc::Layout;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use crate::allocator::{Allocator, PoolType, WdkAllocator};
use crate::iunknown::{NTSTATUS, STATUS_SUCCESS};
use crate::wrapper::ComObject;

use super::{AsyncOperationTask, AsyncOperationVtbl, AsyncValueType};

use wdk_sys::{LOOKASIDE_LIST_EX, PVOID, _POOL_TYPE};
use wdk_sys::ntddk::{
    ExAllocateFromLookasideListEx,
    ExDeleteLookasideListEx,
    ExFreeToLookasideListEx,
    ExInitializeLookasideListEx,
};

const DEFAULT_ASYNC_COM_POOL_DEPTH: u16 = 128;
const DEFAULT_ASYNC_COM_POOL_TAG: u32 = u32::from_ne_bytes(*b"acop");

static ASYNC_COM_POOL_DEPTH: AtomicU32 = AtomicU32::new(DEFAULT_ASYNC_COM_POOL_DEPTH as u32);
static ASYNC_COM_POOL_TAG: AtomicU32 = AtomicU32::new(DEFAULT_ASYNC_COM_POOL_TAG);

/// Override the lookaside depth for async COM allocations.
///
/// Call this before `init_async_com_pool_for::<T>()` to take effect.
#[inline]
pub fn set_async_com_pool_depth(depth: u16) {
    ASYNC_COM_POOL_DEPTH.store(depth as u32, Ordering::Release);
}

/// Override the pool tag used by async COM lookaside allocations.
///
/// Call this before `init_async_com_pool_for::<T>()` to take effect.
#[inline]
pub fn set_async_com_pool_tag(tag: u32) {
    ASYNC_COM_POOL_TAG.store(tag, Ordering::Release);
}

#[inline]
fn async_com_pool_depth() -> u16 {
    ASYNC_COM_POOL_DEPTH.load(Ordering::Acquire) as u16
}

#[inline]
fn async_com_pool_tag() -> u32 {
    ASYNC_COM_POOL_TAG.load(Ordering::Acquire)
}

fn pool_state<T: AsyncValueType>() -> &'static AtomicU32 {
    static STATE: AtomicU32 = AtomicU32::new(0);
    &STATE
}

fn pool_status<T: AsyncValueType>() -> &'static AtomicI32 {
    static STATUS: AtomicI32 = AtomicI32::new(STATUS_SUCCESS);
    &STATUS
}

unsafe fn pool_storage<T: AsyncValueType>() -> *mut LOOKASIDE_LIST_EX {
    static mut LIST: MaybeUninit<LOOKASIDE_LIST_EX> = MaybeUninit::uninit();
    core::ptr::addr_of_mut!(LIST) as *mut LOOKASIDE_LIST_EX
}

#[derive(Copy, Clone, Default)]
pub struct AsyncComAlloc<T: AsyncValueType> {
    _marker: PhantomData<T>,
}

impl<T: AsyncValueType> AsyncComAlloc<T> {
    #[inline]
    pub fn new() -> Self {
        Self { _marker: PhantomData }
    }
}

unsafe impl<T: AsyncValueType> Send for AsyncComAlloc<T> {}
unsafe impl<T: AsyncValueType> Sync for AsyncComAlloc<T> {}

struct AsyncComPool<T: AsyncValueType>(PhantomData<T>);

impl<T: AsyncValueType> AsyncComPool<T> {
    const STATE_UNINIT: u32 = 0;
    const STATE_INITING: u32 = 1;
    const STATE_READY: u32 = 2;
    const STATE_FAILED: u32 = 3;

    #[inline]
    fn expected_layout() -> Layout {
        type LayoutTask<T> = AsyncOperationTask<T, core::future::Ready<T>>;
        type LayoutCom<T> = ComObject<LayoutTask<T>, AsyncOperationVtbl<T>, AsyncComAlloc<T>>;
        Layout::new::<LayoutCom<T>>()
    }

    #[inline]
    fn is_ready() -> bool {
        pool_state::<T>().load(Ordering::Acquire) == Self::STATE_READY
    }

    fn init() -> NTSTATUS {
        let state = pool_state::<T>();
        loop {
            match state.load(Ordering::Acquire) {
                Self::STATE_READY => return STATUS_SUCCESS,
                Self::STATE_FAILED => return pool_status::<T>().load(Ordering::Acquire),
                Self::STATE_INITING => {
                    core::hint::spin_loop();
                    continue;
                }
                _ => {
                    if state
                        .compare_exchange(
                            Self::STATE_UNINIT,
                            Self::STATE_INITING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        break;
                    }
                }
            }
        }

        let layout = Self::expected_layout();
        let lookaside = unsafe { pool_storage::<T>() };
        let status = unsafe {
            ExInitializeLookasideListEx(
                lookaside,
                None,
                None,
                _POOL_TYPE::NonPagedPoolNx,
                0,
                layout.size() as u64,
                async_com_pool_tag(),
                async_com_pool_depth(),
            )
        };

        pool_status::<T>().store(status, Ordering::Release);
        if status >= 0 {
            state.store(Self::STATE_READY, Ordering::Release);
        } else {
            state.store(Self::STATE_FAILED, Ordering::Release);
        }
        status
    }

    #[inline]
    unsafe fn alloc(layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return NonNull::<u8>::dangling().as_ptr();
        }

        let expected = Self::expected_layout();
        if layout.size() != expected.size() || layout.align() != expected.align() {
            return WdkAllocator::new(PoolType::NonPagedNx, async_com_pool_tag()).alloc(layout);
        }

        let status = Self::init();
        if status < 0 {
            return WdkAllocator::new(PoolType::NonPagedNx, async_com_pool_tag()).alloc(layout);
        }

        let lookaside = unsafe { pool_storage::<T>() };
        let ptr = unsafe { ExAllocateFromLookasideListEx(lookaside) };
        ptr as *mut u8
    }

    #[inline]
    unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }

        let expected = Self::expected_layout();
        if layout.size() != expected.size()
            || layout.align() != expected.align()
            || !Self::is_ready()
        {
            WdkAllocator::new(PoolType::NonPagedNx, async_com_pool_tag()).dealloc(ptr, layout);
            return;
        }

        let lookaside = unsafe { pool_storage::<T>() };
        unsafe { ExFreeToLookasideListEx(lookaside, ptr as PVOID) };
    }
}

impl<T: AsyncValueType> Allocator for AsyncComAlloc<T> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { AsyncComPool::<T>::alloc(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { AsyncComPool::<T>::dealloc(ptr, layout) }
    }
}

/// Initialize the async COM lookaside pool for the given output type.
///
/// Call this at PASSIVE_LEVEL during driver initialization.
#[inline]
pub fn init_async_com_pool_for<T: AsyncValueType>() -> NTSTATUS {
    AsyncComPool::<T>::init()
}

/// Tear down the async COM lookaside pool for the given output type.
///
/// # Safety
/// Call this only after all async COM objects of this type are dropped.
#[inline]
pub unsafe fn shutdown_async_com_pool_for<T: AsyncValueType>() {
    let state = pool_state::<T>();
    if state
        .compare_exchange(
            AsyncComPool::<T>::STATE_READY,
            AsyncComPool::<T>::STATE_UNINIT,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        let lookaside = unsafe { pool_storage::<T>() };
        unsafe { ExDeleteLookasideListEx(lookaside) };
        pool_status::<T>().store(STATUS_SUCCESS, Ordering::Release);
    }
}
