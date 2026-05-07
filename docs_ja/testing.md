# テスト

`kcom` はユニットテスト・統合テスト・Miri を組み合わせて検証します。

## テスト構成

- `src/*` のユニットテスト（lib test）
- `tests/*` の統合テスト
- `kcom-tests/`（実践的な利用形態のテスト）

## Miri ランナー

`scripts/test_all.ps1` で Miri を複数構成で実行します。

- default
- async-com
- kernel-unicode
- refcount-hardening
- combo（async-com + kernel-unicode + refcount-hardening）
- wdk-alloc-align（`driver` + `wdk-alloc-align` + `driver-test-stub`）
- driver-miri（`driver` + `async-com-kernel` + `driver-test-stub`）

## driver-test-stub スタブ

`driver-test-stub` はユーザーモードで `driver` feature をリンクするための
`DriverEntry` を提供します。

## ホスト/ドライバの注意

ホスト/Miri の Executor は 1 回だけ poll するスタブです。
本番のスケジューリングが必要なテストはカーネルで実行してください。

## 代表コマンド

```text
cargo test
cargo +nightly miri test -p kcom-tests --features "async-com"
cargo +nightly miri test -p kcom-tests --features "driver async-com-kernel driver-test-stub"
```

