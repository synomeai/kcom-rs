// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Lock-free state machines shared by the kernel executors, IRP futures and
//! the async COM pool.
//!
//! They only use core atomics so they can be unit-tested (and run under Miri)
//! on the host, while the kernel code that drives them can only be built with
//! the WDK.

#![cfg_attr(
    not(all(feature = "driver", feature = "async-com-kernel", not(miri))),
    allow(dead_code)
)]

use core::alloc::Layout;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};

/// Scheduling state of a task polled by a DPC or work-item callback.
///
/// Guarantees that at most one callback polls the future at a time and that a
/// wake arriving while the task is being polled is not lost:
///
/// - `schedule` moves an idle task to `QUEUED`; only that caller queues the
///   callback. A wake while `QUEUED` is absorbed, a wake while `RUNNING` sets
///   `NOTIFIED`.
/// - The callback calls `begin_run` (`QUEUED` -> `RUNNING`), polls, and then
///   `end_run`, which either goes idle or straight back to `QUEUED` (when
///   notified or asked to requeue), in which case the callback queues itself
///   again. A new callback can therefore only start after the previous one
///   has left `RUNNING`.
pub(crate) struct PollState(AtomicU32);

impl PollState {
    const QUEUED: u32 = 1;
    const RUNNING: u32 = 2;
    const NOTIFIED: u32 = 4;

    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Returns `true` if the caller must queue the callback.
    pub(crate) fn schedule(&self) -> bool {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            let next = if state & Self::RUNNING != 0 {
                if state & Self::NOTIFIED != 0 {
                    return false;
                }
                state | Self::NOTIFIED
            } else if state & Self::QUEUED != 0 {
                return false;
            } else {
                state | Self::QUEUED
            };
            match self
                .0
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return next & Self::NOTIFIED == 0,
                Err(actual) => state = actual,
            }
        }
    }

    /// Undo a successful `schedule` whose callback could not be queued.
    pub(crate) fn abort_queue(&self) {
        self.0.fetch_and(!Self::QUEUED, Ordering::AcqRel);
    }

    /// Called at the start of the queued callback.
    pub(crate) fn begin_run(&self) {
        let prev = self.0.swap(Self::RUNNING, Ordering::AcqRel);
        debug_assert!(prev & Self::RUNNING == 0, "task polled concurrently");
        let _ = prev;
    }

    /// Consumes a wake that arrived during the last poll.
    pub(crate) fn take_notified(&self) -> bool {
        self.0.fetch_and(!Self::NOTIFIED, Ordering::AcqRel) & Self::NOTIFIED != 0
    }

    /// Leaves `RUNNING`. Returns `true` if the caller must queue the callback
    /// again (a wake arrived, or `requeue` was requested).
    pub(crate) fn end_run(&self, requeue: bool) -> bool {
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            debug_assert!(state & Self::RUNNING != 0);
            let again = requeue || state & Self::NOTIFIED != 0;
            let next = if again { Self::QUEUED } else { 0 };
            match self
                .0
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return again,
                Err(actual) => state = actual,
            }
        }
    }
}

/// Hand-off between a canceller (`IoCancelIrp`) and the completion path, so
/// the IRP is never freed while the canceller may still be using it.
///
/// The canceller brackets its use with `begin_cancel`/`end_cancel`. The
/// completion path calls `complete` with the item it would free; if a cancel
/// is in progress, disposal is deferred and `end_cancel` hands the item back
/// to the canceller instead.
pub(crate) struct CancelHandshake<T> {
    state: AtomicU32,
    deferred: AtomicPtr<T>,
}

impl<T> CancelHandshake<T> {
    const CANCELLING: u32 = 1;
    const CANCEL_DONE: u32 = 2;
    const COMPLETED: u32 = 4;

    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            deferred: AtomicPtr::new(null_mut()),
        }
    }

    /// Returns `true` if the item is still live and may be used until
    /// `end_cancel`. Only call `end_cancel` when this returned `true`.
    pub(crate) fn begin_cancel(&self) -> bool {
        self.state.fetch_or(Self::CANCELLING, Ordering::AcqRel) & Self::COMPLETED == 0
    }

    /// Returns the item if the completion path deferred its disposal to us.
    pub(crate) fn end_cancel(&self) -> Option<*mut T> {
        let prev = self.state.fetch_or(Self::CANCEL_DONE, Ordering::AcqRel);
        if prev & Self::COMPLETED != 0 {
            Some(self.deferred.swap(null_mut(), Ordering::AcqRel))
        } else {
            None
        }
    }

    /// Returns the item if the caller should dispose of it now.
    pub(crate) fn complete(&self, item: *mut T) -> Option<*mut T> {
        self.deferred.store(item, Ordering::Release);
        let prev = self.state.fetch_or(Self::COMPLETED, Ordering::AcqRel);
        let cancelling = prev & Self::CANCELLING != 0 && prev & Self::CANCEL_DONE == 0;
        if cancelling {
            None
        } else {
            Some(self.deferred.swap(null_mut(), Ordering::AcqRel))
        }
    }
}

/// Outstanding-work counter for drain-before-unload trackers.
///
/// The count starts with a bias of one that only `release_bias` removes, so it
/// can reach zero exactly once, after draining has begun and every tracked
/// item has completed. Whoever brings it to zero signals the drain event, and
/// that signal is its last access to the tracker; the drainer waits for the
/// event rather than polling the count.
pub(crate) struct DrainCount {
    pending: AtomicU32,
    draining: AtomicBool,
}

impl DrainCount {
    pub(crate) const fn new() -> Self {
        Self {
            pending: AtomicU32::new(1),
            draining: AtomicBool::new(false),
        }
    }

    pub(crate) fn begin(&self) {
        self.pending.fetch_add(1, Ordering::AcqRel);
    }

    /// Returns `true` if the caller must signal the drain event.
    pub(crate) fn complete(&self) -> bool {
        self.pending.fetch_sub(1, Ordering::AcqRel) == 1
    }

    /// Starts draining. Returns `true` if the caller must signal the drain
    /// event itself (nothing was outstanding). Idempotent.
    pub(crate) fn release_bias(&self) -> bool {
        if self.draining.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.complete()
    }

    #[cfg(test)]
    fn pending(&self) -> u32 {
        self.pending.load(Ordering::Acquire)
    }
}

/// One-time initialization that must happen at an object's final address
/// (kernel dispatcher objects are self-referential and must not be moved).
pub(crate) struct InitOnce(AtomicU32);

impl InitOnce {
    const UNINIT: u32 = 0;
    const RUNNING: u32 = 1;
    const DONE: u32 = 2;

    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(Self::UNINIT))
    }

    pub(crate) fn call(&self, init: impl FnOnce()) {
        if self.0.load(Ordering::Acquire) == Self::DONE {
            return;
        }
        if self
            .0
            .compare_exchange(Self::UNINIT, Self::RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            init();
            self.0.store(Self::DONE, Ordering::Release);
            return;
        }
        while self.0.load(Ordering::Acquire) != Self::DONE {
            core::hint::spin_loop();
        }
    }
}

/// Fixed table that assigns one slot per distinct allocation layout.
///
/// Generic functions share a single instance of any `static` declared inside
/// them, so per-type pools must be keyed by something else; the layout is
/// what the pool actually depends on.
pub(crate) struct LayoutSlots<const N: usize> {
    keys: [AtomicUsize; N],
}

impl<const N: usize> LayoutSlots<N> {
    pub(crate) const fn new() -> Self {
        Self {
            keys: [const { AtomicUsize::new(0) }; N],
        }
    }

    fn key(layout: Layout) -> Option<usize> {
        if layout.size() == 0 {
            return None;
        }
        let shifted = layout.size().checked_mul(64)?;
        // Never zero (size > 0), which marks an empty slot.
        Some(shifted | layout.align().trailing_zeros() as usize)
    }

    /// Returns the slot for `layout`, claiming a free one if needed.
    pub(crate) fn find_or_claim(&self, layout: Layout) -> Option<usize> {
        let key = Self::key(layout)?;
        for (index, slot) in self.keys.iter().enumerate() {
            match slot.compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(index),
                Err(existing) if existing == key => return Some(index),
                Err(_) => {}
            }
        }
        None
    }

    /// Returns the slot already assigned to `layout`, if any.
    pub(crate) fn find(&self, layout: Layout) -> Option<usize> {
        let key = Self::key(layout)?;
        self.keys
            .iter()
            .position(|slot| slot.load(Ordering::Acquire) == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;
    use std::sync::atomic::AtomicUsize as StdAtomicUsize;
    use std::sync::Arc;
    use std::thread;
    use std::vec::Vec;

    #[test]
    fn poll_state_wake_while_running_requeues_once() {
        let state = PollState::new();
        assert!(state.schedule());
        assert!(!state.schedule(), "already queued");
        state.begin_run();
        assert!(!state.schedule(), "running: only notifies");
        assert!(!state.schedule());
        assert!(state.end_run(false), "notified while running");
        state.begin_run();
        assert!(!state.end_run(false));
        assert!(state.schedule(), "idle again");
    }

    #[test]
    fn poll_state_take_notified_and_forced_requeue() {
        let state = PollState::new();
        assert!(state.schedule());
        state.begin_run();
        assert!(!state.take_notified());
        assert!(!state.schedule());
        assert!(state.take_notified());
        assert!(!state.end_run(false));
        assert!(state.schedule());
        state.begin_run();
        assert!(state.end_run(true), "budget exhausted");
        assert!(!state.schedule(), "queued by end_run");
    }

    #[test]
    fn poll_state_abort_queue() {
        let state = PollState::new();
        assert!(state.schedule());
        state.abort_queue();
        assert!(state.schedule());
    }

    /// Simulates an executor: wakers race with the callback, which may queue
    /// itself again. The future must never be polled concurrently and every
    /// wake must be followed by a poll.
    #[test]
    fn poll_state_never_runs_concurrently() {
        struct Task {
            state: PollState,
            polling: AtomicBool,
            polls: StdAtomicUsize,
        }

        fn run(task: Arc<Task>) {
            let mut requeue = true;
            while requeue {
                task.state.begin_run();
                assert!(!task.polling.swap(true, Ordering::AcqRel), "concurrent poll");
                task.polls.fetch_add(1, Ordering::Relaxed);
                task.polling.store(false, Ordering::Release);
                requeue = task.state.end_run(false);
            }
        }

        let task = Arc::new(Task {
            state: PollState::new(),
            polling: AtomicBool::new(false),
            polls: StdAtomicUsize::new(0),
        });
        let wakers: Vec<_> = (0..3)
            .map(|_| {
                let task = task.clone();
                thread::spawn(move || {
                    let mut runners = Vec::new();
                    for _ in 0..20 {
                        if task.state.schedule() {
                            let task = task.clone();
                            runners.push(thread::spawn(move || run(task)));
                        }
                    }
                    for runner in runners {
                        runner.join().unwrap();
                    }
                })
            })
            .collect();
        for waker in wakers {
            waker.join().unwrap();
        }
        assert!(task.polls.load(Ordering::Relaxed) >= 1);
        // Everything settled: the task is idle and can be scheduled again.
        assert!(task.state.schedule());
    }

    #[test]
    fn cancel_handshake_sequential_orders() {
        let a = Box::into_raw(Box::new(1u32));
        // Completed before any cancel: completer disposes, canceller skips.
        let h = CancelHandshake::new();
        assert_eq!(h.complete(a), Some(a));
        assert!(!h.begin_cancel());

        // Completed during cancel: canceller disposes.
        let h = CancelHandshake::new();
        assert!(h.begin_cancel());
        assert_eq!(h.complete(a), None);
        assert_eq!(h.end_cancel(), Some(a));

        // Completed after cancel finished: completer disposes.
        let h = CancelHandshake::new();
        assert!(h.begin_cancel());
        assert_eq!(h.end_cancel(), None);
        assert_eq!(h.complete(a), Some(a));
        unsafe { drop(Box::from_raw(a)) };
    }

    #[test]
    fn cancel_handshake_races_dispose_exactly_once() {
        for _ in 0..50 {
            let item = Box::into_raw(Box::new(7u64));
            let h = Arc::new(CancelHandshake::new());
            let canceller = {
                let h = h.clone();
                thread::spawn(move || {
                    if h.begin_cancel() {
                        // Using the item here must be sound.
                        let item = h.deferred.load(Ordering::Acquire);
                        let _ = item;
                        h.end_cancel().map(|p| p as usize)
                    } else {
                        None
                    }
                })
            };
            let item_addr = item as usize;
            let completer = {
                let h = h.clone();
                thread::spawn(move || h.complete(item_addr as *mut u64).map(|p| p as usize))
            };
            let a = canceller.join().unwrap();
            let b = completer.join().unwrap();
            match (a, b) {
                (Some(p), None) | (None, Some(p)) => {
                    assert_eq!(p, item as usize);
                    unsafe { drop(Box::from_raw(p as *mut u64)) };
                }
                other => panic!("disposed {other:?}"),
            }
        }
    }

    #[test]
    fn drain_count_reaches_zero_once() {
        let c = DrainCount::new();
        c.begin();
        c.begin();
        assert!(!c.complete());
        assert!(!c.release_bias(), "one still outstanding");
        assert!(!c.release_bias(), "idempotent");
        assert!(c.complete(), "last completer signals");
        assert_eq!(c.pending(), 0);

        let idle = DrainCount::new();
        assert!(idle.release_bias(), "nothing outstanding: drainer signals");
    }

    #[test]
    fn init_once_runs_once() {
        let once = Arc::new(InitOnce::new());
        let runs = Arc::new(StdAtomicUsize::new(0));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let once = once.clone();
                let runs = runs.clone();
                thread::spawn(move || once.call(|| {
                    runs.fetch_add(1, Ordering::Relaxed);
                }))
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(runs.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn layout_slots_are_per_layout() {
        let slots = LayoutSlots::<2>::new();
        let small = Layout::from_size_align(64, 8).unwrap();
        let large = Layout::from_size_align(256, 8).unwrap();
        let aligned = Layout::from_size_align(64, 16).unwrap();
        assert_eq!(slots.find(small), None);
        let s = slots.find_or_claim(small).unwrap();
        let l = slots.find_or_claim(large).unwrap();
        assert_ne!(s, l);
        assert_eq!(slots.find_or_claim(small), Some(s));
        assert_eq!(slots.find(large), Some(l));
        assert_eq!(slots.find_or_claim(aligned), None, "table full");
        assert_eq!(slots.find(aligned), None);
        assert_eq!(slots.find_or_claim(Layout::new::<()>()), None);
    }
}
