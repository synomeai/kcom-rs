# Architecture

This document describes the high-level architecture and data layout used by
`kcom`. It is intentionally implementation-oriented so that you can reason
about ABI safety, allocation, and runtime behavior in kernel builds.

## Goals

- Provide COM-compatible interfaces in `no_std` Rust.
- Keep object layout zero-copy: one allocation that contains vtable pointer,
  refcount, and the Rust inner type.
- Generate shims and vtables at compile time (no runtime reflection).
- Support aggregation and multiple interfaces with deterministic layout.
- Provide an async COM surface that can be driven by a kernel executor.

## Major Components

- `macros/`: declarative macros that build interface traits, vtables, and shims.
- `wrapper`: `ComObject`/`ComObjectN` that host the inner Rust object and
  implement IUnknown plumbing.
- `vtable`: interface metadata (`ComInterfaceInfo`) and layout marker trait.
- `traits`: `ComImpl` and the `query_interface` contract.
- `smart_ptr`: `ComRc<T>` and `ComInterface` marker for client usage.
- `async_com`: `AsyncOperation` object model and spawn helpers.
- `executor`: DPC and work-item executors for kernel builds, plus host stubs.
- `allocator`: `Allocator` trait, `WdkAllocator`, `GlobalAllocator`, `KBox`.
- `unicode`: kernel `UNICODE_STRING` helpers (feature-gated).
- `trace`: lightweight trace hook for debug diagnostics.

## Object Layout

### Primary interface (`ComObject<T, Vtbl>`)

`ComObject` is a single allocation with a `repr(C)` layout:

- primary vtable pointer (first field, so COM expects it)
- non-delegating IUnknown shim (for aggregation)
- refcount
- optional outer unknown pointer
- inner Rust object (`T`)
- allocator instance (stored and dropped on release)

This layout allows the same pointer to be used as a COM interface pointer
(vtable at offset 0) and as the base for refcount and inner storage.

### Multiple interfaces (`ComObjectN<T, Primary, Secondaries>`)

`ComObjectN` extends the layout with a `secondaries` tuple that stores
additional vtable entries. Each secondary entry contains its own vtable and a
parent pointer for `this` adjustment.

The `secondaries` tuple is initialized after allocation so that:
- each secondary vtable points to the correct shim
- each secondary entry knows the parent object

The primary vtable remains at offset 0 to satisfy COM expectations.

## QueryInterface Flow

`ComObject` and `ComObjectN` implement the IUnknown shims:

- `shim_query_interface` validates the IID and uses the inner
  `ComImpl::query_interface` for custom mappings.
- If the requested IID matches the primary interface, the wrapper returns
  `this` (primary pointer).
- For non-primary interfaces, `ComImpl::query_interface` must return a stable
  pointer whose vtable matches the requested IID (often a secondary entry
  pointer).
- The wrapper calls `AddRef` on any returned pointer.

For `ComObjectN`, the primary vtable and `impl_com_interface!` macro can
auto-generate QI logic that returns secondary pointers.

## Aggregation

Aggregation uses a non-delegating IUnknown (NDI) stored within the object:

- Calls to the main interface can delegate IUnknown to the outer object.
- The outer object owns the NDI pointer and manages the inner lifetime.
- `new_aggregated*` is `unsafe`: the caller must provide a valid outer IUnknown.

## Async Pipeline (Overview)

- Async interface methods return an `AsyncOperationRaw<T>` pointer.
- The shim uses the user-provided allocator to build the future (`InitBox`),
  then spawns a task.
- The task writes the result into `AsyncOperationTask` and updates status.
- `AsyncOperationRaw` exposes `get_status` and `get_result` for polling.

## Cancellation and Tracking

`async_com` uses the executor layer for driving futures:

- DPC executor for DISPATCH_LEVEL (unsafe API)
- Work-item executor for PASSIVE_LEVEL (WDM)
- `CancelHandle` / `WorkItemCancelHandle` to request cancellation
- `TaskTracker` / `WorkItemTracker` to drain work before unload

The cancellation bit is stored in a per-CPU table for DPC tasks. CPU index
is group-aware; out-of-range indexes are debug-traced and treated as missing.

## Allocators

`Allocator` is a minimal trait with `alloc`, `alloc_zeroed`, and `dealloc`.

- `GlobalAllocator` is used by default.
  - In driver builds: backed by `WdkAllocator` with `NonPagedNx`.
  - In host builds: backed by `alloc::alloc`.
- `WdkAllocator` supports paged and non-paged pools and optional alignment
  expansion via `wdk-alloc-align`.

## Unicode Utilities

`kernel-unicode` provides:

- `OwnedUnicodeString` (heap-backed, allocator-aware)
- `LocalUnicodeString<N>` (stack buffer)
- `kstr!` static UNICODE_STRING literal (compile-time)

## Diagnostics

`trace` exposes a hook that can be connected to DbgPrint/WPP/ETW for debug
telemetry. The `ensure!` macro emits file/line context when enabled.

