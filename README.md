# kcom

`kcom` is a zero-copy COM runtime for Windows kernel drivers, built for `no_std` Rust. It generates
VTables and shims from Rust traits, minimizing boilerplate for driver authors.

## Features

- **no_std** support for kernel-mode usage
- **Zero-copy layout** (VTable + refcount + Rust struct in one layout)
- **Macro-generated VTables** via `declare_com_interface!`
- **Result -> NTSTATUS** mapping in shims
- **Async interface definitions (Experimental)** returning `AsyncOperation` objects
- **QueryInterface helper macro** for multi-interface support
- **Multiple non-primary interfaces** via `ComObjectN` + `impl_com_interface_multiple!`
- **Reference-counted ComRc** smart pointer for client-side COM usage
- **Kernel Unicode helpers** for `UNICODE_STRING`
- **Stack-based Unicode buffers** via `LocalUnicodeString`
- **Optional refcount hardening** with overflow/underflow guards

## Feature flags

- `async-com`: enables async method support in `declare_com_interface!`
- `async-impl`: enables `async-com` and re-exports `async-trait` as `#[kcom::async_impl]`
- `async-com-kernel`: enables `async-com` and `wdk-sys` (kernel builds)
- `kernel-unicode`: enables `UNICODE_STRING` helpers (requires `wdk-sys`)
- `refcount-hardening`: adds refcount overflow/underflow guards (slower AddRef/Release, fail-fast abort)

## Async executor (kernel)

The kernel executor exposes cooperative cancellation and a work-item tracker to
drain outstanding work before driver unload.

> Note: async state machines are polled on the kernel stack. Avoid large stack
> locals in `async fn` (e.g. large arrays or buffers). Use heap allocation such
> as `KBox` or other kernel allocators for large data.

## Safety notes (panic + DPC)

- `spawn_dpc_task*` are **unsafe** because DPC polling happens at
  `DISPATCH_LEVEL`. Futures must be nonpaged, nonblocking, and must avoid
  pageable kernel APIs.
- `kcom` does **not** catch panics across `extern "system"` boundaries. In
  kernel builds, set `panic = "abort"` and (optionally) install a panic handler
  that calls `KeBugCheckEx` so failures are fail-fast instead of UB.

## Tracing hooks (ETW/WPP integration)

`kcom` exposes a lightweight trace hook to integrate with DbgPrint, WPP, or
custom ETW providers. The `ensure!` macro emits file/line in debug builds when
the hook is installed.

```rust
use kcom::{set_trace_hook, trace};

fn my_trace(args: core::fmt::Arguments<'_>) {
    // Forward to DbgPrint / WPP / ETW
    let _ = args;
}

set_trace_hook(my_trace);
trace!("driver started: status={:#x}", 0);
```

## Stack-based UNICODE_STRING

```rust
use kcom::LocalUnicodeString;
use core::fmt::Write;

let mut s = LocalUnicodeString::<64>::new();
write!(&mut s, "Err code: {:x}", 0x2a).unwrap();
let unicode = s.as_unicode_ref();
let unicode_ref = unicode.as_ref();
```

```rust
use kcom::{spawn_dpc_task_cancellable, CancelHandle, try_finally, WorkItemTracker};

let cancel: CancelHandle = unsafe { spawn_dpc_task_cancellable(async {
    // ... main work ...
    kcom::STATUS_SUCCESS
})? };

// Later (e.g. IRP cancel routine)
cancel.cancel();

// Async cleanup on cancellation
let _ = try_finally(async {
    // main
}, async {
    // cleanup
}).await;

// Work-item tracking
let tracker = WorkItemTracker::new();
let _ = kcom::spawn_task_tracked(device, &tracker, async {
    kcom::STATUS_SUCCESS
});
// Note: device must be non-null; null returns STATUS_INVALID_PARAMETER.
// ... during unload
tracker.drain();
```

## Kernel allocator initialization (driver)

When using `WdkAllocator` (feature `driver`), call `init_ex_allocate_pool2()` once at
PASSIVE_LEVEL (e.g. `DriverEntry`). Allocation paths will attempt a best-effort lazy
initialization, but that can occur at elevated IRQL, so explicit initialization is strongly
recommended to ensure `ExAllocatePool2` is used safely on Windows 10 2004+.

By default, `WdkAllocator` enforces the kernel pool alignment (16 bytes on x64). If you need
over-aligned allocations (e.g. `#[repr(align(32))]`), enable the `wdk-alloc-align` feature.
This adds padding and a small header only for over-aligned layouts; the default path is unchanged.

Allocator note:

- `ComObject`/`KBox` store the allocator inside the allocation and move it out with `ptr::read`
  before freeing. Allocators must be safe to move by bitcopy and must not borrow from the
  allocation being freed (prefer small handle or `Copy`-like types).

## Usage (sync interface)

```rust
use core::ffi::c_void;

use kcom::{
    declare_com_interface, impl_com_object, ComImpl, ComInterfaceInfo, ComObject, GUID, IUnknown,
    IUnknownVtbl, NTSTATUS, STATUS_SUCCESS,
};

declare_com_interface! {
    pub trait IFoo: IUnknown {
        const IID: GUID = GUID {
            data1: 0x1234_5678,
            data2: 0x1234,
            data3: 0x5678,
            data4: [0x90, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67],
        };

        fn ping(&self, value: u32) -> NTSTATUS;
    }
}

struct Foo;

impl IFoo for Foo {
    fn ping(&self, _value: u32) -> NTSTATUS {
        STATUS_SUCCESS
    }
}

// SAFETY: the vtable is built from kcom's shims for `Foo`, and
// query_interface only returns `this` for IFoo.
unsafe impl ComImpl<IFooVtbl> for Foo {
    const VTABLE: &'static IFooVtbl = &IFooVtbl {
        // Pass the most-derived vtable so QueryInterface dispatches through
        // `ComImpl<IFooVtbl>` and answers for IID_IFoo.
        parent: IUnknownVtbl::new::<Foo, IFooVtbl>(),
        ping: shim_IFoo_ping::<Foo>,
    };

    impl_query_interface! {
        Self,
        this,
        riid,
        [IFoo],
        fallback = IUnknownVtbl
    }
}

impl_com_object!(Foo, IFooVtbl);

fn main() {
    let com = Foo::new_com_rc::<IFooRaw>(Foo).unwrap();

    unsafe {
        let ptr = com.as_ptr();
        let vtbl = *(ptr as *mut *const IFooVtbl);
        let status = ((*vtbl).ping)(ptr as *mut _, 42);
        assert_eq!(status, STATUS_SUCCESS);
    }
}
```

When supporting additional interfaces, return explicit tear-off or aggregated pointers for
non-primary interfaces so the returned pointer’s vtable matches the requested IID.

Use `new_com_rc::<IFooRaw>` (or `new_com_rc_in`) to receive a `ComRc` that owns the initial
reference. `new_com` still returns a raw pointer with refcount 1.

## Multiple interfaces (non-primary vtables)

Use `ComObjectN` when you need a primary interface plus multiple non-primary interfaces. The
`secondaries` tuple declares the vtable order, and `index` selects the 0-based position.

```rust
use kcom::{impl_com_interface, impl_com_interface_multiple, IUnknownVtbl};
use kcom::wrapper::ComObjectN;

impl_com_interface! {
    impl Multi: IFoo {
        parent = IUnknownVtbl,
        secondaries = (IBar, IBaz),
        methods = [foo],
    }
}

impl_com_interface_multiple! {
    impl Multi: IBar {
        parent = IUnknownVtbl,
        primary = IFoo,
        index = 0,
        secondaries = (IBar, IBaz),
        methods = [bar],
    }
}

impl_com_interface_multiple! {
    impl Multi: IBaz {
        parent = IUnknownVtbl,
        primary = IFoo,
        index = 1,
        secondaries = (IBar, IBaz),
        methods = [baz],
    }
}

let obj_ptr = raw as *mut ComObjectN<Multi, IFooVtbl, (IBarVtbl, IBazVtbl)>;
let bar_ptr = unsafe {
    ComObjectN::<Multi, IFooVtbl, (IBarVtbl, IBazVtbl)>::secondary_ptr::<IBarVtbl, 0>(obj_ptr)
};
```

## Usage (async interface)

> **Experimental**: Async COM shims spawn tasks and return `AsyncOperation` objects.
> Host builds poll once; driver builds require a kernel executor to drive progress.

## Client-side COM pointers

Use `ComRc<T>` to manage COM references safely (AddRef/Release).

```rust
use kcom::ComRc;

// SAFETY: `raw` must be a valid COM interface pointer.
let com_ref = unsafe { ComRc::from_raw_addref(raw).unwrap() };
let raw_again = com_ref.as_ptr();
```

## Aggregation (non-delegating IUnknown)

`ComObject::new_aggregated` returns a **non-delegating IUnknown** pointer for use by the outer
object. The outer object owns the lifetime of the inner object through this pointer.

Calls made through interfaces returned by `QueryInterface` are **delegated** to the outer
unknown, while the non-delegating pointer manages the inner refcount directly.

```rust
use kcom::{ComObject, IUnknownVtbl};

// SAFETY: outer_unknown must be a valid IUnknown vtable from the outer object.
let non_delegating =
    unsafe { ComObject::<Inner, IUnknownVtbl>::new_aggregated(inner, outer_unknown) };
```

## Unicode helpers (kernel)

Enable the `kernel-unicode` feature to construct and read `UNICODE_STRING` values.

```rust
use kcom::OwnedUnicodeString;

let name = OwnedUnicodeString::new("\\Device\\MyDriver").unwrap();
let unicode = name.as_unicode();
```

Compile-time `UNICODE_STRING` literals can be built with `kstr!`:

```rust
use kcom::kstr;

let name = kstr!("\\Device\\MyDriver");
// name: &'static UNICODE_STRING
```
