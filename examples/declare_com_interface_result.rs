use kcom::{
    declare_com_interface, impl_com_interface, impl_com_object, ComObject, GUID, IUnknownVtbl,
    NTSTATUS, STATUS_SUCCESS,
};

declare_com_interface! {
    /// Interface that returns Result, mapped to NTSTATUS in shims.
    pub trait IResultSample: IUnknown {
        const IID: GUID = GUID {
            data1: 0xDEAD_BEEF,
            data2: 0xFEED,
            data3: 0xBEEF,
            data4: [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF],
        };

        fn do_work(&self, ok: bool) -> Result<(), NTSTATUS>;
    }
}

struct Worker;

impl IResultSample for Worker {
    fn do_work(&self, ok: bool) -> Result<(), NTSTATUS> {
        if ok {
            Ok(())
        } else {
            Err(0xC000_0001u32 as NTSTATUS)
        }
    }
}

impl_com_interface! {
    impl Worker: IResultSample {
        parent = IUnknownVtbl,
        methods = [do_work],
    }
}

impl_com_object!(Worker, IResultSampleVtbl);

fn main() {
    let raw = Worker::new_com(Worker).unwrap();

    unsafe {
        let vtbl = *(raw as *mut *const IResultSampleVtbl);
        let ok_status = ((*vtbl).do_work)(raw, true);
        let err_status = ((*vtbl).do_work)(raw, false);

        assert_eq!(ok_status, STATUS_SUCCESS);
        assert_eq!(err_status, 0xC000_0001u32 as NTSTATUS);

        ComObject::<Worker, IResultSampleVtbl>::shim_release(raw);
    }
}
#[cfg(feature = "driver")]
mod driver_entry_stub;
