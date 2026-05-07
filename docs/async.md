# Async COM

This document describes the async COM surface and how `AsyncOperation` objects
are produced and consumed.

## Overview

Async methods declared via `declare_com_interface!` return a pointer to an
`AsyncOperationRaw<T>`. The object records status, error state, and (when
completed) the final value.

Key types:

- `AsyncOperationRaw<T>`: COM-facing interface with `get_status` / `get_result`
- `AsyncOperationVtbl<T>`: vtable layout (IUnknown + async methods)
- `AsyncOperationTask<T, F>`: internal state machine that owns the result
- `AsyncStatus`: `Started | Completed | Canceled | Error`
- `AsyncValueType`: `Copy + Send + Sync + 'static` bound for result values

The async surface is feature-gated:

- `async-com` for the trait/vtable surface
- `async-com-kernel` to bind to kernel executors

## Defining async methods

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

Async traits are marked `unsafe` because the caller must uphold:

- futures are `Send + 'static`
- allocator is `Send + Sync`
- the executor can actually poll the future to completion

## Spawning operations

The shim created by the macro:

1. Builds the future using `InitBoxTrait`.
2. Adds a refcount guard to keep the COM object alive while the future runs.
3. Spawns the future on the executor.
4. Returns an `AsyncOperationRaw<T>` pointer.

If init fails, the shim returns an `AsyncOperation` that reports an error
status rather than returning null.

There are also explicit helpers:

- `spawn_async_operation` / `spawn_async_operation_raw`
- `spawn_async_operation_cancellable` / `_raw_cancellable`
- `spawn_async_operation_error` / `_error_raw`

## Status and results

`AsyncOperationRaw<T>` exposes:

- `get_status` -> `AsyncStatus`
- `get_result` -> `T` or a status error

Rules:

- When `Started`, `get_result` returns `STATUS_PENDING`.
- When `Completed`, `get_result` writes the value and returns `STATUS_SUCCESS`.
- When `Canceled` or `Error`, the stored error status is returned.

The error path is useful for propagating initialization or allocation failures
from the shim.

## Cancellation

Cancellation is executor-driven:

- `spawn_dpc_task_cancellable` returns a `CancelHandle`
- `try_finally` wraps a main future and async cleanup
- cancellation requests are handled by `take_cancellation_request`
