#[cfg(all(feature = "async-com", any(not(feature = "driver"), feature = "async-com-kernel")))]
mod async_operation_spec {
    use kcom::{spawn_async_operation, AsyncStatus};

    #[test]
    fn async_operation_ready_future_completes() {
        let op = spawn_async_operation(async { 7u32 }).expect("spawn async operation");

        unsafe {
            let status =
                kcom::AsyncOperationRaw::<u32>::get_status_raw(op.as_ptr()).expect("get status");
            assert_eq!(status, AsyncStatus::Completed);
            let result =
                kcom::AsyncOperationRaw::<u32>::get_result_raw(op.as_ptr()).expect("get result");
            assert_eq!(result, 7u32);
        }
    }

    #[cfg(not(feature = "driver"))]
    #[test]
    fn spawn_dpc_task_cancellable_drops_pending_future_on_host() {
        use core::future::Future;
        use core::pin::Pin;
        use core::sync::atomic::{AtomicUsize, Ordering};
        use core::task::{Context, Poll};

        static DROPS: AtomicUsize = AtomicUsize::new(0);

        struct DropProbe;

        impl Drop for DropProbe {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        struct PendingWithDrop {
            _probe: DropProbe,
        }

        impl Future for PendingWithDrop {
            type Output = kcom::NTSTATUS;

            fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
                Poll::Pending
            }
        }

        DROPS.store(0, Ordering::Relaxed);
        let handle = unsafe {
            kcom::spawn_dpc_task_cancellable(PendingWithDrop { _probe: DropProbe })
        }
        .expect("spawn_dpc_task_cancellable");
        handle.cancel();
        drop(handle);

        assert_eq!(DROPS.load(Ordering::Relaxed), 1);
    }
}
