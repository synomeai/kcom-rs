use core::ffi::c_void;
use core::mem;
use kcom::*;
use kcom::vtable::ComInterfaceInfo; // IID取得のために必要


declare_com_interface! {
    /// プライマリとして使用するインターフェース
    pub trait ISmartFoo: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1111_1111,
            data2: 0x0000,
            data3: 0x0000,
            data4: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
        };
        fn foo(&self) -> NTSTATUS;
    }
}