use kcom::{
    declare_com_interface, impl_com_interface, impl_com_object, ComObject, GUID, IUnknownVtbl,
    NTSTATUS, STATUS_SUCCESS,
};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{compiler_fence, Ordering};
use std::time::Instant;

// =========================================================
// 1. Interface Definition
// =========================================================

declare_com_interface! {
    pub trait IMyAsyncOp: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1111_2222, data2: 0x3333, data3: 0x4444,
            data4: [0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC],
        };
        // C++: virtual int GetStatus(int* status) = 0;
        fn get_status(&self, status: &mut i32) -> NTSTATUS;
    }
}

// =========================================================
// 2. kcom Implementation (Corresponds to ManualComImpl)
// =========================================================

struct MyImpl;

impl IMyAsyncOp for MyImpl {
    #[inline(never)]
    fn get_status(&self, status: &mut i32) -> NTSTATUS {
        *status = 1;
        STATUS_SUCCESS
    }
}

impl_com_interface! {
    impl MyImpl: IMyAsyncOp {
        parent = IUnknownVtbl,
        methods = [get_status],
    }
}

impl_com_object!(MyImpl, IMyAsyncOpVtbl);

// =========================================================
// 3. Standard Rust Implementation (Corresponds to ModernImpl)
// =========================================================

struct ModernImpl;

impl ModernImpl {
    #[inline(never)]
    fn get_status(&self, status: &mut i32) -> i32 {
        *status = 1;
        0
    }
}

// =========================================================
// Benchmarking Utilities
// - Warmup + compiler_fence to reduce measurement skew from optimizations.
// =========================================================

const WARMUP_ITERATIONS: u64 = 100_000;

static mut BASELINE_SINK: u64 = 0;

#[inline(never)]
fn touch_baseline() -> u64 {
    unsafe {
        let value = core::ptr::read_volatile(core::ptr::addr_of!(BASELINE_SINK));
        let next = value.wrapping_add(1);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_SINK), next);
        next
    }
}

#[inline(never)]
fn touch_native(value: i32) {
    unsafe {
        let current = core::ptr::read_volatile(core::ptr::addr_of!(BASELINE_SINK));
        let next = current.wrapping_add(value as u64);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_SINK), next);
    }
}

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

fn measure_ns<F>(name: &str, iterations: u64, baseline: f64, mut func: F) -> f64
where
    F: FnMut(),
{
    let avg = measure_ns_raw(iterations, &mut func);
    let adj = if avg > baseline { avg - baseline } else { 0.0 };
    println!("[{}] Average: {:.5} ns (adj {:.5} ns)", name, avg, adj);
    adj
}

fn main() {
    const ITERATIONS: u64 = 10_000_000; // 10M loops

    println!("Running Rust (kcom) Benchmarks ({} iterations)...", ITERATIONS);
    println!("-----------------------------------------------------");
    let baseline = measure_ns_raw(ITERATIONS, || {
        black_box(touch_baseline());
    });
    println!("[Rust_Empty_Loop] Average: {:.5} ns", baseline);

    // --- Allocation Benchmark ---

    // 1. kcom: new + RefCount Init (Corresponds to Cpp_Manual_New)
    measure_ns("Rust_kcom_New", ITERATIONS, baseline, || {
        // C++: new ManualComImpl() -> Release()
        // Rust: ComObject::new() -> shim_release()
        // ※ new_com は *mut c_void を返す (Raw pointer)
        let ptr = MyImpl::new_com(MyImpl).unwrap();
        unsafe {
            ComObject::<MyImpl, IMyAsyncOpVtbl>::shim_release(ptr);
        }
    });

    #[derive(Debug)]
    struct ReadyValue {
        value: i32,
    }

    // 2. Standard Rust: Box::new (Corresponds to Cpp_New_Ready)
    measure_ns("Rust_Box_New", ITERATIONS, baseline, || {
        // C++: std::make_shared<ModernImpl>()
        // Rust: Box::new(ModernImpl)
        let ptr = Box::new(ReadyValue { value: 1 });
        black_box(ptr.value);
        black_box(ptr);
    });

    // 3. Standard Rust: Arc::new (Corresponds to Cpp_Make_Shared_Ready)
    measure_ns("Rust_Arc_Ready", ITERATIONS, baseline, || {
        let ptr = Arc::new(ReadyValue { value: 1 });
        black_box(ptr.value);
        black_box(ptr);
    });

    // --- Dispatch Benchmark ---

    // 準備: Raw pointer を作成
    let raw_void = MyImpl::new_com(MyImpl).unwrap();
    let raw_ptr = raw_void as *mut IMyAsyncOpRaw;

    // 3. Virtual Method Call (Corresponds to Cpp_Virtual_Call)
    measure_ns("Rust_kcom_Call", ITERATIONS, baseline, || {
        let mut status = 0;
        unsafe {
            // C++: raw_obj->GetStatus(&status)
            let vtbl = (*raw_ptr).lpVtbl;
            let _ret = ((*vtbl).get_status)(
                raw_ptr as *mut core::ffi::c_void,
                &mut status,
            );
            black_box(status);
        }
    });

    // 4. Native direct call
    let native = ModernImpl;
    measure_ns("Rust_Native_Call", ITERATIONS, baseline, || {
        let mut status = 0;
        let _ = native.get_status(&mut status);
        touch_native(status);
        black_box(status);
    });

    // クリーンアップ
    unsafe {
        ComObject::<MyImpl, IMyAsyncOpVtbl>::shim_release(raw_void);
    }
}
