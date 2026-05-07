use core::ffi::c_void;

use kcom::{
    declare_com_interface, impl_com_interface, impl_com_interface_multiple, GUID, IUnknownVtbl,
    NTSTATUS, STATUS_SUCCESS,
};
use kcom::wrapper::ComObjectN;

declare_com_interface! {
    /// Primary interface.
    pub trait IFoo: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1234_0001,
            data2: 0x5678,
            data3: 0x9ABC,
            data4: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
        };

        fn foo(&self, value: u32) -> NTSTATUS;
    }
}

declare_com_interface! {
    /// Secondary interface (different vtable offset).
    pub trait IBar: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1234_0002,
            data2: 0x5678,
            data3: 0x9ABC,
            data4: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
        };

        fn bar(&self, value: u32) -> NTSTATUS;
    }
}

struct Multi;

impl IFoo for Multi {
    fn foo(&self, _value: u32) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

impl IBar for Multi {
    fn bar(&self, _value: u32) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

impl_com_interface! {
    impl Multi: IFoo {
        parent = IUnknownVtbl,
        secondaries = (IBar),
        methods = [foo],
    }
}

impl_com_interface_multiple! {
    impl Multi: IBar {
        parent = IUnknownVtbl,
        primary = IFoo,
        index = 0,
        secondaries = (IBar),
        methods = [bar],
    }
}

fn main() {
    let raw = ComObjectN::<Multi, IFooVtbl, (IBarVtbl,)>::new(Multi).unwrap();
    let foo_ptr = raw as *mut IFooRaw;
    let obj_ptr = raw as *mut ComObjectN<Multi, IFooVtbl, (IBarVtbl,)>;
    let bar_ptr = unsafe {
        ComObjectN::<Multi, IFooVtbl, (IBarVtbl,)>::secondary_ptr::<IBarVtbl, 0>(obj_ptr)
    } as *mut IBarRaw;

    unsafe {
        let foo_vtbl = (*foo_ptr).lpVtbl;
        let bar_vtbl = (*bar_ptr).lpVtbl;
        assert_eq!(((*foo_vtbl).foo)(foo_ptr as *mut c_void, 1), STATUS_SUCCESS);
        assert_eq!(((*bar_vtbl).bar)(bar_ptr as *mut c_void, 2), STATUS_SUCCESS);

        ComObjectN::<Multi, IFooVtbl, (IBarVtbl,)>::shim_release(raw);
    }
}
#[cfg(feature = "driver")]
mod driver_entry_stub;
