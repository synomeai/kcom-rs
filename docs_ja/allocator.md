# アロケータ

`kcom` は最小の `Allocator` トレイトと、カーネル向けの補助型を提供します。

## Allocator トレイト

```rust
pub trait Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8;
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 { /* default */ }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout);
}
```

ポイント:

- `GlobalAlloc` に近い API だが、オブジェクトとして渡せる
- `alloc_zeroed` は `alloc` + `write_bytes` がデフォルト
- `ComObject`/`KBox` はアロケータを割り当て領域内に保持し、解放前に
  `ptr::read` で取り出す。アロケータはビットコピーで安全に移動でき、
  解放対象のメモリを参照しない軽量ハンドル型（`Copy` 相当）が前提。

## GlobalAllocator

デフォルトアロケータ:

- ドライバ: `WdkAllocator(NonPagedNx)`
- ホスト/Miri: `alloc::alloc`

## WdkAllocator

対象 feature: `driver`

性質:

- `PoolType::NonPagedNx` / `PoolType::Paged` を選択可能
- `ExAllocatePool2` を優先利用
- 古い OS では `ExAllocatePoolWithTag` にフォールバック

初期化:

- `init_ex_allocate_pool2()` を `PASSIVE_LEVEL` で呼ぶ
- 遅延解決は IRQL の問題があるため明示初期化推奨

アライメント:

- 既定はカーネル標準の 16 バイト（x64）
- `wdk-alloc-align` で over-aligned を許可（ヘッダ + パディング）
- `wdk-alloc-align` の debug ビルドでは全割り当てにヘッダを付与し、
  `dealloc` 時に記録されたアライメントを検証して不整合を検出

ゼロ初期化:

- `alloc_zeroed` は必ずゼロ化
- `ExAllocatePool2` がゼロ化する場合でも挙動は統一

## KBox / InitBox

`KBox<T, A>` は `kcom::Allocator` を利用する Box。

`InitBox` は `PinInit` を使った安全な初期化を提供します。

`PinInit` の契約:

- `Err` を返す場合、`ptr` は未初期化（またはクリーンアップ済み）
- 呼び出し側は `Drop` を呼ばずに dealloc する可能性がある

`pin_init!` / `pin_init_async!` で一般的な初期化を簡潔に書けます。

