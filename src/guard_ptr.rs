use core::ffi::c_void;
use core::ptr::NonNull;

/// Pointer wrapper used by async guards with strict provenance semantics.
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct GuardPtr(NonNull<c_void>);

unsafe impl Send for GuardPtr {}
unsafe impl Sync for GuardPtr {}

impl GuardPtr {
    #[inline]
    pub fn new(ptr: *mut c_void) -> Self {
        Self(NonNull::new(ptr).expect("GuardPtr::new: ptr is null"))
    }

    #[inline]
    pub fn as_ptr(self) -> *mut c_void {
        self.0.as_ptr()
    }

    #[inline]
    pub fn try_new(ptr: *mut c_void) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
}
