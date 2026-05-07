#[cfg(feature = "async-com")]
mod async_operation_edge_spec {
    use kcom::{
        spawn_async_operation_error, AsyncOperationRaw, AsyncStatus,
    };
    use kcom::iunknown::STATUS_UNSUCCESSFUL;

    #[test]
    fn null_vtbl_returns_unsuccessful() {
        let raw = AsyncOperationRaw::<u32> {
            lpVtbl: core::ptr::null_mut(),
        };

        unsafe {
            assert_eq!(raw.get_status().unwrap_err(), STATUS_UNSUCCESSFUL);
            assert_eq!(raw.get_result().unwrap_err(), STATUS_UNSUCCESSFUL);
        }
    }

    #[test]
    fn spawn_error_reports_error_status() {
        let op = spawn_async_operation_error::<u32>(STATUS_UNSUCCESSFUL).expect("spawn error op");

        unsafe {
            let status = op.get_status().expect("get status");
            assert_eq!(status, AsyncStatus::Error);
            let result = op.get_result().unwrap_err();
            assert_eq!(result, STATUS_UNSUCCESSFUL);
        }
    }
}
