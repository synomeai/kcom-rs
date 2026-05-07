# 安全性と不変条件

このドキュメントは `unsafe` 境界や API 契約を整理します。

## COM ポインタの有効性

`*mut c_void` / raw COM ポインタを受け取る関数は、以下を前提とします:

- ポインタが null ではない（許可されている場合を除く）
- VTable が有効で IUnknown から始まる
- 呼び出し中にオブジェクトが解放されない

このため `ComRc::from_raw*` 系は `unsafe` です。

## query_interface 契約

`ComImpl::query_interface` は以下を満たす必要があります:

- IID に一致する VTable を持つ **安定した** ポインタを返す
- primary 以外で `this` を返さない
- inner Rust オブジェクト `T` へのポインタは返さない

## Aggregation

`new_aggregated*` は raw outer IUnknown を受け取るため `unsafe` です。

呼び出し側は:

- outer ポインタが有効であること
- outer が inner より長生きすること

を保証する必要があります。

## Async メソッド

Async trait は `unsafe` です。以下を呼び出し側が保証します:

- Future が `Send + 'static`
- Allocator が `Send + Sync`
- Executor が完了まで poll する

shim は参照カウントガードを付け、実行中は COM オブジェクトが生存します。

## Executor / IRQL ルール

DPC は `DISPATCH_LEVEL`:

- NonPaged であること
- ブロッキング / pageable API を使わない
- 大きなスタック変数を避ける

Work-item は `PASSIVE_LEVEL` で実行されます。

## Panic 方針

`kcom` は `extern "system"` 境界で panic を捕捉しません。
カーネルでは panic を致命的とみなし:

- `panic = "abort"`
- 必要に応じて `KeBugCheckEx` へ接続

すべての COM shim は panic ガードを使い、Unwind が境界を越える場合は
フェイルファストします。

## Refcount ハードニング / Resurrection

`refcount-hardening` により overflow/underflow を検出します。
`leaky-hardening` は overflow/underflow 時に bug check せず、参照カウントを飽和させて
オブジェクトをリークさせます（OS を維持するための選択肢です）。

`Release` 中に `Drop` が `AddRef` を呼び参照カウントが復活した場合、
`kcom` はフェイルファストします（ドライバでは bug check）。
この動作は `leaky-hardening` 有効時も変わりません。

## Provenance ポリシー

Async ガードのポインタは `NonNull<c_void>` で保持し、strict provenance を
デフォルトで採用します。整数キャストを避けつつ、生ポインタと同じ ABI
レイアウトを維持します。

