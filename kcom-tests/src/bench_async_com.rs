use core::future::Future;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::task::{Context, Poll};

use kcom::ntddk::{KeQueryPerformanceCounter, LARGE_INTEGER};
use kcom::{
    reset_async_com_metrics, snapshot_async_com_metrics, spawn_async_operation, NTSTATUS,
    STATUS_INVALID_PARAMETER, STATUS_SUCCESS,
};

#[cfg(any(
    all(feature = "bench-scenario-immediate", feature = "bench-scenario-yield1"),
    all(feature = "bench-scenario-immediate", feature = "bench-scenario-yieldN"),
    all(feature = "bench-scenario-yield1", feature = "bench-scenario-yieldN"),
))]
compile_error!("bench scenario features are mutually exclusive");

#[cfg(not(any(
    feature = "bench-scenario-immediate",
    feature = "bench-scenario-yield1",
    feature = "bench-scenario-yieldN"
)))]
compile_error!("select one bench scenario feature: bench-scenario-immediate | bench-scenario-yield1 | bench-scenario-yieldN");

#[cfg(any(
    all(feature = "bench-iter-small", feature = "bench-iter-medium"),
    all(feature = "bench-iter-small", feature = "bench-iter-large"),
    all(feature = "bench-iter-medium", feature = "bench-iter-large"),
))]
compile_error!("bench iteration presets are mutually exclusive");

#[cfg(any(
    all(feature = "bench-par-1", feature = "bench-par-4"),
    all(feature = "bench-par-1", feature = "bench-par-16"),
    all(feature = "bench-par-4", feature = "bench-par-16"),
))]
compile_error!("bench parallelism presets are mutually exclusive");

#[cfg(feature = "bench-iter-small")]
const ITER_SMALL: usize = 10_000;
const ITER_MEDIUM: usize = 100_000;
#[cfg(feature = "bench-iter-large")]
const ITER_LARGE: usize = 1_000_000;

#[cfg(feature = "bench-par-1")]
const PAR_1: usize = 1;
const PAR_4: usize = 4;
#[cfg(feature = "bench-par-16")]
const PAR_16: usize = 16;

const DEFAULT_YIELD_COUNT: u32 = 8;

const ENV_ITERS: Option<&'static str> = option_env!("KCOM_BENCH_ITERS");
const ENV_PAR: Option<&'static str> = option_env!("KCOM_BENCH_PAR");
const ENV_YIELDS: Option<&'static str> = option_env!("KCOM_BENCH_YIELDS");

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BenchScenario {
    Immediate,
    YieldOnce,
    YieldMany,
}

#[derive(Clone, Copy, Debug)]
pub struct BenchConfig {
    pub scenario: BenchScenario,
    pub iterations: usize,
    pub parallelism: usize,
    pub yield_count: u32,
    pub warmup_iterations: usize,
}

impl BenchConfig {
    #[inline]
    pub fn selected() -> Self {
        Self {
            scenario: selected_scenario(),
            iterations: selected_iterations(),
            parallelism: selected_parallelism(),
            yield_count: selected_yield_count(),
            warmup_iterations: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BenchResult {
    pub iterations: usize,
    pub parallelism: usize,
    pub scenario: BenchScenario,
    pub yield_count: u32,
    pub qpc_freq: u64,
    pub elapsed_ticks: u64,
    pub avg_latency_ticks: u64,
    pub min_latency_ticks: u64,
    pub max_latency_ticks: u64,
    pub dpc_enqueued: u64,
    pub dpc_skipped: u64,
    pub dpc_run: u64,
    pub poll_total: u64,
    pub poll_ready: u64,
    pub poll_pending: u64,
    pub slab_hit: u64,
    pub slab_miss: u64,
}

impl BenchResult {
    #[inline]
    pub fn elapsed_ns(self) -> u64 {
        ticks_to_ns(self.elapsed_ticks, self.qpc_freq)
    }

    #[inline]
    pub fn avg_latency_ns(self) -> u64 {
        ticks_to_ns(self.avg_latency_ticks, self.qpc_freq)
    }

    #[inline]
    pub fn min_latency_ns(self) -> u64 {
        ticks_to_ns(self.min_latency_ticks, self.qpc_freq)
    }

    #[inline]
    pub fn max_latency_ns(self) -> u64 {
        ticks_to_ns(self.max_latency_ticks, self.qpc_freq)
    }

    #[inline]
    pub fn throughput_per_sec(self) -> u64 {
        let elapsed_ns = self.elapsed_ns();
        if elapsed_ns == 0 {
            return 0;
        }
        let total = self.iterations as u128;
        let per_sec = total.saturating_mul(1_000_000_000) / (elapsed_ns as u128);
        per_sec as u64
    }
}

struct BenchCounters {
    completed: AtomicUsize,
    sum_ticks: AtomicU64,
    min_ticks: AtomicU64,
    max_ticks: AtomicU64,
}

impl BenchCounters {
    #[inline]
    const fn new() -> Self {
        Self {
            completed: AtomicUsize::new(0),
            sum_ticks: AtomicU64::new(0),
            min_ticks: AtomicU64::new(u64::MAX),
            max_ticks: AtomicU64::new(0),
        }
    }

    #[inline]
    fn record_latency(&self, latency: u64) {
        self.sum_ticks.fetch_add(latency, Ordering::Relaxed);
        update_min(&self.min_ticks, latency);
        update_max(&self.max_ticks, latency);
        self.completed.fetch_add(1, Ordering::Release);
    }
}

struct BenchFuture {
    remaining: u32,
    done: bool,
    start_ticks: u64,
    counters: *const BenchCounters,
}

// BenchFuture only holds a stable pointer to shared atomics that outlive
// the tasks for the duration of the benchmark loop.
unsafe impl Send for BenchFuture {}

impl BenchFuture {
    #[inline]
    fn new(start_ticks: u64, scenario: BenchScenario, yield_count: u32, counters: &BenchCounters) -> Self {
        let remaining = match scenario {
            BenchScenario::Immediate => 0,
            BenchScenario::YieldOnce => 1,
            BenchScenario::YieldMany => yield_count.max(1),
        };
        Self {
            remaining,
            done: false,
            start_ticks,
            counters,
        }
    }
}

impl Future for BenchFuture {
    type Output = NTSTATUS;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.done {
            return Poll::Ready(STATUS_SUCCESS);
        }

        if self.remaining > 0 {
            self.remaining -= 1;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }

        let end_ticks = qpc_now();
        unsafe {
            (*self.counters).record_latency(end_ticks.saturating_sub(self.start_ticks));
        }
        self.done = true;
        Poll::Ready(STATUS_SUCCESS)
    }
}

#[inline]
pub unsafe fn run_selected_bench() -> Result<BenchResult, NTSTATUS> {
    unsafe { run_async_com_bench(BenchConfig::selected()) }
}

/// # Safety
/// Must be called at PASSIVE_LEVEL in a kernel execution environment.
#[inline]
pub unsafe fn run_async_com_bench(config: BenchConfig) -> Result<BenchResult, NTSTATUS> {
    if config.iterations == 0 || config.parallelism == 0 {
        return Err(STATUS_INVALID_PARAMETER);
    }

    #[cfg(feature = "async-com-fused")]
    unsafe {
        kcom::init_async_com_slabs();
    }

    if config.warmup_iterations > 0 {
        let warmup = BenchCounters::new();
        run_bench_loop(
            config.warmup_iterations,
            config.parallelism,
            config.scenario,
            config.yield_count,
            &warmup,
        )?;
    }

    reset_async_com_metrics();
    let counters = BenchCounters::new();
    let qpc_freq = qpc_freq();
    let bench_start = qpc_now();

    run_bench_loop(
        config.iterations,
        config.parallelism,
        config.scenario,
        config.yield_count,
        &counters,
    )?;

    let bench_end = qpc_now();
    let elapsed_ticks = bench_end.saturating_sub(bench_start);
    let sum_ticks = counters.sum_ticks.load(Ordering::Relaxed);
    let min_ticks = counters.min_ticks.load(Ordering::Relaxed);
    let max_ticks = counters.max_ticks.load(Ordering::Relaxed);

    let avg_latency_ticks = if config.iterations == 0 {
        0
    } else {
        sum_ticks / (config.iterations as u64)
    };
    let min_latency_ticks = if min_ticks == u64::MAX { 0 } else { min_ticks };
    let metrics = snapshot_async_com_metrics();

    Ok(BenchResult {
        iterations: config.iterations,
        parallelism: config.parallelism,
        scenario: config.scenario,
        yield_count: config.yield_count,
        qpc_freq,
        elapsed_ticks,
        avg_latency_ticks,
        min_latency_ticks,
        max_latency_ticks: max_ticks,
        dpc_enqueued: metrics.dpc_enqueued,
        dpc_skipped: metrics.dpc_skipped,
        dpc_run: metrics.dpc_run,
        poll_total: metrics.poll_total,
        poll_ready: metrics.poll_ready,
        poll_pending: metrics.poll_pending,
        slab_hit: metrics.slab_hit,
        slab_miss: metrics.slab_miss,
    })
}

#[inline]
fn run_bench_loop(
    iterations: usize,
    parallelism: usize,
    scenario: BenchScenario,
    yield_count: u32,
    counters: &BenchCounters,
) -> Result<(), NTSTATUS> {
    let total = iterations;
    let mut started = 0usize;

    while started < total {
        let completed = counters.completed.load(Ordering::Acquire);
        let in_flight = started.saturating_sub(completed);
        if in_flight < parallelism {
            let future = BenchFuture::new(qpc_now(), scenario, yield_count, counters);
            let op = spawn_async_operation(future)?;
            core::mem::drop(op);
            started += 1;
        } else {
            core::hint::spin_loop();
        }
    }

    while counters.completed.load(Ordering::Acquire) < total {
        core::hint::spin_loop();
    }

    Ok(())
}

#[inline]
fn update_min(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value < current {
        match target.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}

#[inline]
fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}

#[inline]
fn ticks_to_ns(ticks: u64, freq: u64) -> u64 {
    if freq == 0 {
        return 0;
    }
    let ns = (ticks as u128).saturating_mul(1_000_000_000u128) / (freq as u128);
    ns as u64
}

#[inline]
fn qpc_now() -> u64 {
    unsafe { KeQueryPerformanceCounter(core::ptr::null_mut()).QuadPart as u64 }
}

#[inline]
fn qpc_freq() -> u64 {
    let mut freq = MaybeUninit::<LARGE_INTEGER>::uninit();
    unsafe {
        KeQueryPerformanceCounter(freq.as_mut_ptr());
        freq.assume_init().QuadPart as u64
    }
}

#[cfg(feature = "bench-scenario-immediate")]
#[inline]
fn selected_scenario() -> BenchScenario {
    BenchScenario::Immediate
}

#[cfg(feature = "bench-scenario-yield1")]
#[inline]
fn selected_scenario() -> BenchScenario {
    BenchScenario::YieldOnce
}

#[cfg(feature = "bench-scenario-yieldN")]
#[inline]
fn selected_scenario() -> BenchScenario {
    BenchScenario::YieldMany
}

#[inline]
fn selected_iterations() -> usize {
    if let Some(value) = parse_env_usize(ENV_ITERS) {
        return value;
    }
    selected_iterations_preset()
}

#[cfg(feature = "bench-iter-small")]
#[inline]
fn selected_iterations_preset() -> usize {
    ITER_SMALL
}

#[cfg(feature = "bench-iter-medium")]
#[inline]
fn selected_iterations_preset() -> usize {
    ITER_MEDIUM
}

#[cfg(feature = "bench-iter-large")]
#[inline]
fn selected_iterations_preset() -> usize {
    ITER_LARGE
}

#[cfg(not(any(
    feature = "bench-iter-small",
    feature = "bench-iter-medium",
    feature = "bench-iter-large"
)))]
#[inline]
fn selected_iterations_preset() -> usize {
    ITER_MEDIUM
}

#[inline]
fn selected_parallelism() -> usize {
    if let Some(value) = parse_env_usize(ENV_PAR) {
        return value;
    }
    selected_parallelism_preset()
}

#[cfg(feature = "bench-par-1")]
#[inline]
fn selected_parallelism_preset() -> usize {
    PAR_1
}

#[cfg(feature = "bench-par-4")]
#[inline]
fn selected_parallelism_preset() -> usize {
    PAR_4
}

#[cfg(feature = "bench-par-16")]
#[inline]
fn selected_parallelism_preset() -> usize {
    PAR_16
}

#[cfg(not(any(feature = "bench-par-1", feature = "bench-par-4", feature = "bench-par-16")))]
#[inline]
fn selected_parallelism_preset() -> usize {
    PAR_4
}

#[inline]
fn selected_yield_count() -> u32 {
    parse_env_u32(ENV_YIELDS).unwrap_or(DEFAULT_YIELD_COUNT).max(1)
}

#[inline]
fn parse_env_usize(value: Option<&'static str>) -> Option<usize> {
    value.and_then(|raw| raw.parse::<usize>().ok()).filter(|v| *v > 0)
}

#[inline]
fn parse_env_u32(value: Option<&'static str>) -> Option<u32> {
    value.and_then(|raw| raw.parse::<u32>().ok()).filter(|v| *v > 0)
}
