# Async COM

このドキュメントでは Async COM サーフェスと `AsyncOperation` の生成/利用を説明します。

## 概要

`declare_com_interface!` で宣言した Async メソッドは
`AsyncOperationRaw<T>` へのポインタを返します。内部には
状態・エラー・結果が格納されます。

主な型:

- `AsyncOperationRaw<T>`: COM 側のインターフェース（`get_status` / `get_result`）
- `AsyncOperationVtbl<T>`: VTable レイアウト（IUnknown + Async）
- `AsyncOperationTask<T, F>`: 結果を保持する内部状態
- `AsyncStatus`: `Started | Completed | Canceled | Error`
- `AsyncValueType`: `Copy + Send + Sync + 'static` 制約

対象 feature:

- `async-com`: trait/VTable を有効化
- `async-com-kernel`: カーネル実行系との統合

## Async メソッドの定義

```rust
declare_com_interface! {
    pub trait IMyAsync: IUnknown {
        const IID: GUID = GUID { /* ... */ };
        async fn get_status(&self) -> i32;
    }
}

struct MyImpl;

unsafe impl IMyAsync for MyImpl {
    type GetStatusFuture = core::future::Ready<i32>;
    type Allocator = GlobalAllocator;

    fn get_status(&self) -> impl InitBoxTrait<Self::GetStatusFuture, Self::Allocator, NTSTATUS> {
        InitBox::new(GlobalAllocator, pin_init!(core::future::ready(1)))
    }
}
```

Async trait は `unsafe` です。以下を呼び出し側が保証します:

- Future が `Send + 'static`
- Allocator が `Send + Sync`
- Executor が完了まで poll する

## 操作の生成

shim は次の流れで動作します:

1. `InitBoxTrait` で Future を生成
2. COM オブジェクトの参照を保持するガードを追加
3. Executor にタスクを登録
4. `AsyncOperationRaw<T>` を返す

初期化に失敗した場合は **null を返すのではなく**
エラー状態の `AsyncOperation` を返します。

利用できるヘルパー:

- `spawn_async_operation` / `spawn_async_operation_raw`
- `spawn_async_operation_cancellable` / `_raw_cancellable`
- `spawn_async_operation_error` / `_error_raw`

## 状態と結果

`AsyncOperationRaw<T>` の挙動:

- `Started` のとき `get_result` は `STATUS_PENDING`
- `Completed` のとき `get_result` は値を書き込んで `STATUS_SUCCESS`
- `Canceled` / `Error` は保持している `NTSTATUS` を返す

初期化失敗などはエラー状態に反映されます。

## キャンセル

キャンセルは Executor に依存します:

- `spawn_dpc_task_cancellable` が `CancelHandle` を返す
- `try_finally` により cleanup を Async で実行可能
- `take_cancellation_request` がキャンセル要求を消費

