#[cfg(all(feature = "driver", feature = "async-com-kernel"))]
mod async_operation_driver_spec {
    use core::future;

    use kcom::{spawn_async_operation_cancellable, AsyncStatus};

    #[test]
    #[ignore = "requires kernel driver execution environment"]
    fn cancellable_operation_transitions_to_canceled() {
        let (op, handle) =
            spawn_async_operation_cancellable(future::pending::<u32>()).expect("spawn operation");

        handle.cancel();

        let mut status = unsafe { op.get_status().expect("get status") };
        for _ in 0..1_000_000 {
            if status == AsyncStatus::Canceled {
                break;
            }
            status = unsafe { op.get_status().expect("get status") };
            core::hint::spin_loop();
        }

        assert_eq!(status, AsyncStatus::Canceled);

        let result = unsafe { op.get_result() };
        assert!(matches!(result, Err(kcom::iunknown::STATUS_CANCELLED)));
    }
}
