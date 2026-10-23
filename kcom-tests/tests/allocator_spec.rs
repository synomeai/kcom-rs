use core::alloc::Layout;

use kcom::{Allocator, GlobalAllocator, KBox, PinInitOnce, NTSTATUS};

#[repr(align(64))]
struct OverAlignedZst;

#[test]
fn zero_size_alloc_respects_alignment() {
    for align in [1usize, 2, 8, 16, 64, 4096] {
        let layout = Layout::from_size_align(0, align).unwrap();
        unsafe {
            let ptr = GlobalAllocator.alloc(layout);
            assert!(!ptr.is_null());
            assert_eq!(ptr as usize % align, 0, "align {align}");
            GlobalAllocator.dealloc(ptr, layout);

            let ptr = GlobalAllocator.alloc_zeroed(layout);
            assert!(!ptr.is_null());
            assert_eq!(ptr as usize % align, 0, "align {align}");
            GlobalAllocator.dealloc(ptr, layout);
        }
    }
}

#[test]
fn kbox_of_overaligned_zst_is_aligned() {
    let boxed = KBox::<OverAlignedZst>::try_pin_init(
        GlobalAllocator,
        PinInitOnce::new(|ptr: *mut OverAlignedZst| {
            unsafe { ptr.write(OverAlignedZst) };
            Ok::<(), NTSTATUS>(())
        }),
    )
    .ok()
    .expect("zst kbox");
    let addr = &*boxed as *const OverAlignedZst as usize;
    assert_eq!(addr % core::mem::align_of::<OverAlignedZst>(), 0);
    drop(boxed);
}

#[test]
fn mdl_guard_into_raw_round_trips() {
    let fake = 0x1000usize as *mut core::ffi::c_void;
    let guard = unsafe { kcom::mdl::MdlGuard::from_raw(fake, kcom::mdl::MdlKind::NonPaged) }
        .expect("non-null mdl");
    assert_eq!(guard.into_raw(), fake);
    // Dropping a guard must free its MDL (a no-op stub on host builds).
    let guard = unsafe { kcom::mdl::MdlGuard::from_raw(fake, kcom::mdl::MdlKind::NonPaged) };
    drop(guard);
}
