// executor.rs

// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::future::Future;
#[cfg(any(not(feature = "driver"), feature = "async-com-kernel", miri))]
use core::pin::Pin;
#[cfg(any(not(feature = "driver"), feature = "async-com-kernel", miri))]
use core::task::{Context, Poll};
#[cfg(any(not(feature = "driver"), miri))]
use core::cell::{Cell, RefCell};
#[cfg(any(not(feature = "driver"), miri))]
use crate::alloc::boxed::Box;

use crate::iunknown::{NTSTATUS, STATUS_NOT_SUPPORTED};
#[cfg(all(
    feature = "driver",
    feature = "async-com-kernel",
    driver_model__driver_type = "WDM",
    not(miri)
))]
use crate::iunknown::STATUS_INVALID_PARAMETER;
#[cfg(any(not(feature = "driver"), feature = "async-com-kernel", miri))]
use crate::iunknown::STATUS_SUCCESS;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use core::task::{RawWaker, RawWakerVTable, Waker};
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use core::cell::UnsafeCell;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use core::ffi::c_void;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use core::mem::ManuallyDrop;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use core::ptr::{NonNull, null_mut};
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
use crate::ntddk::{
    DEVICE_OBJECT, IoAllocateWorkItem, IoFreeWorkItem, IoQueueWorkItem, ObDereferenceObject,
    ObReferenceObject, PIO_WORKITEM, PIO_WORKITEM_ROUTINE, WORK_QUEUE_TYPE,
};

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM")), miri))]
#[allow(non_camel_case_types)]
type DEVICE_OBJECT = core::ffi::c_void;

#[cfg(any(not(feature = "driver"), miri))]
fn dummy_waker() -> core::task::Waker {
    use core::task::{RawWaker, RawWakerVTable, Waker};

    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use crate::iunknown::STATUS_INSUFFICIENT_RESOURCES;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use crate::allocator::{Allocator, KBox, PoolType, WdkAllocator};
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use crate::refcount;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use crate::async_com_metrics as metrics;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
use crate::ntddk::{
    KeAcquireSpinLockRaiseToDpc, KeCancelTimer, KeInitializeDpc, KeInitializeSpinLock,
    KeInitializeTimer, KeInsertQueueDpc, KeQueryPerformanceCounter, KeReleaseSpinLock,
    KeRemoveQueueDpc, KeSetTimer, KDPC, KIRQL, KSPIN_LOCK, LARGE_INTEGER, PKDPC, KTIMER, PKTIMER,
};

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
type TaskPollFn = for<'a> unsafe fn(*mut TaskHeader, &mut Context<'a>) -> Poll<NTSTATUS>;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[derive(Copy, Clone)]
enum DestroyMode {
    Drop,
    Dealloc,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
struct TaskVTable {
    poll: TaskPollFn,
    destroy: unsafe fn(*mut TaskHeader, DestroyMode),
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[repr(C)]
struct TaskHeader {
    ref_count: AtomicU32,
    scheduled: AtomicU32,
    running: AtomicU32,
    completed: AtomicU32,
    cancel_requested: AtomicU32,
    dpc: KDPC,
    vtable: &'static TaskVTable,
    alloc_tag: u32,
    tracker: *const TaskTracker,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const DEFAULT_TASK_BUDGET_POLLS: u32 = 64;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const DEFAULT_TASK_BUDGET_TIME_US: u64 = 200;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const DEFAULT_TASK_BUDGET_ADAPTIVE_MIN: u32 = 16;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const DEFAULT_TASK_BUDGET_ADAPTIVE_MAX: u32 = 128;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const DEFAULT_TASK_BUDGET_ADAPTIVE_LOW_PCT: u32 = 5;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const DEFAULT_TASK_BUDGET_ADAPTIVE_HIGH_PCT: u32 = 50;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const TASK_BUDGET_MODE_POLLS: u32 = 0;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const TASK_BUDGET_MODE_TIME_US: u32 = 1;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const TASK_BUDGET_MODE_ADAPTIVE: u32 = 2;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const TASK_BUDGET_TIME_CHECK_INTERVAL: u32 = 8;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_MODE: AtomicU32 = AtomicU32::new(TASK_BUDGET_MODE_POLLS);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_POLLS: AtomicU32 = AtomicU32::new(DEFAULT_TASK_BUDGET_POLLS);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_TIME_US: AtomicU64 = AtomicU64::new(DEFAULT_TASK_BUDGET_TIME_US);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_ADAPTIVE_MIN: AtomicU32 = AtomicU32::new(DEFAULT_TASK_BUDGET_ADAPTIVE_MIN);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_ADAPTIVE_MAX: AtomicU32 = AtomicU32::new(DEFAULT_TASK_BUDGET_ADAPTIVE_MAX);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_ADAPTIVE_LOW_PCT: AtomicU32 = AtomicU32::new(DEFAULT_TASK_BUDGET_ADAPTIVE_LOW_PCT);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_ADAPTIVE_HIGH_PCT: AtomicU32 = AtomicU32::new(DEFAULT_TASK_BUDGET_ADAPTIVE_HIGH_PCT);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_ADAPTIVE_LAST_SKIPPED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static TASK_BUDGET_ADAPTIVE_LAST_ENQUEUED: AtomicU64 = AtomicU64::new(0);

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[derive(Copy, Clone, Debug)]
pub enum TaskBudget {
    Polls(u32),
    TimeUs(u64),
    Adaptive { min_polls: u32, max_polls: u32 },
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
/// Configure the DPC executor budget.
///
/// - `Polls(n)` matches the original behavior (poll at most n times per DPC run).
/// - `TimeUs(us)` limits execution time using `KeQueryPerformanceCounter`.
/// - `Adaptive { min_polls, max_polls }` varies the poll budget based on DPC pressure.
#[inline]
pub fn set_task_budget(budget: TaskBudget) {
    match budget {
        TaskBudget::Polls(polls) => {
            TASK_BUDGET_POLLS.store(polls, Ordering::Release);
            TASK_BUDGET_MODE.store(TASK_BUDGET_MODE_POLLS, Ordering::Release);
        }
        TaskBudget::TimeUs(us) => {
            TASK_BUDGET_TIME_US.store(us, Ordering::Release);
            TASK_BUDGET_MODE.store(TASK_BUDGET_MODE_TIME_US, Ordering::Release);
        }
        TaskBudget::Adaptive { min_polls, max_polls } => {
            let min = min_polls.min(max_polls);
            let max = max_polls.max(min_polls);
            TASK_BUDGET_ADAPTIVE_MIN.store(min, Ordering::Release);
            TASK_BUDGET_ADAPTIVE_MAX.store(max, Ordering::Release);
            TASK_BUDGET_MODE.store(TASK_BUDGET_MODE_ADAPTIVE, Ordering::Release);
        }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
/// Adjust the adaptive budget thresholds (percent skipped DPCs).
///
/// `low_pct` picks the max budget, `high_pct` picks the min budget.
#[inline]
pub fn set_task_budget_adaptive_thresholds(low_pct: u32, high_pct: u32) {
    let low = low_pct.min(100);
    let high = high_pct.max(low).min(100);
    TASK_BUDGET_ADAPTIVE_LOW_PCT.store(low, Ordering::Release);
    TASK_BUDGET_ADAPTIVE_HIGH_PCT.store(high, Ordering::Release);
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
fn adaptive_poll_budget() -> u32 {
    let min = TASK_BUDGET_ADAPTIVE_MIN.load(Ordering::Acquire);
    let max = TASK_BUDGET_ADAPTIVE_MAX.load(Ordering::Acquire).max(min);
    if max == min {
        return min;
    }

    let low = TASK_BUDGET_ADAPTIVE_LOW_PCT.load(Ordering::Acquire).min(100);
    let high = TASK_BUDGET_ADAPTIVE_HIGH_PCT
        .load(Ordering::Acquire)
        .max(low)
        .min(100);

    let metrics = metrics::snapshot_async_com_metrics();
    let skipped = metrics.dpc_skipped;
    let enqueued = metrics.dpc_enqueued;
    let prev_skipped = TASK_BUDGET_ADAPTIVE_LAST_SKIPPED.swap(skipped, Ordering::AcqRel);
    let prev_enqueued = TASK_BUDGET_ADAPTIVE_LAST_ENQUEUED.swap(enqueued, Ordering::AcqRel);
    let delta_skipped = skipped.saturating_sub(prev_skipped);
    let delta_enqueued = enqueued.saturating_sub(prev_enqueued);
    let ratio = if delta_enqueued == 0 {
        0
    } else {
        let percent = delta_skipped.saturating_mul(100) / delta_enqueued;
        percent.min(u32::MAX as u64) as u32
    };

    if ratio <= low {
        return max;
    }
    if ratio >= high {
        return min;
    }

    let span = high - low;
    if span == 0 {
        return min;
    }
    let pos = ratio - low;
    let range = max - min;
    let scaled = range - (range.saturating_mul(pos) / span);
    min + scaled
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static DEFAULT_TASK_TAG: AtomicU32 = AtomicU32::new(u32::from_ne_bytes(*b"kcom"));

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
/// Override the default pool tag used by DPC task allocations.
///
/// Call during driver initialization before spawning tasks.
#[inline]
pub fn set_task_alloc_tag(tag: u32) {
    DEFAULT_TASK_TAG.store(tag, Ordering::Release);
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
// NOTE: KeGetCurrentProcessorNumberEx returns a group-relative index.
// Windows currently supports up to 64 processors per group and 64 groups.
const MAX_PROC_PER_GROUP: usize = 64;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const MAX_GROUP_COUNT: usize = 64;
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
const MAX_CPU_COUNT: usize = MAX_PROC_PER_GROUP * MAX_GROUP_COUNT;

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
static CURRENT_TASKS: [AtomicPtr<TaskHeader>; MAX_CPU_COUNT] =
    [const { AtomicPtr::new(null_mut()) }; MAX_CPU_COUNT];

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[repr(C)]
#[allow(dead_code, non_camel_case_types, non_snake_case)]
struct PROCESSOR_NUMBER {
    Group: u16,
    Number: u8,
    Reserved: u8,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
extern "system" {
    fn KeGetCurrentProcessorNumberEx(processor: *mut PROCESSOR_NUMBER);
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
fn current_cpu_index() -> Option<usize> {
    let mut processor = PROCESSOR_NUMBER {
        Group: 0,
        Number: 0,
        Reserved: 0,
    };
    unsafe { KeGetCurrentProcessorNumberEx(&mut processor) };
    let group = processor.Group as usize;
    let number = processor.Number as usize;
    if group >= MAX_GROUP_COUNT || number >= MAX_PROC_PER_GROUP {
        #[cfg(debug_assertions)]
        crate::trace::trace(format_args!(
            "kcom warning: processor index out of range (group={}, number={}, max_group={}, max_per_group={})",
            group, number, MAX_GROUP_COUNT, MAX_PROC_PER_GROUP
        ));
        return None;
    }
    Some(group * MAX_PROC_PER_GROUP + number)
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
unsafe fn set_current_task(cpu_index: usize, ptr: NonNull<TaskHeader>) {
    CURRENT_TASKS[cpu_index].store(ptr.as_ptr(), Ordering::Release);
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
unsafe fn clear_current_task(cpu_index: usize) {
    CURRENT_TASKS[cpu_index].store(null_mut(), Ordering::Release);
}

/// Returns true when the currently running DPC task has a cancellation request.
///
/// Only valid inside tasks spawned by the DPC-based executor (spawn_dpc_task*). Work-item
/// tasks may migrate across CPUs, so this helper will not reliably track their state.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub fn is_cancellation_requested() -> bool {
    let irql = unsafe { crate::ntddk::KeGetCurrentIrql() };
    if irql < crate::ntddk::DISPATCH_LEVEL as u8 {
        return false;
    }
    let Some(cpu_index) = current_cpu_index() else {
        return false;
    };
    let ptr = CURRENT_TASKS[cpu_index].load(Ordering::Acquire);
    if ptr.is_null() {
        return false;
    }

    unsafe { (*ptr).cancel_requested.load(Ordering::Relaxed) != 0 }
}

/// Stub for non-kernel builds.
#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
pub fn is_cancellation_requested() -> bool {
    false
}

/// Returns true only once per cancellation request, then marks it as acknowledged.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
pub(crate) fn take_cancellation_request() -> bool {
    let irql = unsafe { crate::ntddk::KeGetCurrentIrql() };
    if irql < crate::ntddk::DISPATCH_LEVEL as u8 {
        return false;
    }
    let Some(cpu_index) = current_cpu_index() else {
        return false;
    };
    let ptr = CURRENT_TASKS[cpu_index].load(Ordering::Acquire);
    if ptr.is_null() {
        return false;
    }

    let header = unsafe { &*ptr };
    header
        .cancel_requested
        .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Stub for non-kernel builds.
#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
#[inline]
pub(crate) fn take_cancellation_request() -> bool {
    false
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl TaskHeader {
    #[inline]
    unsafe fn add_ref(ptr: NonNull<Self>) {
        let header = unsafe { &*ptr.as_ptr() };
        let _ = refcount::add(&header.ref_count);
    }

    unsafe fn release(ptr: NonNull<Self>) {
        let header = unsafe { &*ptr.as_ptr() };
        let count = refcount::sub(&header.ref_count);
        if count != 0 {
            return;
        }

        core::sync::atomic::fence(Ordering::Acquire);
        let header = unsafe { &*ptr.as_ptr() };
        let tracker = header.tracker;
        if header.completed.load(Ordering::Acquire) == 0 {
            header.completed.store(1, Ordering::Release);
            unsafe { (header.vtable.destroy)(ptr.as_ptr(), DestroyMode::Drop) };
            unsafe { (header.vtable.destroy)(ptr.as_ptr(), DestroyMode::Dealloc) };
            unsafe { task_tracker_complete(tracker) };
            return;
        }

        unsafe { (header.vtable.destroy)(ptr.as_ptr(), DestroyMode::Dealloc) };
        unsafe { task_tracker_complete(tracker) };
    }

    #[inline]
    unsafe fn cancel(ptr: NonNull<Self>) {
        let header = unsafe { &*ptr.as_ptr() };
        if header
            .cancel_requested
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::Acquire)
            .is_ok()
        {
            unsafe { Self::schedule(ptr) };
        }
    }

    #[inline]
    unsafe fn schedule(ptr: NonNull<Self>) {
        if unsafe { &*ptr.as_ptr() }
            .completed
            .load(Ordering::Acquire)
            != 0
        {
            return;
        }

        if unsafe { &*ptr.as_ptr() }
            .scheduled
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            metrics::inc_dpc_skipped();
            return;
        }

        if unsafe { &*ptr.as_ptr() }
            .running
            .load(Ordering::Acquire)
            != 0
        {
            metrics::inc_dpc_skipped();
            return;
        }

        unsafe { Self::add_ref(ptr) };
        let inserted = unsafe {
            KeInsertQueueDpc(
                &mut (*ptr.as_ptr()).dpc as PKDPC,
                null_mut(),
                null_mut(),
            )
        };

        if inserted == 0 {
            metrics::inc_dpc_skipped();
            unsafe { Self::release(ptr) };
        } else {
            metrics::inc_dpc_enqueued();
        }
    }

    #[inline]
    unsafe fn queue_dpc(ptr: NonNull<Self>) {
        unsafe { Self::add_ref(ptr) };
        let inserted = unsafe {
            KeInsertQueueDpc(
                &mut (*ptr.as_ptr()).dpc as PKDPC,
                null_mut(),
                null_mut(),
            )
        };

        if inserted == 0 {
            metrics::inc_dpc_skipped();
            unsafe { Self::release(ptr) };
        } else {
            metrics::inc_dpc_enqueued();
        }
    }

    unsafe extern "C" fn dpc_routine(
        _dpc: PKDPC,
        deferred_context: *mut c_void,
        _system_argument1: *mut c_void,
        _system_argument2: *mut c_void,
    ) {
        let ptr = match NonNull::new(deferred_context as *mut TaskHeader) {
            Some(p) => p,
            None => return,
        };

        metrics::inc_dpc_run();

        let cpu_index = current_cpu_index();
        unsafe { &*ptr.as_ptr() }.scheduled.store(0, Ordering::Release);
        unsafe { &*ptr.as_ptr() }.running.store(1, Ordering::Release);

        if unsafe { &*ptr.as_ptr() }
            .completed
            .load(Ordering::Acquire)
            != 0
        {
            unsafe { &*ptr.as_ptr() }.running.store(0, Ordering::Release);
            unsafe { Self::release(ptr) };
            return;
        }

        let waker = unsafe { Waker::from_raw(Self::raw_waker_borrowed(ptr)) };
        let mut cx = Context::from_waker(&waker);

        if let Some(cpu_index) = cpu_index {
            unsafe { set_current_task(cpu_index, ptr) };
        }
        let budget_mode = TASK_BUDGET_MODE.load(Ordering::Acquire);
        let mut poll_budget = if budget_mode == TASK_BUDGET_MODE_ADAPTIVE {
            adaptive_poll_budget()
        } else {
            TASK_BUDGET_POLLS.load(Ordering::Acquire)
        };
        let (time_budget_ticks, time_start_ticks) = if budget_mode == TASK_BUDGET_MODE_TIME_US {
            let mut freq = LARGE_INTEGER { QuadPart: 0 };
            let start = unsafe { KeQueryPerformanceCounter(&mut freq) };
            let freq = if freq.QuadPart <= 0 { 1 } else { freq.QuadPart as u64 };
            let budget_us = TASK_BUDGET_TIME_US.load(Ordering::Acquire);
            let ticks = budget_us.saturating_mul(freq) / 1_000_000;
            (ticks, start.QuadPart as u64)
        } else {
            (0, 0)
        };
        let mut time_check_counter: u32 = 0;

        loop {
            metrics::inc_poll_total();
            let poll = unsafe { ((*ptr.as_ptr()).vtable.poll)(ptr.as_ptr(), &mut cx) };
            match poll {
                Poll::Ready(_status) => {
                    metrics::inc_poll_ready();
                    unsafe { &*ptr.as_ptr() }.completed.store(1, Ordering::Release);
                    unsafe {
                        ((*ptr.as_ptr()).vtable.destroy)(ptr.as_ptr(), DestroyMode::Drop)
                    };
                    if let Some(cpu_index) = cpu_index {
                        unsafe { clear_current_task(cpu_index) };
                    }
                    unsafe { Self::release(ptr) };
                    return;
                }
                Poll::Pending => {
                    metrics::inc_poll_pending();
                    let scheduled = unsafe { &*ptr.as_ptr() }.scheduled.load(Ordering::Acquire);
                    if scheduled == 0 {
                        break;
                    }
                    let woken = unsafe { &*ptr.as_ptr() }
                        .scheduled
                        .swap(0, Ordering::AcqRel)
                        != 0;
                    if !woken {
                        break;
                    }

                    let mut budget_exhausted = false;
                    if budget_mode == TASK_BUDGET_MODE_POLLS
                        || budget_mode == TASK_BUDGET_MODE_ADAPTIVE
                    {
                        if poll_budget == 0 {
                            budget_exhausted = true;
                        } else {
                            poll_budget -= 1;
                        }
                    } else {
                        time_check_counter = time_check_counter.wrapping_add(1);
                        if time_check_counter % TASK_BUDGET_TIME_CHECK_INTERVAL == 0 {
                            let now = unsafe { KeQueryPerformanceCounter(null_mut()) };
                            let elapsed = (now.QuadPart as u64)
                                .wrapping_sub(time_start_ticks);
                            if elapsed >= time_budget_ticks {
                                budget_exhausted = true;
                            }
                        }
                    }

                    if budget_exhausted {
                        unsafe { Self::queue_dpc(ptr) };
                        break;
                    }
                }
            }
        }

        if let Some(cpu_index) = cpu_index {
            unsafe { clear_current_task(cpu_index) };
        }
        let late_wake = unsafe { &*ptr.as_ptr() }
            .scheduled
            .swap(0, Ordering::AcqRel)
            != 0;
        unsafe { &*ptr.as_ptr() }.running.store(0, Ordering::Release);
        if late_wake {
            unsafe { Self::queue_dpc(ptr) };
        } else {
            let late_after = unsafe { &*ptr.as_ptr() }
                .scheduled
                .swap(0, Ordering::AcqRel)
                != 0;
            if late_after {
                unsafe { Self::queue_dpc(ptr) };
            }
        }
        unsafe { Self::release(ptr) };
    }

    #[inline]
    unsafe fn raw_waker_owned(ptr: NonNull<Self>) -> RawWaker {
        unsafe { Self::add_ref(ptr) };
        RawWaker::new(ptr.as_ptr() as *const (), &Self::RAW_WAKER_VTABLE)
    }

    #[inline]
    unsafe fn raw_waker_borrowed(ptr: NonNull<Self>) -> RawWaker {
        RawWaker::new(ptr.as_ptr() as *const (), &Self::RAW_WAKER_BORROWED_VTABLE)
    }

    const RAW_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw_owned,
        Self::wake_raw_owned,
        Self::wake_by_ref_raw_owned,
        Self::drop_raw_owned,
    );

    const RAW_WAKER_BORROWED_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw_owned,
        Self::wake_raw_borrowed,
        Self::wake_by_ref_raw_borrowed,
        Self::drop_raw_borrowed,
    );

    unsafe fn clone_raw_owned(ptr: *const ()) -> RawWaker {
        let ptr = match NonNull::new(ptr as *mut TaskHeader) {
            Some(p) => p,
            None => return RawWaker::new(null_mut(), &Self::RAW_WAKER_VTABLE),
        };
        unsafe { Self::raw_waker_owned(ptr) }
    }

    unsafe fn wake_raw_owned(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut TaskHeader) {
            Some(p) => p,
            None => return,
        };
        unsafe { Self::schedule(ptr) };
        unsafe { Self::release(ptr) };
    }

    unsafe fn wake_by_ref_raw_owned(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut TaskHeader) {
            Some(p) => p,
            None => return,
        };
        unsafe { Self::schedule(ptr) };
    }

    unsafe fn drop_raw_owned(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut TaskHeader) {
            Some(p) => p,
            None => return,
        };
        unsafe { Self::release(ptr) };
    }

    unsafe fn wake_raw_borrowed(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut TaskHeader) {
            Some(p) => p,
            None => return,
        };
        unsafe { Self::schedule(ptr) };
    }

    unsafe fn wake_by_ref_raw_borrowed(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut TaskHeader) {
            Some(p) => p,
            None => return,
        };
        unsafe { Self::schedule(ptr) };
    }

    unsafe fn drop_raw_borrowed(_ptr: *const ()) {}
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[repr(C)]
struct Task<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    header: TaskHeader,
    future: ManuallyDrop<F>,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl<F> Task<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    #[inline]
    fn alloc_tag() -> u32 {
        DEFAULT_TASK_TAG.load(Ordering::Acquire)
    }

    const VTABLE: TaskVTable = TaskVTable {
        poll: Self::poll_shim,
        destroy: Self::destroy_shim,
    };

    unsafe fn allocate(
        future: F,
        tracker: *const TaskTracker,
    ) -> Result<NonNull<TaskHeader>, NTSTATUS> {
        let tag = Self::alloc_tag();
        let alloc = WdkAllocator::new(PoolType::NonPagedNx, tag);
        let layout = core::alloc::Layout::new::<Task<F>>();

        let ptr = unsafe { alloc.alloc(layout) } as *mut Task<F>;
        let ptr = NonNull::new(ptr).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

        unsafe {
            core::ptr::write(
                ptr.as_ptr(),
                Task {
                    header: TaskHeader {
                        ref_count: AtomicU32::new(1),
                        scheduled: AtomicU32::new(0),
                        running: AtomicU32::new(0),
                        completed: AtomicU32::new(0),
                        cancel_requested: AtomicU32::new(0),
                        dpc: core::mem::zeroed(),
                        vtable: &Self::VTABLE,
                        alloc_tag: tag,
                        tracker,
                    },
                    future: ManuallyDrop::new(future),
                },
            );

            KeInitializeDpc(
                &mut (*ptr.as_ptr()).header.dpc as PKDPC,
                Some(TaskHeader::dpc_routine),
                &mut (*ptr.as_ptr()).header as *mut TaskHeader as *mut c_void,
            );
        }

        unsafe { task_tracker_begin(tracker) };

        Ok(unsafe { NonNull::new_unchecked(&mut (*ptr.as_ptr()).header) })
    }

    unsafe fn poll_shim(header: *mut TaskHeader, cx: &mut Context<'_>) -> Poll<NTSTATUS> {
        let task = header as *mut Task<F>;
        let fut = unsafe { &mut *(*task).future };
        let fut = unsafe { Pin::new_unchecked(fut) };
        fut.poll(cx)
    }

    unsafe fn destroy_shim(header: *mut TaskHeader, mode: DestroyMode) {
        let task = header as *mut Task<F>;
        match mode {
            DestroyMode::Drop => unsafe { ManuallyDrop::drop(&mut (*task).future) },
            DestroyMode::Dealloc => {
                let tag = unsafe { (*task).header.alloc_tag };
                let alloc = WdkAllocator::new(PoolType::NonPagedNx, tag);
                let layout = core::alloc::Layout::new::<Task<F>>();
                unsafe {
                    let task = NonNull::new_unchecked(task);
                    drop(KBox::from_raw_parts(task, alloc, layout));
                }
            }
        }
    }
}

/// Handle for requesting cancellation on a spawned task.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub struct CancelHandle {
    task: AtomicPtr<TaskHeader>,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl Send for CancelHandle {}
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl Sync for CancelHandle {}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl CancelHandle {
    #[inline]
    unsafe fn new(ptr: NonNull<TaskHeader>) -> Self {
        unsafe { TaskHeader::add_ref(ptr) };
        Self {
            task: AtomicPtr::new(ptr.as_ptr()),
        }
    }

    /// Request cancellation for the associated task.
    #[inline]
    pub fn cancel(&self) {
        let ptr = self.task.load(Ordering::Acquire);
        let Some(ptr) = NonNull::new(ptr) else {
            return;
        };

        unsafe { TaskHeader::cancel(ptr) };
    }

    /// Check whether cancellation has been requested.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        let ptr = self.task.load(Ordering::Acquire);
        let Some(ptr) = NonNull::new(ptr) else {
            return false;
        };

        unsafe { (*ptr.as_ptr()).cancel_requested.load(Ordering::Relaxed) != 0 }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl Drop for CancelHandle {
    fn drop(&mut self) {
        let ptr = self.task.swap(null_mut(), Ordering::AcqRel);
        if let Some(ptr) = NonNull::new(ptr) {
            unsafe { TaskHeader::release(ptr) };
        }
    }
}

/// Stub handle for non-kernel builds.
#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
pub struct CancelHandle {
    cancelled: Cell<bool>,
    future: RefCell<Option<Pin<Box<dyn Future<Output = NTSTATUS> + 'static>>>>,
}

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
impl CancelHandle {
    #[inline]
    fn new(future: Option<Pin<Box<dyn Future<Output = NTSTATUS> + 'static>>>) -> Self {
        Self {
            cancelled: Cell::new(false),
            future: RefCell::new(future),
        }
    }

    #[inline]
    pub fn cancel(&self) {
        self.cancelled.set(true);
        let _ = self.future.borrow_mut().take();
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel")), miri))]
impl Drop for CancelHandle {
    fn drop(&mut self) {
        let _ = self.future.borrow_mut().take();
    }
}

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM")), miri))]
pub struct WorkItemTracker;

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM")), miri))]
impl WorkItemTracker {
    #[inline]
    pub fn new() -> Self {
        Self
    }

    #[inline]
    pub fn drain(&self) {}
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
struct SpinLock<T> {
    lock: UnsafeCell<KSPIN_LOCK>,
    value: UnsafeCell<T>,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl<T: Send> Send for SpinLock<T> {}
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl<T: Send> Sync for SpinLock<T> {}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
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

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    old_irql: KIRQL,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl<T> core::ops::Deref for SpinLockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl<T> core::ops::DerefMut for SpinLockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl<T> Drop for SpinLockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe { KeReleaseSpinLock(self.lock.lock.get(), self.old_irql) };
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
struct KernelTimerInner {
    ref_count: AtomicU32,
    fired: AtomicU32,
    armed: AtomicU32,
    cancelled: AtomicU32,
    timer: KTIMER,
    dpc: KDPC,
    waker: SpinLock<Option<Waker>>,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl KernelTimerInner {
    unsafe fn allocate() -> Result<NonNull<Self>, NTSTATUS> {
        let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"irnt"));

        let layout = core::alloc::Layout::new::<KernelTimerInner>();

        let ptr = unsafe { alloc.alloc(layout) } as *mut KernelTimerInner;
        let ptr = NonNull::new(ptr).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

        unsafe {
            core::ptr::write(
                ptr.as_ptr(),
                KernelTimerInner {
                    ref_count: AtomicU32::new(1),
                    fired: AtomicU32::new(0),
                    armed: AtomicU32::new(0),
                    cancelled: AtomicU32::new(0),
                    timer: core::mem::zeroed(),
                    dpc: core::mem::zeroed(),
                    waker: SpinLock::new(None),
                },
            );
        }

        Ok(ptr)
    }

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
        let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"irnt"));
        let layout = core::alloc::Layout::new::<KernelTimerInner>();
        unsafe { drop(KBox::from_raw_parts(ptr, alloc, layout)) }
    }
}

/// A timer-based future for kernel mode.
///
/// `due_time_100ns` must be a relative negative interval in 100ns units
/// (i.e., like the `DueTime` passed to `KeSetTimer`).
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub struct KernelTimerFuture {
    inner: NonNull<KernelTimerInner>,
    due_time_100ns: i64,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl Send for KernelTimerFuture {}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl KernelTimerFuture {
    #[inline]
    pub fn new(due_time_100ns: i64) -> Result<Self, NTSTATUS> {
        let inner = unsafe { KernelTimerInner::allocate() }?;
        Ok(Self { inner, due_time_100ns })
    }

    unsafe extern "C" fn timer_dpc_routine(
        _dpc: PKDPC,
        deferred_context: *mut c_void,
        _system_argument1: *mut c_void,
        _system_argument2: *mut c_void,
    ) {
        let this = match NonNull::new(deferred_context as *mut KernelTimerInner) {
            Some(p) => p,
            None => return,
        };

        unsafe { &*this.as_ptr() }.fired.store(1, Ordering::Release);

        if unsafe { &*this.as_ptr() }.cancelled.load(Ordering::Acquire) == 0 {
            let guard = unsafe { &*this.as_ptr() }.waker.lock();

            if let Some(w) = guard.as_ref() {
                w.wake_by_ref();
            }
        }

        unsafe { KernelTimerInner::release(this) };
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl Future for KernelTimerFuture {
    type Output = NTSTATUS;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let inner = unsafe { &*this.inner.as_ptr() };

        if inner.fired.load(Ordering::Acquire) != 0 {
            return Poll::Ready(STATUS_SUCCESS);
        }

        {
            let mut guard = inner.waker.lock();
            *guard = Some(cx.waker().clone());
        }

        if inner
            .armed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            unsafe {
                KernelTimerInner::add_ref(this.inner);
                KeInitializeTimer(&mut (*this.inner.as_ptr()).timer as PKTIMER);
                KeInitializeDpc(
                    &mut (*this.inner.as_ptr()).dpc as PKDPC,
                    Some(Self::timer_dpc_routine),
                    this.inner.as_ptr() as *mut c_void,
                );

                let due = LARGE_INTEGER {
                    QuadPart: this.due_time_100ns,
                };
                let _ = KeSetTimer(
                    &mut (*this.inner.as_ptr()).timer as PKTIMER,
                    due,
                    &mut (*this.inner.as_ptr()).dpc as PKDPC,
                );
            }
        }
        Poll::Pending
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl Drop for KernelTimerFuture {
    fn drop(&mut self) {
        unsafe {
            let inner = self.inner;
            (*inner.as_ptr()).cancelled.store(1, Ordering::Release);
            let cancelled = KeCancelTimer(&mut (*inner.as_ptr()).timer as PKTIMER);
            let dpc_removed = KeRemoveQueueDpc(&mut (*inner.as_ptr()).dpc as PKDPC);
            KernelTimerInner::release(inner);
            if cancelled != 0 || dpc_removed != 0 {
                KernelTimerInner::release(inner);
            }
        }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
struct WorkItemTask<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    ref_count: AtomicU32,
    scheduled: AtomicU32,
    completed: AtomicU32,
    cancel_requested: AtomicU32,
    future: ManuallyDrop<F>,
    device: *mut DEVICE_OBJECT,
    tracker: *const WorkItemTracker,
    work_item: AtomicPtr<PIO_WORKITEM>,
}

/// Handle for requesting cancellation on a work-item task.
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub struct WorkItemCancelHandle<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    task: AtomicPtr<WorkItemTask<F>>,
}

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM")), miri))]
pub struct WorkItemCancelHandle<F>(core::marker::PhantomData<F>);

#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM")), miri))]
impl<F> WorkItemCancelHandle<F> {
    #[inline]
    pub fn cancel(&self) {}

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        false
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
unsafe impl<F> Send for WorkItemCancelHandle<F> where F: Future<Output = NTSTATUS> + Send + 'static {}
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
unsafe impl<F> Sync for WorkItemCancelHandle<F> where F: Future<Output = NTSTATUS> + Send + 'static {}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
impl<F> WorkItemCancelHandle<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    #[inline]
    unsafe fn new(ptr: NonNull<WorkItemTask<F>>) -> Self {
        unsafe { WorkItemTask::<F>::add_ref(ptr) };
        Self {
            task: AtomicPtr::new(ptr.as_ptr()),
        }
    }

    /// Request cancellation for the associated task.
    ///
    /// Cancellation queues a work item to drop the future, so call at PASSIVE_LEVEL.
    #[inline]
    pub fn cancel(&self) {
        let ptr = self.task.load(Ordering::Acquire);
        let Some(ptr) = NonNull::new(ptr) else {
            return;
        };

        unsafe { WorkItemTask::<F>::cancel(ptr) };
    }

    /// Check whether cancellation has been requested.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        let ptr = self.task.load(Ordering::Acquire);
        let Some(ptr) = NonNull::new(ptr) else {
            return false;
        };

        unsafe { (*ptr.as_ptr()).cancel_requested.load(Ordering::Relaxed) != 0 }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
impl<F> Clone for WorkItemCancelHandle<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    fn clone(&self) -> Self {
        let ptr = self.task.load(Ordering::Acquire);
        if let Some(ptr) = NonNull::new(ptr) {
            unsafe { WorkItemTask::<F>::add_ref(ptr) };
        }
        Self {
            task: AtomicPtr::new(ptr),
        }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
impl<F> Drop for WorkItemCancelHandle<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    fn drop(&mut self) {
        let ptr = self.task.swap(null_mut(), Ordering::AcqRel);
        if let Some(ptr) = NonNull::new(ptr) {
            unsafe { WorkItemTask::<F>::release(ptr) };
        }
    }
}

/// Tracks outstanding work items so you can drain them before driver unload.
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub struct WorkItemTracker {
    pending: AtomicU32,
    event: UnsafeCell<crate::ntddk::KEVENT>,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
unsafe impl Send for WorkItemTracker {}
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
unsafe impl Sync for WorkItemTracker {}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
impl WorkItemTracker {
    #[inline]
    pub fn new() -> Self {
        let mut event = unsafe { core::mem::zeroed() };
        unsafe {
            crate::ntddk::KeInitializeEvent(
                &mut event,
                crate::ntddk::SynchronizationEvent,
                1,
            );
        }
        Self {
            pending: AtomicU32::new(0),
            event: UnsafeCell::new(event),
        }
    }

    #[inline]
    fn begin(&self) {
        let prev = self.pending.fetch_add(1, Ordering::AcqRel);
        if prev == 0 {
            unsafe {
                crate::ntddk::KeInitializeEvent(
                    self.event.get(),
                    crate::ntddk::SynchronizationEvent,
                    0,
                );
            }
        }
    }

    #[inline]
    fn complete(&self) {
        let prev = self.pending.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            unsafe {
                crate::ntddk::KeSetEvent(self.event.get(), 0, 0);
            }
        }
    }

    /// Wait until all tracked work items have completed.
    ///
    /// Call only after you stop submitting new work items.
    #[inline]
    pub fn drain(&self) {
        if self.pending.load(Ordering::Acquire) == 0 {
            return;
        }

        unsafe {
            let _ = crate::ntddk::KeWaitForSingleObject(
                self.event.get() as *mut c_void,
                crate::ntddk::_KWAIT_REASON::Executive,
                crate::ntddk::_MODE::KernelMode as i8,
                0,
                core::ptr::null_mut(),
            );
        }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
#[inline]
unsafe fn tracker_begin(tracker: *const WorkItemTracker) {
    if !tracker.is_null() {
        unsafe { (*tracker).begin() };
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
#[inline]
unsafe fn tracker_complete(tracker: *const WorkItemTracker) {
    if !tracker.is_null() {
        unsafe { (*tracker).complete() };
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
impl<F> WorkItemTask<F>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    #[inline]
    fn alloc_tag() -> u32 {
        u32::from_ne_bytes(*b"kcow")
    }

    unsafe fn allocate(future: F) -> Result<NonNull<Self>, NTSTATUS> {
        let alloc = WdkAllocator::new(PoolType::NonPagedNx, Self::alloc_tag());
        let layout = core::alloc::Layout::new::<WorkItemTask<F>>();

        let ptr = unsafe { alloc.alloc(layout) } as *mut WorkItemTask<F>;
        let ptr = NonNull::new(ptr).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

        unsafe {
            core::ptr::write(
                ptr.as_ptr(),
                WorkItemTask {
                    ref_count: AtomicU32::new(1),
                    scheduled: AtomicU32::new(0),
                    completed: AtomicU32::new(0),
                    cancel_requested: AtomicU32::new(0),
                    future: ManuallyDrop::new(future),
                    device: null_mut(),
                    tracker: core::ptr::null(),
                    work_item: AtomicPtr::new(null_mut()),
                },
            );
        }

        Ok(ptr)
    }

    #[inline]
    unsafe fn add_ref(ptr: NonNull<Self>) {
        let task = unsafe { &*ptr.as_ptr() };
        let _ = refcount::add(&task.ref_count);
    }

    unsafe fn release(ptr: NonNull<Self>) {
        let task = unsafe { &*ptr.as_ptr() };
        let count = refcount::sub(&task.ref_count);
        if count != 0 {
            return;
        }

        core::sync::atomic::fence(Ordering::Acquire);
        unsafe { Self::free(ptr) }
    }

    unsafe fn free(ptr: NonNull<Self>) {
        let alloc = WdkAllocator::new(PoolType::NonPagedNx, Self::alloc_tag());
        let device = unsafe { (*ptr.as_ptr()).device };
        unsafe {
            drop(KBox::from_raw_parts(
                ptr,
                alloc,
                core::alloc::Layout::new::<WorkItemTask<F>>(),
            ));
        }
        if !device.is_null() {
            unsafe { ObDereferenceObject(device.cast()) };
        }
    }

    unsafe fn schedule(ptr: NonNull<Self>) -> NTSTATUS {
        if unsafe { &*ptr.as_ptr() }
            .completed
            .load(Ordering::Acquire)
            != 0
        {
            return STATUS_SUCCESS;
        }

        if unsafe { &*ptr.as_ptr() }
            .scheduled
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return STATUS_SUCCESS;
        }

        let tracker = unsafe { &*ptr.as_ptr() }.tracker;
        unsafe { tracker_begin(tracker) };

        let device = unsafe { &*ptr.as_ptr() }.device;
        if device.is_null() {
            unsafe { &*ptr.as_ptr() }.scheduled.store(0, Ordering::Release);
            unsafe { tracker_complete(tracker) };
            return STATUS_INVALID_PARAMETER;
        }

        let work_item = unsafe { IoAllocateWorkItem(device) };
        if work_item.is_null() {
            unsafe { &*ptr.as_ptr() }.scheduled.store(0, Ordering::Release);
            unsafe { tracker_complete(tracker) };
            return STATUS_INSUFFICIENT_RESOURCES;
        }

        unsafe { Self::add_ref(ptr) };
        unsafe { &*ptr.as_ptr() }
            .work_item
            .store(work_item, Ordering::Release);

        unsafe {
            IoQueueWorkItem(
                work_item,
                Some(Self::work_item_routine as PIO_WORKITEM_ROUTINE),
                WORK_QUEUE_TYPE::DelayedWorkQueue,
                ptr.as_ptr() as *mut c_void,
            );
        }

        STATUS_SUCCESS
    }

    #[inline]
    unsafe fn cancel(ptr: NonNull<Self>) {
        let header = unsafe { &*ptr.as_ptr() };
        if header
            .cancel_requested
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::Acquire)
            .is_ok()
        {
            let _ = unsafe { Self::schedule(ptr) };
        }
    }

    unsafe extern "C" fn work_item_routine(
        _device: *mut DEVICE_OBJECT,
        context: *mut c_void,
    ) {
        let ptr = match NonNull::new(context as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return,
        };

        let tracker = unsafe { &*ptr.as_ptr() }.tracker;

        unsafe { &*ptr.as_ptr() }.scheduled.store(0, Ordering::Release);

        if unsafe { &*ptr.as_ptr() }
            .completed
            .load(Ordering::Acquire)
            != 0
        {
            unsafe { tracker_complete(tracker) };
            unsafe { Self::release(ptr) };
            return;
        }

        let cancelled = unsafe { &*ptr.as_ptr() }
            .cancel_requested
            .load(Ordering::Acquire)
            != 0;
        if cancelled {
            unsafe { &*ptr.as_ptr() }.completed.store(1, Ordering::Release);
            unsafe { ManuallyDrop::drop(&mut (*ptr.as_ptr()).future) };
        } else {
            let waker = unsafe { Waker::from_raw(Self::raw_waker_borrowed(ptr)) };
            let mut cx = Context::from_waker(&waker);

            let poll = unsafe {
                let task = &mut *ptr.as_ptr();
                let fut = Pin::new_unchecked(&mut *task.future);
                fut.poll(&mut cx)
            };

            if let Poll::Ready(_status) = poll {
                unsafe { &*ptr.as_ptr() }.completed.store(1, Ordering::Release);
                unsafe { ManuallyDrop::drop(&mut (*ptr.as_ptr()).future) };
            }
        }

        let work_item = unsafe { &*ptr.as_ptr() }
            .work_item
            .swap(null_mut(), Ordering::AcqRel);
        if !work_item.is_null() {
            unsafe { IoFreeWorkItem(work_item) };
        }

        unsafe { tracker_complete(tracker) };
        unsafe { Self::release(ptr) };
    }

    #[inline]
    unsafe fn raw_waker_owned(ptr: NonNull<Self>) -> RawWaker {
        unsafe { Self::add_ref(ptr) };
        RawWaker::new(ptr.as_ptr() as *const (), &Self::RAW_WAKER_VTABLE)
    }

    #[inline]
    unsafe fn raw_waker_borrowed(ptr: NonNull<Self>) -> RawWaker {
        RawWaker::new(ptr.as_ptr() as *const (), &Self::RAW_WAKER_BORROWED_VTABLE)
    }

    const RAW_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw_owned,
        Self::wake_raw_owned,
        Self::wake_by_ref_raw_owned,
        Self::drop_raw_owned,
    );

    const RAW_WAKER_BORROWED_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_raw_owned,
        Self::wake_raw_borrowed,
        Self::wake_by_ref_raw_borrowed,
        Self::drop_raw_borrowed,
    );

    unsafe fn clone_raw_owned(ptr: *const ()) -> RawWaker {
        let ptr = match NonNull::new(ptr as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return RawWaker::new(core::ptr::null(), &Self::RAW_WAKER_VTABLE),
        };
        unsafe { Self::raw_waker_owned(ptr) }
    }

    unsafe fn wake_raw_owned(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return,
        };
        let _ = unsafe { Self::schedule(ptr) };
        unsafe { Self::release(ptr) };
    }

    unsafe fn wake_by_ref_raw_owned(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return,
        };
        let _ = unsafe { Self::schedule(ptr) };
    }

    unsafe fn drop_raw_owned(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return,
        };
        unsafe { Self::release(ptr) };
    }

    unsafe fn wake_raw_borrowed(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return,
        };
        let _ = unsafe { Self::schedule(ptr) };
    }

    unsafe fn wake_by_ref_raw_borrowed(ptr: *const ()) {
        let ptr = match NonNull::new(ptr as *mut WorkItemTask<F>) {
            Some(p) => p,
            None => return,
        };
        let _ = unsafe { Self::schedule(ptr) };
    }

    unsafe fn drop_raw_borrowed(_ptr: *const ()) {}
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
unsafe impl<F> Send for WorkItemTask<F> where F: Future<Output = NTSTATUS> + Send + 'static {}
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
unsafe impl<F> Sync for WorkItemTask<F> where F: Future<Output = NTSTATUS> + Send + 'static {}

/// Spawn a future onto the PASSIVE_LEVEL work-item executor (WDM).
///
/// Note: ensure outstanding work items are drained before driver unload to
/// avoid freeing device objects while work-item callbacks are still running.
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub fn spawn_task<F>(device: *mut DEVICE_OBJECT, future: F) -> NTSTATUS
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    if device.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let ptr = match unsafe { WorkItemTask::<F>::allocate(future) } {
        Ok(p) => p,
        Err(s) => return s,
    };

    unsafe {
        if !device.is_null() {
            ObReferenceObject(device.cast());
        }
        (&mut *ptr.as_ptr()).device = device;
    }
    let status = unsafe { WorkItemTask::<F>::schedule(ptr) };
    unsafe { WorkItemTask::<F>::release(ptr) };

    status
}

/// Spawn a future onto the PASSIVE_LEVEL work-item executor (WDM), tracking
/// outstanding work so you can drain before unload.
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub fn spawn_task_tracked<F>(
    device: *mut DEVICE_OBJECT,
    tracker: &WorkItemTracker,
    future: F,
) -> NTSTATUS
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    if device.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let ptr = match unsafe { WorkItemTask::<F>::allocate(future) } {
        Ok(p) => p,
        Err(s) => return s,
    };

    unsafe {
        let task = &mut *ptr.as_ptr();
        if !device.is_null() {
            ObReferenceObject(device.cast());
        }
        task.device = device;
        task.tracker = tracker as *const WorkItemTracker;
    }
    let status = unsafe { WorkItemTask::<F>::schedule(ptr) };
    unsafe { WorkItemTask::<F>::release(ptr) };

    status
}

/// Spawn a future onto the PASSIVE_LEVEL work-item executor (WDM) and return a
/// cancellation handle.
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub fn spawn_task_cancellable<F>(
    device: *mut DEVICE_OBJECT,
    future: F,
) -> Result<WorkItemCancelHandle<F>, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    if device.is_null() {
        return Err(STATUS_INVALID_PARAMETER);
    }
    let ptr = match unsafe { WorkItemTask::<F>::allocate(future) } {
        Ok(p) => p,
        Err(s) => return Err(s),
    };

    unsafe {
        let task = &mut *ptr.as_ptr();
        if !device.is_null() {
            ObReferenceObject(device.cast());
        }
        task.device = device;
        task.tracker = core::ptr::null();
    }

    let handle = unsafe { WorkItemCancelHandle::new(ptr) };
    let status = unsafe { WorkItemTask::<F>::schedule(ptr) };
    unsafe { WorkItemTask::<F>::release(ptr) };

    if status != STATUS_SUCCESS {
        drop(handle);
        return Err(status);
    }

    Ok(handle)
}

/// Spawn a future onto the kcom DPC executor (driver build without async-com-kernel).
#[cfg(all(feature = "driver", not(feature = "async-com-kernel"), not(miri)))]
pub unsafe fn spawn_dpc_task_cancellable<F>(_future: F) -> Result<CancelHandle, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    Err(STATUS_NOT_SUPPORTED)
}

/// Spawn a future onto the kcom DPC executor (host stub).
#[cfg(any(not(feature = "driver"), miri))]
pub unsafe fn spawn_dpc_task_cancellable<F>(future: F) -> Result<CancelHandle, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + 'static,
{
    let waker = dummy_waker();

    let mut cx = Context::from_waker(&waker);
    let mut future: Pin<Box<dyn Future<Output = NTSTATUS> + 'static>> = Box::pin(future);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(_) => Ok(CancelHandle::new(None)),
        Poll::Pending => Ok(CancelHandle::new(Some(future))),
    }
}

/// Spawn a future onto the PASSIVE_LEVEL work-item executor (WDM), tracking
/// outstanding work and returning a cancellation handle.
#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub fn spawn_task_cancellable_tracked<F>(
    device: *mut DEVICE_OBJECT,
    tracker: &WorkItemTracker,
    future: F,
) -> Result<WorkItemCancelHandle<F>, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    if device.is_null() {
        return Err(STATUS_INVALID_PARAMETER);
    }
    let ptr = match unsafe { WorkItemTask::<F>::allocate(future) } {
        Ok(p) => p,
        Err(s) => return Err(s),
    };

    unsafe {
        let task = &mut *ptr.as_ptr();
        if !device.is_null() {
            ObReferenceObject(device.cast());
        }
        task.device = device;
        task.tracker = tracker as *const WorkItemTracker;
    }

    let handle = unsafe { WorkItemCancelHandle::new(ptr) };
    let status = unsafe { WorkItemTask::<F>::schedule(ptr) };
    unsafe { WorkItemTask::<F>::release(ptr) };

    if status != STATUS_SUCCESS {
        drop(handle);
        return Err(status);
    }

    Ok(handle)
}

/// Spawn a future onto the PASSIVE_LEVEL work-item executor (unsupported builds).
#[cfg(any(not(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM")), miri))]
pub fn spawn_task_cancellable_tracked<F>(
    _device: *mut DEVICE_OBJECT,
    _tracker: &WorkItemTracker,
    _future: F,
) -> Result<WorkItemCancelHandle<F>, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    Err(STATUS_NOT_SUPPORTED)
}

/// Tracks outstanding DPC tasks so you can drain them before driver unload.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub struct TaskTracker {
    pending: AtomicU32,
    event: UnsafeCell<crate::ntddk::KEVENT>,
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl Send for TaskTracker {}
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
unsafe impl Sync for TaskTracker {}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
impl TaskTracker {
    #[inline]
    pub fn new() -> Self {
        let mut event = unsafe { core::mem::zeroed() };
        unsafe {
            crate::ntddk::KeInitializeEvent(
                &mut event,
                crate::ntddk::SynchronizationEvent,
                1,
            );
        }
        Self {
            pending: AtomicU32::new(0),
            event: UnsafeCell::new(event),
        }
    }

    #[inline]
    fn begin(&self) {
        let prev = self.pending.fetch_add(1, Ordering::AcqRel);
        if prev == 0 {
            unsafe {
                crate::ntddk::KeInitializeEvent(
                    self.event.get(),
                    crate::ntddk::SynchronizationEvent,
                    0,
                );
            }
        }
    }

    #[inline]
    fn complete(&self) {
        let prev = self.pending.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            unsafe {
                crate::ntddk::KeSetEvent(self.event.get(), 0, 0);
            }
        }
    }

    /// Wait until all tracked DPC tasks have completed.
    ///
    /// Call only after you stop submitting new tasks.
    #[inline]
    pub fn drain(&self) {
        if self.pending.load(Ordering::Acquire) == 0 {
            return;
        }

        unsafe {
            let _ = crate::ntddk::KeWaitForSingleObject(
                self.event.get() as *mut c_void,
                crate::ntddk::_KWAIT_REASON::Executive,
                crate::ntddk::_MODE::KernelMode as i8,
                0,
                core::ptr::null_mut(),
            );
        }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
unsafe fn task_tracker_begin(tracker: *const TaskTracker) {
    if !tracker.is_null() {
        unsafe { (*tracker).begin() };
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
#[inline]
unsafe fn task_tracker_complete(tracker: *const TaskTracker) {
    if !tracker.is_null() {
        unsafe { (*tracker).complete() };
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
/// Spawn a future onto the kcom DPC executor and return a cancellation handle.
///
/// # IRQL
/// The future is polled at DISPATCH_LEVEL. It must use nonpaged memory, must not
/// block, and must avoid pageable kernel APIs. For PASSIVE_LEVEL work, prefer the
/// work-item executor APIs.
///
/// # Driver unload
/// Untracked tasks can outlive driver unload. Prefer
/// [`spawn_dpc_task_cancellable_tracked`] with a [`TaskTracker`] and call
/// [`TaskTracker::drain`] during unload.
pub unsafe fn spawn_dpc_task_cancellable<F>(future: F) -> Result<CancelHandle, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    let ptr = match unsafe { Task::<F>::allocate(future, core::ptr::null()) } {
        Ok(p) => p,
        Err(s) => return Err(s),
    };

    let handle = unsafe { CancelHandle::new(ptr) };
    unsafe { TaskHeader::schedule(ptr) };
    unsafe { TaskHeader::release(ptr) };

    Ok(handle)
}

/// Spawn a future onto the kcom DPC executor, tracking outstanding tasks.
///
/// # IRQL
/// The future is polled at DISPATCH_LEVEL. It must use nonpaged memory, must not
/// block, and must avoid pageable kernel APIs. For PASSIVE_LEVEL work, prefer the
/// work-item executor APIs.
///
/// # Driver unload
/// Call [`TaskTracker::drain`] after stopping submissions to ensure all tracked
/// tasks have completed before unload.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub unsafe fn spawn_dpc_task_cancellable_tracked<F>(
    tracker: &TaskTracker,
    future: F,
) -> Result<CancelHandle, NTSTATUS>
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    let ptr = match unsafe { Task::<F>::allocate(future, tracker as *const TaskTracker) } {
        Ok(p) => p,
        Err(s) => return Err(s),
    };

    let handle = unsafe { CancelHandle::new(ptr) };
    unsafe { TaskHeader::schedule(ptr) };
    unsafe { TaskHeader::release(ptr) };

    Ok(handle)
}

/// Spawn a future onto the kcom DPC executor.
///
/// # IRQL
/// The future is polled at DISPATCH_LEVEL. It must use nonpaged memory, must not
/// block, and must avoid pageable kernel APIs. For PASSIVE_LEVEL work, prefer the
/// work-item executor APIs.
///
/// # Driver unload
/// This function requires a [`TaskTracker`] to ensure all tasks have completed
/// before unload.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub unsafe fn spawn_dpc_task<F>(tracker: &TaskTracker, future: F) -> NTSTATUS
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    spawn_dpc_task_tracked(tracker, future)
}

/// Compatibility wrapper for spawning a tracked DPC task.
#[cfg(all(feature = "driver", feature = "async-com-kernel", not(miri)))]
pub unsafe fn spawn_dpc_task_tracked<F>(tracker: &TaskTracker, future: F) -> NTSTATUS
where
    F: Future<Output = NTSTATUS> + Send + 'static,
{
    let ptr = match unsafe { Task::<F>::allocate(future, tracker as *const TaskTracker) } {
        Ok(p) => p,
        Err(s) => return s,
    };

    unsafe { TaskHeader::schedule(ptr) };
    unsafe { TaskHeader::release(ptr) };

    STATUS_SUCCESS
}

/// Spawn a future onto the kcom executor (host stub).
#[cfg(any(not(feature = "driver"), miri))]
pub fn spawn_task<F>(mut future: F) -> NTSTATUS
where
    F: Future<Output = NTSTATUS> + 'static,
{
    let waker = dummy_waker();
    let mut cx = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(_s) => STATUS_SUCCESS,
        Poll::Pending => STATUS_SUCCESS,
    }
}

