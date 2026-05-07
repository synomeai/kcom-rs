use core::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use kcom::{
    declare_com_interface, impl_com_interface, impl_com_interface_multiple, impl_com_object,
    ComObject, ComObjectN, ComRc, GUID, IUnknownVtbl, NTSTATUS, STATUS_SUCCESS,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static MULTI_DROP_COUNT: AtomicU32 = AtomicU32::new(0);

declare_com_interface! {
    pub trait IFoo: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1122_3344,
            data2: 0x5566,
            data3: 0x7788,
            data4: [0x90, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67],
        };

        fn ping(&self) -> NTSTATUS;
    }
}

struct Foo;

impl IFoo for Foo {
    fn ping(&self) -> NTSTATUS {
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

#[test]
fn basic_vtable_call_works() {
    let _guard = TEST_LOCK.lock().unwrap();
    let ptr = Foo::new_com(Foo).unwrap();

    unsafe {
        let vtbl = *(ptr as *mut *mut IFooVtbl);
        let status = ((*vtbl).ping)(ptr as *mut core::ffi::c_void);
        assert_eq!(status, STATUS_SUCCESS);
        let _ = ComObject::<Foo, IFooVtbl>::shim_release(ptr);
    }
}

#[test]
fn basic_query_interface_roundtrip() {
    let _guard = TEST_LOCK.lock().unwrap();
    let ptr = Foo::new_com(Foo).unwrap();

    let com = unsafe { ComRc::<IFooRaw>::from_raw_addref(ptr as *mut IFooRaw).unwrap() };
    let queried = com.query_interface::<IFooRaw>().expect("query_interface");
    drop(queried);
    drop(com);

    unsafe {
        let _ = ComObject::<Foo, IFooVtbl>::shim_release(ptr);
    }
}

declare_com_interface! {
    pub trait IPrimary: IUnknown {
        const IID: GUID = GUID {
            data1: 0xDEAD_BEEF,
            data2: 0xCAFE,
            data3: 0xBABE,
            data4: [0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE],
        };

        fn primary(&self) -> NTSTATUS;
    }
}

declare_com_interface! {
    pub trait ISecondary: IUnknown {
        const IID: GUID = GUID {
            data1: 0xABCD_EF01,
            data2: 0x2345,
            data3: 0x6789,
            data4: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22],
        };

        fn secondary(&self) -> NTSTATUS;
    }
}

struct Multi;

impl Drop for Multi {
    fn drop(&mut self) {
        MULTI_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

impl IPrimary for Multi {
    fn primary(&self) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

impl ISecondary for Multi {
    fn secondary(&self) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

impl_com_interface! {
    impl Multi: IPrimary {
        parent = IUnknownVtbl,
        secondaries = (ISecondary),
        methods = [primary],
    }
}

impl_com_interface_multiple! {
    impl Multi: ISecondary {
        parent = IUnknownVtbl,
        primary = IPrimary,
        secondaries = (ISecondary),
        methods = [secondary],
    }
}

#[test]
fn basic_comobjectn_secondary_call_and_qi() {
    let _guard = TEST_LOCK.lock().unwrap();
    let ptr = ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::new(Multi).unwrap();

    unsafe {
        let vtbl = *(ptr as *mut *mut IPrimaryVtbl);
        let status = ((*vtbl).primary)(ptr as *mut core::ffi::c_void);
        assert_eq!(status, STATUS_SUCCESS);
    }

    let obj_ptr = ptr as *mut ComObjectN<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>;
    let sec_ptr = unsafe {
        ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::secondary_ptr::<ISecondaryVtbl, 0>(
            obj_ptr,
        )
    };
    unsafe {
        let vtbl = *(sec_ptr as *mut *mut ISecondaryVtbl);
        let status = ((*vtbl).secondary)(sec_ptr as *mut core::ffi::c_void);
        assert_eq!(status, STATUS_SUCCESS);
    }

    let mut out = core::ptr::null_mut();
    unsafe {
        let vtbl = *(ptr as *mut *mut IUnknownVtbl);
        let status = ((*vtbl).QueryInterface)(
            ptr as *mut core::ffi::c_void,
            &<ISecondaryRaw as kcom::vtable::ComInterfaceInfo>::IID,
            &mut out,
        );
        assert_eq!(status, STATUS_SUCCESS);
        assert!(!out.is_null());
        let vtbl = *(out as *mut *mut ISecondaryVtbl);
        let status = ((*vtbl).secondary)(out);
        assert_eq!(status, STATUS_SUCCESS);
        let _ = ((*vtbl).parent.Release)(out as *mut core::ffi::c_void);
    }

    unsafe {
        let _ = ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::shim_release(ptr);
    }
}

#[test]
fn basic_secondary_query_interface_roundtrip() {
    let _guard = TEST_LOCK.lock().unwrap();
    let ptr = ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::new(Multi).unwrap();
    let obj_ptr = ptr as *mut ComObjectN<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>;
    let sec_ptr = unsafe {
        ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::secondary_ptr::<ISecondaryVtbl, 0>(
            obj_ptr,
        )
    };

    let mut out = core::ptr::null_mut();
    unsafe {
        let vtbl = *(sec_ptr as *mut *mut ISecondaryVtbl);
        let status = ((*vtbl).parent.QueryInterface)(
            sec_ptr as *mut core::ffi::c_void,
            &<IPrimaryRaw as kcom::vtable::ComInterfaceInfo>::IID,
            &mut out,
        );
        assert_eq!(status, STATUS_SUCCESS);
        assert!(!out.is_null());

        let primary_vtbl = *(out as *mut *mut IPrimaryVtbl);
        let status = ((*primary_vtbl).primary)(out as *mut core::ffi::c_void);
        assert_eq!(status, STATUS_SUCCESS);
        let _ = ((*primary_vtbl).parent.Release)(out as *mut core::ffi::c_void);
    }

    unsafe {
        let _ = ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::shim_release(ptr);
    }
}

#[test]
fn basic_comrc_secondary_usage() {
    let _guard = TEST_LOCK.lock().unwrap();
    let primary =
        ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::new_rc::<IPrimaryRaw>(Multi)
            .unwrap();

    let secondary = primary.query_interface::<ISecondaryRaw>().expect("query secondary");
    unsafe {
        let vtbl = *(secondary.as_ptr() as *mut *mut ISecondaryVtbl);
        let status = ((*vtbl).secondary)(secondary.as_ptr() as *mut core::ffi::c_void);
        assert_eq!(status, STATUS_SUCCESS);
    }

    let primary_again = secondary
        .query_interface::<IPrimaryRaw>()
        .expect("query primary");
    drop(primary_again);
    drop(secondary);
    drop(primary);
}

#[test]
fn basic_comrc_drop_after_all_refs() {
    let _guard = TEST_LOCK.lock().unwrap();
    MULTI_DROP_COUNT.store(0, Ordering::Relaxed);

    let primary =
        ComObjectN::<Multi, IPrimaryVtbl, (ISecondaryVtbl,)>::new_rc::<IPrimaryRaw>(Multi)
            .unwrap();
    let secondary = primary.query_interface::<ISecondaryRaw>().expect("query secondary");
    let primary_clone = primary.clone();

    drop(primary);
    assert_eq!(MULTI_DROP_COUNT.load(Ordering::Relaxed), 0);
    drop(secondary);
    assert_eq!(MULTI_DROP_COUNT.load(Ordering::Relaxed), 0);
    drop(primary_clone);
    assert_eq!(MULTI_DROP_COUNT.load(Ordering::Relaxed), 1);
}

#[cfg(feature = "async-com")]
mod async_smoke {
    use super::*;
    use kcom::{pin_init, AsyncOperationRaw, AsyncStatus, GlobalAllocator, InitBox, InitBoxTrait};

    declare_com_interface! {
        pub trait IAsyncPing: IUnknown {
            const IID: GUID = GUID {
                data1: 0x1357_9BDF,
                data2: 0x2468,
                data3: 0xACE0,
                data4: [0x02, 0x13, 0x24, 0x35, 0x46, 0x57, 0x68, 0x79],
            };

            async fn ping_async(&self) -> NTSTATUS;
        }
    }

    struct AsyncFoo;

    unsafe impl IAsyncPing for AsyncFoo {
        type PingAsyncFuture = core::future::Ready<NTSTATUS>;
        type Allocator = GlobalAllocator;

        fn ping_async(&self) -> impl InitBoxTrait<Self::PingAsyncFuture, Self::Allocator, NTSTATUS> {
            InitBox::new(GlobalAllocator, pin_init!(core::future::ready(STATUS_SUCCESS)))
        }
    }

    impl_com_interface! {
        impl AsyncFoo: IAsyncPing {
            parent = IUnknownVtbl,
            methods = [ping_async],
        }
    }

    impl_com_object!(AsyncFoo, IAsyncPingVtbl);

    #[test]
    fn basic_async_interface_call() {
        let _guard = TEST_LOCK.lock().unwrap();
        let ptr = AsyncFoo::new_com(AsyncFoo).unwrap();

        unsafe {
            let vtbl = *(ptr as *mut *mut IAsyncPingVtbl);
            let op = ((*vtbl).ping_async)(ptr as *mut core::ffi::c_void);
            assert!(!op.is_null());

            let op = kcom::ComRc::<AsyncOperationRaw<NTSTATUS>>::from_raw_unchecked(op);
            let status = AsyncOperationRaw::<NTSTATUS>::get_status_raw(op.as_ptr())
                .expect("get status");
            assert_eq!(status, AsyncStatus::Completed);
            let result = AsyncOperationRaw::<NTSTATUS>::get_result_raw(op.as_ptr())
                .expect("get result");
            assert_eq!(result, STATUS_SUCCESS);
        }

        unsafe {
            let _ = ComObject::<AsyncFoo, IAsyncPingVtbl>::shim_release(ptr);
        }
    }
}
