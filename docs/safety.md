# Safety and Invariants

This document lists the key unsafe boundaries and invariants that callers and
implementors must uphold.

## COM pointer validity

Any function that accepts `*mut c_void` or raw COM interface pointers assumes:

- the pointer is non-null (unless explicitly allowed)
- the vtable is valid and starts with IUnknown
- the object is alive for the duration of the call

`ComRc::from_raw*` constructors are unsafe for this reason.

## query_interface contract

`ComImpl::query_interface` must:

- return a stable COM interface pointer whose vtable matches the requested IID
- return `this` only for the primary interface at offset 0
- never return a pointer into the inner Rust object (`T`)

## Aggregation

`new_aggregated*` APIs are `unsafe` because they accept raw outer IUnknown
pointers. The caller must guarantee:

- the outer pointer is valid and points to a real COM object
- the outer object outlives the inner object

The inner object exposes a non-delegating IUnknown for the outer to manage.

## Async methods

Async shims are `unsafe` to implement because:

- futures must be `Send + 'static`
- allocators must be `Send + Sync`
- the executor must poll the future to completion

The shim adds a refcount guard so the COM object stays alive until completion.

## Executor IRQL rules

DPC execution runs at `DISPATCH_LEVEL`:

- futures must be non-blocking and non-paged
- no pageable memory or blocking kernel APIs
- avoid large stack allocations in async state machines

Work-item execution runs at `PASSIVE_LEVEL` and is safer, but still requires
correct lifetime management for device objects.

## Panic behavior

`kcom` does not catch panics across `extern "system"` boundaries. In kernel
builds, panics should be treated as fatal:

- set `panic = "abort"`
- optionally route to `KeBugCheckEx` in a panic handler

All COM shims install a panic guard and fail-fast if unwinding would cross the
FFI boundary.

## Refcount hardening and resurrection

`refcount-hardening` adds overflow/underflow guards.
`leaky-hardening` keeps the system alive on overflow/underflow by saturating the
refcount instead of bug checking (this can leak the object).

During `Release`, `kcom` checks for refcount resurrection (a Drop that calls
AddRef). If detected, the code triggers a fail-fast path (bug check in driver
builds), even when `leaky-hardening` is enabled. This prevents use-after-free
when the object is resurrected during destruction.

## Provenance policy

Async guard pointers are stored using `NonNull<c_void>` with strict provenance
semantics by default. This avoids integer-based casts while keeping the ABI
layout identical to a raw pointer.

