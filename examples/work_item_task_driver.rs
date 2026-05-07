#[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM"))]
mod driver_only {
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use kcom::{spawn_task, spawn_task_cancellable};
    use kcom::NTSTATUS;
    use kcom::iunknown::{STATUS_NOT_SUPPORTED, STATUS_SUCCESS};

    struct ReadyOk;

    impl Future for ReadyOk {
        type Output = NTSTATUS;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(STATUS_SUCCESS)
        }
    }

    pub fn run() {
        // In a real driver, pass a valid DEVICE_OBJECT from DriverEntry.
        let device = core::ptr::null_mut();
        let status = spawn_task(device, ReadyOk);
        let _ = status;

        let handle = spawn_task_cancellable(device, ReadyOk);
        let _ = handle.as_ref().map(|h| h.cancel());

        // STATUS_NOT_SUPPORTED is returned if no device was set.
        if status == STATUS_NOT_SUPPORTED {
            // Expected when running this example without a real DEVICE_OBJECT.
        }
    }
}

fn main() {
    #[cfg(all(feature = "driver", feature = "async-com-kernel", driver_model__driver_type = "WDM"))]
    driver_only::run();
}
#[cfg(feature = "driver")]
mod driver_entry_stub;
