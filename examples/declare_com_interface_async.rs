#[cfg(feature = "async-com")]
mod async_example {
    use core::future::Ready;

    use kcom::{
        declare_com_interface, impl_com_interface, impl_com_object, pin_init, AsyncOperationRaw,
        AsyncStatus, ComObject, ComRc, GlobalAllocator, GUID, IUnknownVtbl, InitBox, InitBoxTrait,
        NTSTATUS, STATUS_SUCCESS,
    };

    declare_com_interface! {
        /// Async COM interface example (kernel-only).
        pub trait IAsyncFoo: IUnknown {
            const IID: GUID = GUID {
                data1: 0xABCD_1234,
                data2: 0x5678,
                data3: 0x90AB,
                data4: [0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0x0A],
            };

            async fn init(&self, value: u32) -> NTSTATUS;
        }
    }

    struct AsyncFoo;

    unsafe impl IAsyncFoo for AsyncFoo {
        type InitFuture = Ready<NTSTATUS>;
        type Allocator = GlobalAllocator;

        fn init(&self, _value: u32) -> impl InitBoxTrait<Self::InitFuture, Self::Allocator, NTSTATUS> {
            InitBox::new(
                GlobalAllocator,
                pin_init!(core::future::ready(STATUS_SUCCESS)),
            )
        }
    }

    impl_com_interface! {
        impl AsyncFoo: IAsyncFoo {
            parent = IUnknownVtbl,
            methods = [init],
        }
    }

    impl_com_object!(AsyncFoo, IAsyncFooVtbl);

    pub fn run() {
        let raw = AsyncFoo::new_com(AsyncFoo).unwrap();

        unsafe {
            let vtbl = *(raw as *mut *const IAsyncFooVtbl);
            let op = ((*vtbl).init)(raw, 123);
            assert!(!op.is_null());

            let op = ComRc::<AsyncOperationRaw<NTSTATUS>>::from_raw(op).unwrap();
            let status = op.get_status().unwrap();
            assert_eq!(status, AsyncStatus::Completed);
            let result = op.get_result().unwrap();
            assert_eq!(result, STATUS_SUCCESS);

            ComObject::<AsyncFoo, IAsyncFooVtbl>::shim_release(raw);
        }
    }
}

#[cfg(feature = "async-com")]
fn main() {
    async_example::run();
}

#[cfg(not(feature = "async-com"))]
fn main() {}
#[cfg(feature = "driver")]
mod driver_entry_stub;
