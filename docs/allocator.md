# Allocator

`kcom` defines a minimal `Allocator` trait and several helper types to support
kernel allocation patterns.

## Allocator trait

```rust
pub trait Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8;
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 { /* default */ }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout);
}
```

Notes:

- The trait mirrors `GlobalAlloc` but is object-safe and easy to pass around.
- `alloc_zeroed` uses `alloc` + `write_bytes` unless overridden.
- `ComObject`/`KBox` store the allocator inside the allocation and move it out
  with `ptr::read` before freeing. Allocators must be safe to move by bitcopy
  and must not borrow from the allocation being freed (prefer small handle or
  `Copy`-like types).

## GlobalAllocator

Default allocator used by `ComObject` and `InitBox`:

- Driver builds: backed by `WdkAllocator` using `NonPagedNx`.
- Host/Miri builds: uses `alloc::alloc`.

## WdkAllocator

Feature: `driver` (requires `wdk-sys`).

Properties:

- Supports `PoolType::NonPagedNx` and `PoolType::Paged`.
- Uses `ExAllocatePool2` when available.
- Falls back to `ExAllocatePoolWithTag` on older systems.

Initialization:

- Call `init_ex_allocate_pool2()` at PASSIVE_LEVEL (e.g. `DriverEntry`).
- Lazy resolution may occur at elevated IRQL; explicit init is safer.

Alignment:

- Default path enforces kernel alignment (16 bytes on x64).
- `wdk-alloc-align` enables over-aligned allocations with padding and a small
  header. The default fast path is unchanged.
- Debug builds with `wdk-alloc-align` use the header for all allocations and
  validate the recorded alignment on `dealloc` to catch mismatched layouts.

Zeroing:

- `alloc_zeroed` guarantees zeroed memory.
- When `ExAllocatePool2` is used without `UNINITIALIZED`, the OS may already
  zero the memory. The fallback path zeros manually to keep behavior consistent.

## KBox and InitBox

`KBox<T, A>` is a heap box using a `kcom::Allocator`.

`InitBox` provides fallible, pinned initialization using `PinInit`.

`PinInit` contract:

- On error, the memory must be left uninitialized (or cleaned by the init
  implementation). Callers may deallocate without running `Drop` on `T`.

`pin_init!` and `pin_init_async!` macros make common initialization patterns
 concise.

