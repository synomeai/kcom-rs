use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use kcom::{ComObject, GUID, IUnknownVtbl, NTSTATUS, STATUS_SUCCESS};

static DROP_COUNT: AtomicU32 = AtomicU32::new(0);
static OUTER_ADDREF_COUNT: AtomicU32 = AtomicU32::new(0);
static OUTER_RELEASE_COUNT: AtomicU32 = AtomicU32::new(0);
static OUTER_QUERY_COUNT: AtomicU32 = AtomicU32::new(0);
static OUTER_PTR: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn outer_add_ref(this: *mut c_void) -> u32 {
    assert_eq!(this as usize, OUTER_PTR.load(Ordering::Relaxed));
    OUTER_ADDREF_COUNT.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn outer_release(this: *mut c_void) -> u32 {
    assert_eq!(this as usize, OUTER_PTR.load(Ordering::Relaxed));
    OUTER_RELEASE_COUNT.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn outer_query_interface(
    this: *mut c_void,
    _riid: *const GUID,
    ppv: *mut *mut c_void,
) -> NTSTATUS {
    assert_eq!(this as usize, OUTER_PTR.load(Ordering::Relaxed));
    OUTER_QUERY_COUNT.fetch_add(1, Ordering::Relaxed);
    if !ppv.is_null() {
        unsafe { *ppv = this };
    }
    STATUS_SUCCESS
}

static OUTER_VTBL: IUnknownVtbl = IUnknownVtbl {
    QueryInterface: outer_query_interface,
    AddRef: outer_add_ref,
    Release: outer_release,
};

#[repr(C)]
#[allow(non_snake_case)]
struct OuterUnknown {
    lpVtbl: *const IUnknownVtbl,
}

struct Dummy;

impl Drop for Dummy {
    fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn aggregated_outer_unknown_delegates_iunknown_calls() {
    DROP_COUNT.store(0, Ordering::Relaxed);
    OUTER_ADDREF_COUNT.store(0, Ordering::Relaxed);
    OUTER_RELEASE_COUNT.store(0, Ordering::Relaxed);
    OUTER_QUERY_COUNT.store(0, Ordering::Relaxed);

    let outer = OuterUnknown {
        lpVtbl: &OUTER_VTBL as *const _,
    };
    OUTER_PTR.store(&outer as *const _ as usize, Ordering::Relaxed);

    let non_delegating = unsafe {
        ComObject::<Dummy, IUnknownVtbl>::new_aggregated(
            Dummy,
            &outer as *const _ as *mut c_void,
        )
    }
    .unwrap();

    let delegating =
        unsafe { ComObject::<Dummy, IUnknownVtbl>::from_non_delegating(non_delegating) }
            as *const _ as *mut c_void;

    unsafe {
        let vtbl = *(delegating as *mut *mut IUnknownVtbl);
        assert_eq!(((*vtbl).AddRef)(delegating), 1);
        assert_eq!(((*vtbl).Release)(delegating), 1);
    }

    assert_eq!(OUTER_ADDREF_COUNT.load(Ordering::Relaxed), 1);
    assert_eq!(OUTER_RELEASE_COUNT.load(Ordering::Relaxed), 1);

    let mut out = core::ptr::null_mut();
    unsafe {
        let vtbl = *(delegating as *mut *mut IUnknownVtbl);
        assert_eq!(
            ((*vtbl).QueryInterface)(delegating, &kcom::IID_IUNKNOWN, &mut out),
            STATUS_SUCCESS
        );
    }
    assert_eq!(OUTER_QUERY_COUNT.load(Ordering::Relaxed), 1);
    assert_eq!(out as usize, OUTER_PTR.load(Ordering::Relaxed));

    unsafe {
        let vtbl = *(non_delegating as *mut *mut IUnknownVtbl);
        assert_eq!(((*vtbl).Release)(non_delegating), 0);
    }

    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    assert_eq!(OUTER_QUERY_COUNT.load(Ordering::Relaxed), 1);
}
