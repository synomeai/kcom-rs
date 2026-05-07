# トラブルシューティング

## LNK2019: DriverEntry が未解決

`driver` feature をユーザーモードでリンクすると以下が出ることがあります:

- `WdfDriverEntry.lib(stub.obj) : error LNK2019: unresolved external symbol DriverEntry`

`driver-test-stub` feature を付けて `DriverEntry` のスタブを提供してください。

## Miri は stable で使えない

`cargo miri` は nightly が必要です:

```text
cargo +nightly miri test -p kcom-tests --features async-com
```

## target 配下へのアクセス拒否 (Windows)

`os error 5` が出る場合:

- `target/` を掴んでいるプロセスがないか確認
- ウイルス対策のリアルタイムスキャンを一時停止
- 再ビルド

## kernel-unicode の失敗

`kernel-unicode` はカーネルの `UNICODE_STRING` に依存します。
ホスト/Miri ではスタブ型を使用していて、想定した feature 構成か確認してください。
