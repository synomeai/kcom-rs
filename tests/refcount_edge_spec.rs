use core::alloc::Layout;
use core::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use kcom::{ComInterface, ComObject, ComRc, GlobalAllocator, IUnknownVtbl};

#[repr(C)]
#[allow(non_snake_case)]
struct IUnknownRaw {
    lpVtbl: *mut IUnknownVtbl,
}

unsafe impl ComInterface for IUnknownRaw {}

static DROP_COUNT: AtomicU32 = AtomicU32::new(0);
static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Dummy;

impl Drop for Dummy {
    fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn comrc_many_clones_drop_once() {
    let _guard = TEST_LOCK.lock().unwrap();
    DROP_COUNT.store(0, Ordering::Relaxed);
    let raw = ComObject::<Dummy, IUnknownVtbl>::new(Dummy).unwrap();

    let com = unsafe { ComRc::<IUnknownRaw>::from_raw_addref(raw as *mut IUnknownRaw).unwrap() };
    let clones: Vec<_> = (0..8).map(|_| com.clone()).collect();
    drop(com);
    drop(clones);

    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 0);

    unsafe {
        assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(raw), 0);
    }

    assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
}

#[test]
fn try_new_in_with_layout_rejects_mismatch() {
    let _guard = TEST_LOCK.lock().unwrap();
    let layout = Layout::new::<u8>();
    let ptr = ComObject::<Dummy, IUnknownVtbl>::try_new_in_with_layout(
        Dummy,
        GlobalAllocator,
        layout,
    );
    assert!(ptr.is_none());
}
