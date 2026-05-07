# ベンチマーク

`benches/` には同期/非同期の比較ベンチが含まれます。

- `comparison.rs` / `comparison.cpp`（同期比較）
- `comparison_async.rs` / `comparison_async.cpp`（非同期比較）
- `async_benchmark.rs`（criterion ベンチ）

## 実行（Rust）

```text
cargo bench --bench comparison
cargo bench --bench comparison_async --features async-com
```

## 実行（C++）

```text
.\benches\comparison.exe
.\benches\comparison_async.exe
```

## 解釈ガイド

- CPU、電源設定、ビルドプロファイルを明記する
- 空ループのオーバーヘッドを差し引いた値を併記する
- カーネル API コストが支配的な場合はその旨を記録する

取得済みの結果は `Benchmark.md` に整理されています。

