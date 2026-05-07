#[cfg(all(feature = "driver", feature = "async-com-kernel"))]
mod driver_only {
    use kcom::{spawn_dpc_task, KernelTimerFuture, NTSTATUS, STATUS_SUCCESS, TaskTracker};

    pub fn run() -> NTSTATUS {
        let fut = async move {
            let timer = match KernelTimerFuture::new(-10_000) {
                Ok(timer) => timer,
                Err(status) => return status,
            };
            timer.await;
            STATUS_SUCCESS
        };

        let tracker = TaskTracker::new();
        let status = unsafe { spawn_dpc_task(&tracker, fut) };
        tracker.drain();
        status
    }
}

fn main() {
    #[cfg(all(feature = "driver", feature = "async-com-kernel"))]
    {
        let status = driver_only::run();
        let _ = status;
    }

    #[cfg(not(all(feature = "driver", feature = "async-com-kernel")))]
    {
    }
}
#[cfg(feature = "driver")]
mod driver_entry_stub;
