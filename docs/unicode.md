# Unicode

This module is available under the `kernel-unicode` feature and provides
helpers around `UNICODE_STRING`.

## OwnedUnicodeString

Heap-backed UNICODE_STRING that owns its buffer.

Properties:

- Allocates `len + 1` UTF-16 units to ensure a null terminator.
- Stores `Length` (bytes, not including terminator) and `MaximumLength`.
- Buffer is freed in `Drop`.

Construction:

```rust
let name = OwnedUnicodeString::new(\"\\\\Device\\\\MyDriver\").unwrap();
let unicode = name.as_unicode();
```

## LocalUnicodeString

Stack-based buffer with fixed capacity:

```rust
let mut s = LocalUnicodeString::<64>::new();
write!(&mut s, \"Err {:#x}\", status).unwrap();
let unicode = s.as_unicode_ref();
```

Notes:

- Capacity is in UTF-16 units, not bytes.
- `try_push_str` ensures space for a null terminator.
- `as_unicode_ref()` returns a lifetime-tied view.

## kstr! (static literals)

`kstr!(\"Text\")` builds a `&'static UNICODE_STRING` without allocations.

## Conversions

`unicode_string_as_slice` returns a UTF-16 slice view.

`unicode_string_to_string` builds a `String` using `try_reserve` to avoid OOM
panics; errors are reported as `UnicodeStringError`.

