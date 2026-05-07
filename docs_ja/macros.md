# マクロ

ここでは公開マクロと生成コードの要点をまとめます。

## declare_com_interface!

COM インターフェースの trait と VTable 型を宣言します。

基本形:

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

主な性質:

- `trait IFoo: IUnknown + Sync` と `#[repr(C)] IFooVtbl` を生成
- VTable 先頭は親 VTable（IUnknown または他のインターフェース）
- 生成された VTable 型は `InterfaceVtable` を実装
- `ComObject` / `ComObjectN` が利用する shim を生成

### 戻り値のマッピング

sync shim は `NTSTATUS` に変換されます。

- `fn foo(...) -> NTSTATUS` はそのまま
- `fn foo(...) -> Result<T, E>` は `IntoNtStatus` で変換
  - `Ok(_)` → `STATUS_SUCCESS`
  - `Err(e)` → `e.into()`

`Result` / `core::result::Result` / `std::result::Result` を許容。

### Async メソッド

`async fn` は `async-com` feature が必須で、以下を生成します。

- `type FooFuture: Future<Output = Ret> + Send + 'static`
- `type Allocator: Allocator + Send + Sync`
- `InitBoxTrait<FooFuture, Allocator, NTSTATUS>` を返すメソッド
- `*mut AsyncOperationRaw<Ret>` を返す VTable エントリ

Async trait は安全性上 `unsafe` になります。

## impl_com_interface!

`ComImpl` を実装し、宣言済みインターフェースの VTable を生成します。

```rust
impl_com_interface! {
    impl MyType: IFoo {
        parent = IUnknownVtbl,
        methods = [ping, fallible],
    }
}
```

多重インターフェースの場合:

```rust
impl_com_interface! {
    impl MyType: IPrimary {
        parent = IUnknownVtbl,
        secondaries = (IBar, IBaz),
        methods = [foo],
    }
}
```

ポイント:

- 単一インターフェースでは、primary IID のみに `this` を返す簡易 QI を生成
- 多重インターフェースでは primary + secondaries を自動マッチ
- `allocator = SomeAllocator` を指定可能

## impl_com_interface_multiple!

`ComObjectN` で非プライマリインターフェースを実装します。

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

secondary エントリの VTable と index を正しく結び付けます。

## impl_query_interface!

明示的な `query_interface` を簡潔に書くためのマクロです。

```rust
impl_query_interface! {
    Self,
    this,
    riid,
    [IFoo, IBar => bar_ptr],
    fallback = IUnknownVtbl
}
```

戻すポインタは **安定** かつ **IID と一致する VTable** を持つ必要があります。

## impl_com_object!

実装型に便利なコンストラクタを追加します。

- `new_com` / `new_com_rc`
- `new_com_in` / `new_com_rc_in`
- `try_new_com` / `try_new_com_rc`
- Aggregation 系 (`new_com_aggregated*`, `try_new_com_aggregated*`)

Aggregation 系は raw outer IUnknown を受け取るため `unsafe` です。

## 補助マクロ

- `ensure!` / `trace!`：トレースフックへ報告
- `iunknown_vtbl!`：IUnknown VTable を生成
- `pin_init!`, `pin_init_async!`, `init_box!`：`InitBox` を構築

