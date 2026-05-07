use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use kcom::{NTSTATUS, STATUS_SUCCESS};

#[cfg(all(feature = "driver", feature = "async-com-kernel"))]
use kcom::spawn_dpc_task;
#[cfg(not(feature = "driver"))]
use kcom::spawn_task;

#[cfg(all(feature = "driver", feature = "async-com-kernel"))]
use kcom::TaskTracker;

struct YieldOnce {
    yielded: bool,
}

impl Future for YieldOnce {
    type Output = NTSTATUS;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(STATUS_SUCCESS)
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[cfg(all(feature = "driver", feature = "async-com-kernel"))]
fn main() {
    let tracker = TaskTracker::new();
    let status = unsafe { spawn_dpc_task(&tracker, YieldOnce { yielded: false }) };
    assert_eq!(status, STATUS_SUCCESS);
    tracker.drain();
}

#[cfg(not(feature = "driver"))]
fn main() {
    let status = spawn_task(YieldOnce { yielded: false });
    assert_eq!(status, STATUS_SUCCESS);
}
#[cfg(feature = "driver")]
mod driver_entry_stub;
