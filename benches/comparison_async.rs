#[cfg(feature = "async-com")]
use kcom::{
    declare_com_interface, impl_com_interface, impl_com_object, pin_init, AsyncOperationRaw,
    AsyncStatus, ComObject, ComRc, GlobalAllocator, GUID, IUnknownVtbl, InitBox, InitBoxTrait,
    NTSTATUS,
};
#[cfg(feature = "async-com")]
use std::hint::black_box;
#[cfg(feature = "async-com")]
use std::sync::Arc;
#[cfg(feature = "async-com")]
use std::sync::atomic::{compiler_fence, Ordering};
#[cfg(feature = "async-com")]
use std::time::Instant;

// =========================================================
// 1. Interface Definition (Async)
// =========================================================

#[cfg(feature = "async-com")]
declare_com_interface! {
    pub trait IMyAsyncOp: IUnknown {
        const IID: GUID = GUID {
            data1: 0x2222_3333, data2: 0x4444, data3: 0x5555,
            data4: [0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD],
        };
        async fn get_status(&self) -> i32;
    }
}

// =========================================================
// 2. kcom Implementation (Async)
// =========================================================

#[cfg(feature = "async-com")]
struct MyAsyncImpl;

#[cfg(feature = "async-com")]
unsafe impl IMyAsyncOp for MyAsyncImpl {
    type GetStatusFuture = core::future::Ready<i32>;
    type Allocator = GlobalAllocator;

    #[inline(never)]
    fn get_status(&self) -> impl InitBoxTrait<Self::GetStatusFuture, Self::Allocator, NTSTATUS> {
        InitBox::new(GlobalAllocator, pin_init!(core::future::ready(1)))
    }
}

#[cfg(feature = "async-com")]
impl_com_interface! {
    impl MyAsyncImpl: IMyAsyncOp {
        parent = IUnknownVtbl,
        methods = [get_status],
    }
}

#[cfg(feature = "async-com")]
impl_com_object!(MyAsyncImpl, IMyAsyncOpVtbl);

// =========================================================
// 3. Standard Rust Implementation (Baseline)
// =========================================================

#[cfg(feature = "async-com")]
struct ModernImpl;

#[cfg(feature = "async-com")]
impl ModernImpl {
    #[inline(never)]
    fn get_status(&self) -> i32 {
        1
    }
}

// =========================================================
// Benchmarking Utilities
// - Warmup + compiler_fence to reduce measurement skew from optimizations.
// =========================================================

#[cfg(feature = "async-com")]
const WARMUP_ITERATIONS: u64 = 100_000;

#[cfg(feature = "async-com")]
static mut BASELINE_SINK: u64 = 0;

#[cfg(feature = "async-com")]
#[inline(never)]
fn touch_baseline() -> u64 {
    unsafe {
        let value = core::ptr::read_volatile(core::ptr::addr_of!(BASELINE_SINK));
        let next = value.wrapping_add(1);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_SINK), next);
        next
    }
}

#[cfg(feature = "async-com")]
#[inline(never)]
fn touch_native(value: i32) {
    unsafe {
        let current = core::ptr::read_volatile(core::ptr::addr_of!(BASELINE_SINK));
        let next = current.wrapping_add(value as u64);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_SINK), next);
    }
}

#[cfg(feature = "async-com")]
fn measure_ns_raw<F>(iterations: u64, mut func: F) -> f64
where
    F: FnMut(),
{
    for _ in 0..WARMUP_ITERATIONS {
        func();
        compiler_fence(Ordering::SeqCst);
    }
    let start = Instant::now();
    for _ in 0..iterations {
        func();
        compiler_fence(Ordering::SeqCst);
    }
    let duration = start.elapsed();
    let total_ns = duration.as_nanos() as f64;
    total_ns / iterations as f64
}

#[cfg(feature = "async-com")]
fn measure_ns<F>(name: &str, iterations: u64, baseline: f64, mut func: F) -> f64
where
    F: FnMut(),
{
    let avg = measure_ns_raw(iterations, &mut func);
    let adj = if avg > baseline { avg - baseline } else { 0.0 };
    println!("[{}] Average: {:.5} ns (adj {:.5} ns)", name, avg, adj);
    adj
}

#[cfg(feature = "async-com")]
fn main() {
    const ITERATIONS: u64 = 10_000_000; // 10M loops

    println!("Running Rust (kcom) Async Benchmarks ({} iterations)...", ITERATIONS);
    println!("-----------------------------------------------------");
    let baseline = measure_ns_raw(ITERATIONS, || {
        black_box(touch_baseline());
    });
    println!("[Rust_Empty_Loop] Average: {:.5} ns", baseline);

    // Prepare a COM object.
    let raw_obj = MyAsyncImpl::new_com(MyAsyncImpl).unwrap();
    let vtbl = unsafe { *(raw_obj as *mut *const IMyAsyncOpVtbl) };

    // --- Allocation Benchmark ---

    // 1. kcom: async method -> AsyncOperation allocation
    measure_ns("Rust_kcom_AsyncOp_New", ITERATIONS, baseline, || {
        let op = unsafe { ((*vtbl).get_status)(raw_obj) };
        if op.is_null() {
            return;
        }
        unsafe {
            let op = ComRc::<AsyncOperationRaw<i32>>::from_raw_unchecked(op);
            black_box(op.as_ptr());
        }
    });

    #[derive(Debug)]
    struct ReadyValue {
        value: i32,
    }

    // 2. Standard Rust: Box::new
    measure_ns("Rust_Box_New_Ready", ITERATIONS, baseline, || {
        let boxed = Box::new(ReadyValue { value: 1 });
        black_box(boxed.value);
        black_box(boxed);
    });

    // 3. Standard Rust: Arc::new (shared baseline)
    measure_ns("Rust_Arc_Ready", ITERATIONS, baseline, || {
        let shared = Arc::new(ReadyValue { value: 1 });
        black_box(shared.value);
        black_box(shared);
    });

    // --- Dispatch Benchmark ---

    // Prepare a completed async operation.
    let op = unsafe {
        let ptr = ((*vtbl).get_status)(raw_obj);
        if ptr.is_null() {
            return;
        }
        ComRc::<AsyncOperationRaw<i32>>::from_raw_unchecked(ptr)
    };
    let op_ptr = op.as_ptr();

    // 3. VTable call on AsyncOperation::get_status
    measure_ns("Rust_kcom_AsyncOp_GetStatus", ITERATIONS, baseline, || {
        unsafe {
            let mut status = AsyncStatus::Started;
            let _ = ((*(*op_ptr).lpVtbl).get_status)(
                op_ptr as *mut core::ffi::c_void,
                &mut status,
            );
            black_box(status);
        }
    });

    // 4. Standard Rust: direct method call
    let native = ModernImpl;
    measure_ns("Rust_Native_Call", ITERATIONS, baseline, || {
        let value = native.get_status();
        touch_native(value);
        black_box(value);
    });

    // Cleanup.
    unsafe {
        ComObject::<MyAsyncImpl, IMyAsyncOpVtbl>::shim_release(raw_obj);
    }
}

#[cfg(not(feature = "async-com"))]
fn main() {
    eprintln!("This benchmark requires the `async-com` feature. Run with `--features async-com`." );
}
