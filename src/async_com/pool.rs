// async_com/pool.rs
//
// Lookaside-backed allocator for async COM objects (driver builds).

#![cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]

use core::alloc::Layout;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use crate::allocator::{Allocator, PoolType, WdkAllocator};
use crate::iunknown::{NTSTATUS, STATUS_INSUFFICIENT_RESOURCES, STATUS_SUCCESS};
use crate::sync_state::LayoutSlots;
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

/// Number of distinct object layouts that get a lookaside list; further
/// layouts fall back to the pool allocator.
const POOL_SLOTS: usize = 16;

// A `static` inside a generic function is a single instance shared by every
// monomorphization, so per-type storage would hand one list (sized for the
// first `T`) to every `T`. Lists are keyed by the object layout instead.
static POOL_KEYS: LayoutSlots<POOL_SLOTS> = LayoutSlots::new();
static POOL_STATE: [AtomicU32; POOL_SLOTS] = [const { AtomicU32::new(0) }; POOL_SLOTS];
static POOL_STATUS: [AtomicI32; POOL_SLOTS] = [const { AtomicI32::new(STATUS_SUCCESS) }; POOL_SLOTS];
static mut POOL_LISTS: [MaybeUninit<LOOKASIDE_LIST_EX>; POOL_SLOTS] =
    [const { MaybeUninit::uninit() }; POOL_SLOTS];

#[inline]
unsafe fn pool_storage(slot: usize) -> *mut LOOKASIDE_LIST_EX {
    let lists = core::ptr::addr_of_mut!(POOL_LISTS) as *mut MaybeUninit<LOOKASIDE_LIST_EX>;
    unsafe { lists.add(slot) as *mut LOOKASIDE_LIST_EX }
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
    fn is_ready(slot: usize) -> bool {
        POOL_STATE[slot].load(Ordering::Acquire) == Self::STATE_READY
    }

    fn init() -> NTSTATUS {
        let layout = Self::expected_layout();
        let Some(slot) = POOL_KEYS.find_or_claim(layout) else {
            // Out of slots: allocations use the fallback allocator.
            return STATUS_INSUFFICIENT_RESOURCES;
        };
        let state = &POOL_STATE[slot];
        loop {
            match state.load(Ordering::Acquire) {
                Self::STATE_READY => return STATUS_SUCCESS,
                Self::STATE_FAILED => return POOL_STATUS[slot].load(Ordering::Acquire),
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

        let lookaside = unsafe { pool_storage(slot) };
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

        POOL_STATUS[slot].store(status, Ordering::Release);
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
            // Aligned dangling pointer (NonNull::dangling() is only 1-aligned).
            return core::ptr::null_mut::<u8>().wrapping_add(layout.align());
        }

        let fallback = WdkAllocator::new(PoolType::NonPagedNx, async_com_pool_tag());
        if layout != Self::expected_layout() {
            return unsafe { fallback.alloc(layout) };
        }

        if Self::init() < 0 {
            return unsafe { fallback.alloc(layout) };
        }
        let Some(slot) = POOL_KEYS.find(layout) else {
            return unsafe { fallback.alloc(layout) };
        };

        let lookaside = unsafe { pool_storage(slot) };
        let ptr = unsafe { ExAllocateFromLookasideListEx(lookaside) };
        ptr as *mut u8
    }

    #[inline]
    unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }

        let fallback = WdkAllocator::new(PoolType::NonPagedNx, async_com_pool_tag());
        let slot = if layout == Self::expected_layout() {
            POOL_KEYS.find(layout).filter(|&slot| Self::is_ready(slot))
        } else {
            None
        };
        let Some(slot) = slot else {
            unsafe { fallback.dealloc(ptr, layout) };
            return;
        };

        let lookaside = unsafe { pool_storage(slot) };
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
/// Pools are keyed by object layout, so output types whose objects have the
/// same layout share one lookaside list.
///
/// Call this at PASSIVE_LEVEL during driver initialization.
#[inline]
pub fn init_async_com_pool_for<T: AsyncValueType>() -> NTSTATUS {
    AsyncComPool::<T>::init()
}

/// Tear down the async COM lookaside pool for the given output type.
///
/// # Safety
/// Call this only after all async COM objects that use this pool are dropped,
/// including those of other output types with the same object layout.
#[inline]
pub unsafe fn shutdown_async_com_pool_for<T: AsyncValueType>() {
    let Some(slot) = POOL_KEYS.find(AsyncComPool::<T>::expected_layout()) else {
        return;
    };
    if POOL_STATE[slot]
        .compare_exchange(
            AsyncComPool::<T>::STATE_READY,
            AsyncComPool::<T>::STATE_UNINIT,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        let lookaside = unsafe { pool_storage(slot) };
        unsafe { ExDeleteLookasideListEx(lookaside) };
        POOL_STATUS[slot].store(STATUS_SUCCESS, Ordering::Release);
    }
}
