// benches/async_benchmark.rs

use criterion::{black_box, criterion_group, criterion_main, Criterion};

#[cfg(feature = "async-com")]
use kcom::{spawn_async_operation, AsyncStatus};

#[cfg(feature = "async-com")]
mod async_benches {
    use super::*;

    // 比較対象: Rust標準の Box<Future>
    pub(super) fn bench_native_allocation(c: &mut Criterion) {

        c.bench_function("native_box_pin_overhead", |b| {
            b.iter(|| {
                // 単純に Future をヒープに確保して Pin するコスト
                let future = async { black_box(42) };
                let boxed = Box::pin(future);
                black_box(boxed);
            })
        });
    }

    // kcom: Task<F> の確保と VTable セットアップのコスト
    pub(super) fn bench_kcom_allocation(c: &mut Criterion) {

        c.bench_function("kcom_spawn_task_overhead", |b| {
            b.iter(|| {
                // 1. Task構造体のアロケーション
                // 2. VTable (const fn) のセットアップ
                // 3. 参照カウント初期化
                // 4. executor への投入 (UserModeスタブでは即時実行)
                let result = super::spawn_async_operation(async { black_box(42) });
                black_box(result.unwrap());
            })
        });
    }

    // VTable 経由のメソッド呼び出しコスト (Dispatch)
    pub(super) fn bench_vtable_dispatch(c: &mut Criterion) {

        // 準備: 完了済みのタスクを1つ作る
        let task = super::spawn_async_operation(async { 42 }).unwrap();
        let raw_ptr = task.as_ptr();

        c.bench_function("kcom_vtable_get_status", |b| {
            b.iter(|| {
                // FFI境界を越えて GetStatus を呼び出す
                // 期待値: 直接関数呼び出し + 数ナノ秒 (jmp + 引数セットアップ)
                unsafe {
                    let mut status = super::AsyncStatus::Started;
                    let _ = ((*(*raw_ptr).lpVtbl).get_status)(
                        raw_ptr as *mut core::ffi::c_void,
                        &mut status,
                    );
                    black_box(status);
                }
            })
        });
    }

    // 比較対象: Rust の dyn Trait 経由のメソッド呼び出し
    trait NativeStatus {
        fn get_status(&self) -> super::AsyncStatus;
    }
    struct NativeImpl;
    impl NativeStatus for NativeImpl {
        #[inline(never)] // フェアにするためインライン化を阻害
        fn get_status(&self) -> super::AsyncStatus {
            super::AsyncStatus::Completed
        }
    }

    pub(super) fn bench_native_dispatch(c: &mut Criterion) {

        let obj = Box::new(NativeImpl);
        let trait_obj: &dyn NativeStatus = &*obj;

        c.bench_function("native_dyn_trait_call", |b| {
            b.iter(|| {
                black_box(trait_obj.get_status());
            })
        });
    }
}

#[cfg(feature = "async-com")]
criterion_group!(
    benches,
    async_benches::bench_native_allocation,
    async_benches::bench_kcom_allocation,
    async_benches::bench_native_dispatch,
    async_benches::bench_vtable_dispatch
);
#[cfg(feature = "async-com")]
criterion_main!(benches);

#[cfg(not(feature = "async-com"))]
fn bench_stub(_c: &mut Criterion) {}

#[cfg(not(feature = "async-com"))]
criterion_group!(benches, bench_stub);
#[cfg(not(feature = "async-com"))]
criterion_main!(benches);