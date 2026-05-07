# Macros

This document summarizes the public macros and the code they generate.

## declare_com_interface!

Declares a COM interface trait and its vtable type.

Basic shape:

```rust
declare_com_interface! {
    pub trait IFoo: IUnknown {
        const IID: GUID = GUID { /* ... */ };
        fn ping(&self, value: u32) -> NTSTATUS;
        fn fallible(&self) -> Result<(), NTSTATUS>;
        async fn get_status(&self) -> i32;
    }
}
```

Key properties:

- Generates a `trait IFoo: IUnknown + Sync` and a `#[repr(C)]` `IFooVtbl`.
- Vtable layout starts with the parent vtable (IUnknown or another interface).
- The generated vtable type implements `InterfaceVtable`.
- The macro also emits the shim functions used by `ComObject`/`ComObjectN`.

### Return type mapping

The sync shims convert to `NTSTATUS`:

- `fn foo(...) -> NTSTATUS` is passed through.
- `fn foo(...) -> Result<T, E>` is mapped via `IntoNtStatus`.
  - `Ok(_)` => `STATUS_SUCCESS`
  - `Err(e)` => `e.into()`

The macro accepts `Result` / `core::result::Result` / `std::result::Result`.

### Async methods

`async fn` in the macro requires the `async-com` feature and generates:

- A `type FooFuture: Future<Output = Ret> + Send + 'static`
- A `type Allocator: Allocator + Send + Sync`
- A method returning `InitBoxTrait<FooFuture, Allocator, NTSTATUS>`
- A vtable entry returning `*mut AsyncOperationRaw<Ret>`

Async interfaces are marked `unsafe` because the caller must uphold executor
and lifetime constraints.

## impl_com_interface!

Implements `ComImpl` and builds a static vtable for a declared interface.

Common forms:

```rust
impl_com_interface! {
    impl MyType: IFoo {
        parent = IUnknownVtbl,
        methods = [ping, fallible],
    }
}
```

With multiple interfaces:

```rust
impl_com_interface! {
    impl MyType: IPrimary {
        parent = IUnknownVtbl,
        secondaries = (IBar, IBaz),
        methods = [foo],
    }
}
```

Notes:

- For single-interface objects, the macro emits a simple QI that returns `this`
  for the primary IID, and defers to the fallback for everything else.
- For multiple interfaces, the macro emits a QI that matches the primary and
  all secondary IIDs and returns secondary pointers via `ComObjectN`.
- `allocator = SomeAllocator` can be supplied for the multi-interface case.

## impl_com_interface_multiple!

Implements `ComImpl` for non-primary interfaces when using `ComObjectN`.

Example:

```rust
impl_com_interface_multiple! {
    impl MyType: IBar {
        parent = IUnknownVtbl,
        primary = IFoo,
        index = 0,
        secondaries = (IBar, IBaz),
        methods = [bar],
    }
}
```

This ties the non-primary vtable to the correct secondary entry and sets up
QI in a way that is consistent with the primary.

## impl_query_interface!

Provides a small, explicit QI implementation.

```rust
impl_query_interface! {
    Self,
    this,
    riid,
    [IFoo, IBar => bar_ptr],
    fallback = IUnknownVtbl
}
```

The macro compares `riid` to interface IIDs and returns a pointer on match.
It expects returned pointers to be stable, non-null, and to point to the
correct vtable for the requested IID.

## impl_com_object!

Adds convenience constructors to an implementation type:

- `new_com` / `new_com_rc`
- `new_com_in` / `new_com_rc_in`
- `try_new_com` / `try_new_com_rc`
- Aggregation variants (`new_com_aggregated*`, `try_new_com_aggregated*`)

Aggregation methods are `unsafe` because they accept raw outer IUnknown pointers.

## Helper macros

- `ensure!` and `trace!` for debug reporting with the trace hook.
- `iunknown_vtbl!` to build a basic IUnknown vtable referencing wrapper shims.
- `pin_init!`, `pin_init_async!`, and `init_box!` to build `InitBox` payloads.

