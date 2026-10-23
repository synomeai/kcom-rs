use core::alloc::Layout;
use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use kcom::{
    declare_com_interface, impl_com_interface, impl_com_object, Allocator, ComObject, ComRc,
    IUnknownVtbl, GUID, NTSTATUS, STATUS_NOINTERFACE, STATUS_SUCCESS,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Objects created with a custom allocator must be freed by that allocator with
// the layout they were allocated with, even though the vtable shims are
// monomorphized for `GlobalAllocator`.
// ---------------------------------------------------------------------------

static CUSTOM_ALLOCS: AtomicUsize = AtomicUsize::new(0);
static CUSTOM_DEALLOCS: AtomicUsize = AtomicUsize::new(0);
static CUSTOM_LAST_LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);

/// A non-zero-sized allocator, so `ComObject<T, I, Self>` has a different
/// layout from `ComObject<T, I, GlobalAllocator>`.
struct CountingAlloc {
    _state: [u64; 4],
}

impl CountingAlloc {
    fn new() -> Self {
        Self { _state: [0xA5; 4] }
    }
}

impl Allocator for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        CUSTOM_ALLOCS.fetch_add(1, Ordering::Relaxed);
        *CUSTOM_LAST_LAYOUT.lock().unwrap() = Some(layout);
        unsafe { std::alloc::alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CUSTOM_DEALLOCS.fetch_add(1, Ordering::Relaxed);
        assert_eq!(*CUSTOM_LAST_LAYOUT.lock().unwrap(), Some(layout));
        unsafe { std::alloc::dealloc(ptr, layout) }
    }
}

declare_com_interface! {
    pub trait IWidget: IUnknown {
        const IID: GUID = GUID {
            data1: 0x5A5A_0001,
            data2: 0x0001,
            data3: 0x0001,
            data4: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17],
        };

        fn value(&self) -> u32;
    }
}

static WIDGET_DROPS: AtomicU32 = AtomicU32::new(0);

struct Widget {
    value: u32,
}

impl Drop for Widget {
    fn drop(&mut self) {
        WIDGET_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

impl IWidget for Widget {
    fn value(&self) -> u32 {
        self.value
    }
}

impl_com_interface! {
    impl Widget: IWidget {
        parent = IUnknownVtbl,
        methods = [value],
    }
}

impl_com_object!(Widget, IWidgetVtbl);

#[test]
fn custom_allocator_frees_with_its_own_layout() {
    let _guard = TEST_LOCK.lock().unwrap();
    CUSTOM_ALLOCS.store(0, Ordering::Relaxed);
    CUSTOM_DEALLOCS.store(0, Ordering::Relaxed);
    WIDGET_DROPS.store(0, Ordering::Relaxed);

    let rc = Widget::new_com_rc_in::<_, IWidgetRaw>(Widget { value: 7 }, CountingAlloc::new())
        .expect("alloc");
    assert_eq!(CUSTOM_ALLOCS.load(Ordering::Relaxed), 1);
    unsafe {
        let vtbl = (*rc.as_ptr()).lpVtbl;
        assert_eq!(((*vtbl).value)(rc.as_ptr() as *mut c_void), 7);
    }
    let again = rc.query_interface::<IWidgetRaw>().expect("qi");
    drop(again);
    drop(rc);

    assert_eq!(WIDGET_DROPS.load(Ordering::Relaxed), 1);
    assert_eq!(CUSTOM_DEALLOCS.load(Ordering::Relaxed), 1);
}

#[test]
fn custom_allocator_aggregated_frees_with_its_own_layout() {
    let _guard = TEST_LOCK.lock().unwrap();
    CUSTOM_ALLOCS.store(0, Ordering::Relaxed);
    CUSTOM_DEALLOCS.store(0, Ordering::Relaxed);
    WIDGET_DROPS.store(0, Ordering::Relaxed);

    #[repr(C)]
    #[allow(non_snake_case)]
    struct Outer {
        lpVtbl: *const IUnknownVtbl,
    }
    unsafe extern "system" fn outer_qi(_: *mut c_void, _: *const GUID, ppv: *mut *mut c_void) -> NTSTATUS {
        unsafe { *ppv = core::ptr::null_mut() };
        STATUS_NOINTERFACE
    }
    unsafe extern "system" fn outer_count(_: *mut c_void) -> u32 {
        1
    }
    static OUTER_VTBL: IUnknownVtbl = IUnknownVtbl {
        QueryInterface: outer_qi,
        AddRef: outer_count,
        Release: outer_count,
    };
    let outer = Outer { lpVtbl: &OUTER_VTBL };

    let non_delegating = unsafe {
        Widget::new_com_aggregated_in(
            Widget { value: 1 },
            &outer as *const Outer as *mut c_void,
            CountingAlloc::new(),
        )
    }
    .expect("alloc");
    unsafe {
        let vtbl = *(non_delegating as *mut *mut IUnknownVtbl);
        assert_eq!(((*vtbl).Release)(non_delegating), 0);
    }
    assert_eq!(WIDGET_DROPS.load(Ordering::Relaxed), 1);
    assert_eq!(CUSTOM_DEALLOCS.load(Ordering::Relaxed), 1);
}

// ---------------------------------------------------------------------------
// Single-ComObject interface inheritance: QI must answer for the derived IID.
// ---------------------------------------------------------------------------

declare_com_interface! {
    pub trait IBase: IUnknown {
        const IID: GUID = GUID {
            data1: 0x5A5A_0002,
            data2: 0x0002,
            data3: 0x0002,
            data4: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27],
        };

        fn base(&self) -> u32;
    }
}

declare_com_interface! {
    pub trait IChild: IBase {
        const IID: GUID = GUID {
            data1: 0x5A5A_0003,
            data2: 0x0003,
            data3: 0x0003,
            data4: [0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37],
        };

        fn child(&self) -> u32;
    }
}

struct Derived;

impl IBase for Derived {
    fn base(&self) -> u32 {
        1
    }
}

impl IChild for Derived {
    fn child(&self) -> u32 {
        2
    }
}

impl_com_interface! {
    impl Derived: IBase {
        parent = IUnknownVtbl,
        methods = [base],
    }
}

impl_com_interface! {
    impl Derived: IChild {
        parent = IBaseVtbl,
        methods = [child],
    }
}

impl_com_object!(Derived, IChildVtbl);

#[test]
fn inherited_interface_query_interface_answers_for_derived_iid() {
    let _guard = TEST_LOCK.lock().unwrap();
    let child = Derived::new_com_rc::<IChildRaw>(Derived).expect("alloc");

    let again = child.query_interface::<IChildRaw>().expect("QI for the derived IID");
    let base = child.query_interface::<IBaseRaw>().expect("QI for the base IID");
    assert_eq!(again.as_ptr() as usize, child.as_ptr() as usize);
    assert_eq!(base.as_ptr() as usize, child.as_ptr() as usize);

    unsafe {
        let vtbl = (*child.as_ptr()).lpVtbl;
        let this = child.as_ptr() as *mut c_void;
        assert_eq!(((*vtbl).parent.base)(this), 1);
        assert_eq!(((*vtbl).child)(this), 2);

        // The raw IUnknown slots in the embedded base vtable must behave the same.
        let mut out = core::ptr::null_mut();
        let status = ((*vtbl).parent.parent.QueryInterface)(this, &<IChildRaw as kcom::ComInterfaceInfo>::IID, &mut out);
        assert_eq!(status, STATUS_SUCCESS);
        assert_eq!(out, this);
        ((*vtbl).parent.parent.Release)(this);
    }
}

// ---------------------------------------------------------------------------
// AddRef/Release from inside `T::drop` must not free the object a second time.
// ---------------------------------------------------------------------------

static SELF_PTR: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static TOUCHY_DROPS: AtomicU32 = AtomicU32::new(0);

struct TouchesSelfInDrop;

impl Drop for TouchesSelfInDrop {
    fn drop(&mut self) {
        TOUCHY_DROPS.fetch_add(1, Ordering::Relaxed);
        let this = SELF_PTR.swap(core::ptr::null_mut(), Ordering::Relaxed);
        if !this.is_null() {
            unsafe {
                ComObject::<TouchesSelfInDrop, IUnknownVtbl>::shim_add_ref(this);
                ComObject::<TouchesSelfInDrop, IUnknownVtbl>::shim_release(this);
            }
        }
    }
}

#[test]
fn balanced_addref_release_inside_drop_frees_once() {
    let _guard = TEST_LOCK.lock().unwrap();
    TOUCHY_DROPS.store(0, Ordering::Relaxed);

    let ptr = ComObject::<TouchesSelfInDrop, IUnknownVtbl>::new(TouchesSelfInDrop).unwrap();
    SELF_PTR.store(ptr, Ordering::Relaxed);
    unsafe {
        assert_eq!(ComObject::<TouchesSelfInDrop, IUnknownVtbl>::shim_release(ptr), 0);
    }
    assert_eq!(TOUCHY_DROPS.load(Ordering::Relaxed), 1);
}

// ---------------------------------------------------------------------------
// Generated `I*Raw` types must not be `Copy`/`Clone`: a copy has a different
// address than the COM object and calling through it passes a bogus `this`.
// ---------------------------------------------------------------------------

#[allow(dead_code)]
trait AmbiguousIfClone<A> {
    fn check() {}
}
impl<T: ?Sized> AmbiguousIfClone<()> for T {}
#[allow(dead_code)]
struct IsClone;
impl<T: ?Sized + Clone> AmbiguousIfClone<IsClone> for T {}

#[test]
fn generated_raw_types_are_not_clone() {
    // Fails to compile (ambiguous impl) if the Raw types implement Clone.
    <IWidgetRaw as AmbiguousIfClone<_>>::check();
    <IChildRaw as AmbiguousIfClone<_>>::check();
    let _ = ComRc::<IWidgetRaw>::as_ptr;
}
