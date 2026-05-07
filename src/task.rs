// task.rs
//
// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::future::Future;
use core::mem::ManuallyDrop;
use core::pin::Pin;
use core::task::{Context, Poll};

#[derive(Copy, Clone, Eq, PartialEq)]
enum CancellableState {
    RunningMain,
    RunningCleanup,
    Done,
}

/// A cancellable future with async cleanup.
pub struct Cancellable<M, C> {
    state: CancellableState,
    main: ManuallyDrop<M>,
    cleanup: ManuallyDrop<C>,
}

impl<M, C> Cancellable<M, C> {
    #[inline]
    fn new(main: M, cleanup: C) -> Self {
        Self {
            state: CancellableState::RunningMain,
            main: ManuallyDrop::new(main),
            cleanup: ManuallyDrop::new(cleanup),
        }
    }
}

impl<M, C> Future for Cancellable<M, C>
where
    M: Future,
    C: Future<Output = ()>,
{
    type Output = Option<M::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        loop {
            match this.state {
                CancellableState::RunningMain => {
                    if crate::executor::take_cancellation_request() {
                        unsafe { ManuallyDrop::drop(&mut this.main) };
                        this.state = CancellableState::RunningCleanup;
                        continue;
                    }

                    let main = unsafe { Pin::new_unchecked(&mut *this.main) };
                    match main.poll(cx) {
                        Poll::Ready(value) => {
                            unsafe { ManuallyDrop::drop(&mut this.main) };
                            unsafe { ManuallyDrop::drop(&mut this.cleanup) };
                            this.state = CancellableState::Done;
                            return Poll::Ready(Some(value));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                CancellableState::RunningCleanup => {
                    let cleanup = unsafe { Pin::new_unchecked(&mut *this.cleanup) };
                    match cleanup.poll(cx) {
                        Poll::Ready(()) => {
                            unsafe { ManuallyDrop::drop(&mut this.cleanup) };
                            this.state = CancellableState::Done;
                            return Poll::Ready(None);
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                CancellableState::Done => panic!("Polled after completion"),
            }
        }
    }
}

impl<M, C> Drop for Cancellable<M, C> {
    fn drop(&mut self) {
        unsafe {
            match self.state {
                CancellableState::RunningMain => {
                    ManuallyDrop::drop(&mut self.main);
                    ManuallyDrop::drop(&mut self.cleanup);
                }
                CancellableState::RunningCleanup => {
                    ManuallyDrop::drop(&mut self.cleanup);
                }
                CancellableState::Done => {}
            }
        }
    }
}

/// Construct a cancellable future with async cleanup.
#[inline]
pub fn try_finally<M, C>(main: M, cleanup: C) -> Cancellable<M, C>
where
    M: Future,
    C: Future<Output = ()>,
{
    Cancellable::new(main, cleanup)
}

/// A future that yields once before completing.
#[must_use]
pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Yield execution back to the executor once.
#[inline]
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

/// Wrap a future and force cooperative yielding every `interval` polls.
#[must_use]
pub struct Cooperative<F> {
    inner: F,
    interval: u32,
    polls: u32,
}

impl<F> Cooperative<F> {
    #[inline]
    fn new(inner: F, interval: u32) -> Self {
        Self {
            inner,
            interval,
            polls: 0,
        }
    }
}

impl<F> Future for Cooperative<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if this.interval != 0 && this.polls >= this.interval {
            this.polls = 0;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        let poll = unsafe { Pin::new_unchecked(&mut this.inner) }.poll(cx);
        if poll.is_pending() && this.interval != 0 {
            this.polls = this.polls.saturating_add(1);
        }
        poll
    }
}

/// Wrap a future to yield every `interval` polls (0 disables forced yields).
#[inline]
pub fn cooperative<F>(inner: F, interval: u32) -> Cooperative<F>
where
    F: Future,
{
    Cooperative::new(inner, interval)
}
