// refcount.rs
//
// Shared refcount helpers with optional hardening.

use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "refcount-hardening")]
const MAX_REFCOUNT: u32 = i32::MAX as u32;

#[cfg(feature = "refcount-hardening")]
use crate::iunknown::STATUS_UNSUCCESSFUL;

#[cfg(feature = "refcount-hardening")]
#[cold]
#[inline(never)]
fn refcount_violation() -> ! {
    #[cfg(debug_assertions)]
    crate::trace::report_error(file!(), line!(), STATUS_UNSUCCESSFUL);

    #[cfg(all(
        feature = "driver",
        any(feature = "async-com-kernel", feature = "kernel-unicode"),
        not(miri)
    ))]
    unsafe {
        crate::ntddk::KeBugCheckEx(0x4B43_4F4D, 0, 0, 0, 0);
    }

    #[cfg(all(not(feature = "driver"), test))]
    {
        std::process::abort();
    }

    #[cfg(all(not(feature = "driver"), not(test)))]
    {
        loop {
            core::hint::spin_loop();
        }
    }

    #[cfg(all(feature = "driver", not(any(feature = "async-com-kernel", feature = "kernel-unicode"))))]
    {
        loop {
            core::hint::spin_loop();
        }
    }

    #[cfg(all(
        feature = "driver",
        any(feature = "async-com-kernel", feature = "kernel-unicode"),
        miri
    ))]
    {
        loop {
            core::hint::spin_loop();
        }
    }
}

#[cfg(not(feature = "refcount-hardening"))]
#[inline]
pub(crate) fn add(ref_count: &AtomicU32) -> u32 {
    ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

#[cfg(all(feature = "refcount-hardening", not(feature = "leaky-hardening")))]
#[inline]
pub(crate) fn add(ref_count: &AtomicU32) -> u32 {
    match ref_count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |curr| {
        if curr >= MAX_REFCOUNT {
            None
        } else {
            Some(curr + 1)
        }
    }) {
        Ok(prev) => prev + 1,
        Err(_) => refcount_violation(),
    }
}

#[cfg(all(feature = "refcount-hardening", feature = "leaky-hardening"))]
#[inline]
pub(crate) fn add(ref_count: &AtomicU32) -> u32 {
    match ref_count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |curr| {
        if curr >= MAX_REFCOUNT {
            None
        } else {
            Some(curr + 1)
        }
    }) {
        Ok(prev) => prev + 1,
        Err(_) => MAX_REFCOUNT,
    }
}

#[cfg(not(feature = "refcount-hardening"))]
#[inline]
pub(crate) fn sub(ref_count: &AtomicU32) -> u32 {
    ref_count.fetch_sub(1, Ordering::Release) - 1
}

#[cfg(all(feature = "refcount-hardening", not(feature = "leaky-hardening")))]
#[inline]
pub(crate) fn sub(ref_count: &AtomicU32) -> u32 {
    match ref_count.fetch_update(Ordering::Release, Ordering::Relaxed, |curr| {
        if curr == 0 {
            None
        } else {
            Some(curr - 1)
        }
    }) {
        Ok(prev) => prev - 1,
        Err(_) => refcount_violation(),
    }
}

/// Once `add` has saturated at `MAX_REFCOUNT`, some AddRefs were dropped, so
/// the count no longer tracks the real owners. Decrementing from there would
/// free the object while references remain; instead the object stays pinned
/// at the cap and is leaked.
#[cfg(all(feature = "refcount-hardening", feature = "leaky-hardening"))]
#[inline]
pub(crate) fn sub(ref_count: &AtomicU32) -> u32 {
    match ref_count.fetch_update(Ordering::Release, Ordering::Relaxed, |curr| {
        if curr == 0 || curr >= MAX_REFCOUNT {
            None
        } else {
            Some(curr - 1)
        }
    }) {
        Ok(prev) => prev - 1,
        Err(_) => MAX_REFCOUNT,
    }
}

#[cfg(all(test, feature = "leaky-hardening"))]
mod tests {
    use super::*;

    #[test]
    fn saturated_count_is_not_decremented() {
        let count = AtomicU32::new(MAX_REFCOUNT - 1);
        assert_eq!(add(&count), MAX_REFCOUNT);
        // Saturated: this AddRef is dropped.
        assert_eq!(add(&count), MAX_REFCOUNT);
        // Releases must not walk the count back toward zero.
        assert_eq!(sub(&count), MAX_REFCOUNT);
        assert_eq!(count.load(Ordering::Relaxed), MAX_REFCOUNT);
    }

    #[test]
    fn underflow_is_ignored() {
        let count = AtomicU32::new(0);
        assert_eq!(sub(&count), MAX_REFCOUNT);
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }
}
