# Unicode

対象 feature: `kernel-unicode`

`UNICODE_STRING` ヘルパーを提供します。

## OwnedUnicodeString

ヒープに所有権を持つ UNICODE_STRING。

特徴:

- UTF-16 の `len + 1` を確保し null 終端を持つ
- `Length` は終端を含まず、`MaximumLength` は含む
- `Drop` でバッファを解放

例:

```rust
let name = OwnedUnicodeString::new("\\\\Device\\\\MyDriver").unwrap();
let unicode = name.as_unicode();
```

## LocalUnicodeString

固定長スタックバッファ:

```rust
let mut s = LocalUnicodeString::<64>::new();
write!(&mut s, "Err {:#x}", status).unwrap();
let unicode = s.as_unicode_ref();
```

注意点:

- 容量は UTF-16 ユニット数
- `try_push_str` は終端の 1 ユニットを確保する
- `as_unicode_ref()` はライフタイムを結び付けた安全な参照

## kstr!

`kstr!("Text")` は静的 `UNICODE_STRING` を生成します。

## 変換

`unicode_string_as_slice` は UTF-16 スライスを返します。

`unicode_string_to_string` は `try_reserve` を用いて OOM パニックを避け、
`UnicodeStringError` でエラーを返します。

