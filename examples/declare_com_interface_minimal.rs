use core::ffi::c_void;

use kcom::{
    declare_com_interface, impl_com_interface, impl_com_object, ComObject, GUID, IUnknownVtbl,
    NTSTATUS, STATUS_SUCCESS,
};

declare_com_interface! {
    /// Minimal COM interface example.
    pub trait IFoo: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1234_5678,
            data2: 0x1234,
            data3: 0x5678,
            data4: [0x90, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67],
        };

        fn ping(&self, value: u32) -> NTSTATUS;
    }
}

struct Foo;

impl IFoo for Foo {
    fn ping(&self, _value: u32) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

impl_com_interface! {
    impl Foo: IFoo {
        parent = IUnknownVtbl,
        methods = [ping],
    }
}

impl_com_object!(Foo, IFooVtbl);

fn main() {
    let raw = Foo::new_com(Foo).unwrap();
    let foo_ptr = raw as *mut IFooRaw;
    let com_ref = unsafe { IFooRaw::from_raw_addref(foo_ptr).unwrap() };

    unsafe {
        let vtbl = (*com_ref.as_ptr()).lpVtbl;
        let status = ((*vtbl).ping)(com_ref.as_ptr() as *mut c_void, 42);
        assert_eq!(status, STATUS_SUCCESS);
    }

    drop(com_ref);
    unsafe { ComObject::<Foo, IFooVtbl>::shim_release(raw) };
}
#[cfg(feature = "driver")]
mod driver_entry_stub;
