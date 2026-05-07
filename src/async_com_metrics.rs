// async_com_metrics.rs
//
// Lightweight counters for async COM execution paths (DPC/polling/slab).
// Intended for benchmarking/diagnostics; use relaxed atomics to minimize overhead.
//
// Build with `features = ["async-com-metrics"]` to enable counters.

#[cfg(feature = "async-com-metrics")]
mod imp {
    use core::sync::atomic::{AtomicU64, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct AsyncComMetrics {
        pub dpc_enqueued: u64,
        pub dpc_skipped: u64,
        pub dpc_run: u64,
        pub poll_total: u64,
        pub poll_ready: u64,
        pub poll_pending: u64,
        pub slab_hit: u64,
        pub slab_miss: u64,
    }

    static DPC_ENQUEUED: AtomicU64 = AtomicU64::new(0);
    static DPC_SKIPPED: AtomicU64 = AtomicU64::new(0);
    static DPC_RUN: AtomicU64 = AtomicU64::new(0);
    static POLL_TOTAL: AtomicU64 = AtomicU64::new(0);
    static POLL_READY: AtomicU64 = AtomicU64::new(0);
    static POLL_PENDING: AtomicU64 = AtomicU64::new(0);
    static SLAB_HIT: AtomicU64 = AtomicU64::new(0);
    static SLAB_MISS: AtomicU64 = AtomicU64::new(0);

    #[inline]
    pub fn reset_async_com_metrics() {
        DPC_ENQUEUED.store(0, Ordering::Relaxed);
        DPC_SKIPPED.store(0, Ordering::Relaxed);
        DPC_RUN.store(0, Ordering::Relaxed);
        POLL_TOTAL.store(0, Ordering::Relaxed);
        POLL_READY.store(0, Ordering::Relaxed);
        POLL_PENDING.store(0, Ordering::Relaxed);
        SLAB_HIT.store(0, Ordering::Relaxed);
        SLAB_MISS.store(0, Ordering::Relaxed);
    }

    #[inline]
    pub fn snapshot_async_com_metrics() -> AsyncComMetrics {
        AsyncComMetrics {
            dpc_enqueued: DPC_ENQUEUED.load(Ordering::Relaxed),
            dpc_skipped: DPC_SKIPPED.load(Ordering::Relaxed),
            dpc_run: DPC_RUN.load(Ordering::Relaxed),
            poll_total: POLL_TOTAL.load(Ordering::Relaxed),
            poll_ready: POLL_READY.load(Ordering::Relaxed),
            poll_pending: POLL_PENDING.load(Ordering::Relaxed),
            slab_hit: SLAB_HIT.load(Ordering::Relaxed),
            slab_miss: SLAB_MISS.load(Ordering::Relaxed),
        }
    }

    #[inline]
    pub(crate) fn inc_dpc_enqueued() {
        DPC_ENQUEUED.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_dpc_skipped() {
        DPC_SKIPPED.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_dpc_run() {
        DPC_RUN.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_poll_total() {
        POLL_TOTAL.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_poll_ready() {
        POLL_READY.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_poll_pending() {
        POLL_PENDING.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_slab_hit() {
        SLAB_HIT.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn inc_slab_miss() {
        SLAB_MISS.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(not(feature = "async-com-metrics"))]
mod imp {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct AsyncComMetrics {
        pub dpc_enqueued: u64,
        pub dpc_skipped: u64,
        pub dpc_run: u64,
        pub poll_total: u64,
        pub poll_ready: u64,
        pub poll_pending: u64,
        pub slab_hit: u64,
        pub slab_miss: u64,
    }

    #[inline]
    pub fn reset_async_com_metrics() {}

    #[inline]
    pub fn snapshot_async_com_metrics() -> AsyncComMetrics {
        AsyncComMetrics::default()
    }

    #[inline]
    pub(crate) fn inc_dpc_enqueued() {}

    #[inline]
    pub(crate) fn inc_dpc_skipped() {}

    #[inline]
    pub(crate) fn inc_dpc_run() {}

    #[inline]
    pub(crate) fn inc_poll_total() {}

    #[inline]
    pub(crate) fn inc_poll_ready() {}

    #[inline]
    pub(crate) fn inc_poll_pending() {}

    #[inline]
    pub(crate) fn inc_slab_hit() {}

    #[inline]
    pub(crate) fn inc_slab_miss() {}
}

pub use imp::*;
