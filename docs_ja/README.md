# kcom ドキュメント (日本語)

このディレクトリは `kcom` クレートの設計・使用方法・安全性境界・
カーネル固有の制約を詳しく説明します。トップレベルの `README.md` を補完する
詳細資料として利用してください。

## 目次

- `architecture.md` — 全体アーキテクチャとオブジェクトレイアウト
- `macros.md` — マクロ API（`declare_com_interface!` など）
- `async.md` — Async COM 仕様、`AsyncOperation`、キャンセル
- `executor.md` — DPC / Work-item 実行系、IRQL 制約
- `allocator.md` — アロケータ設計、`WdkAllocator`、アライメント
- `unicode.md` — `UNICODE_STRING` ヘルパー
- `safety.md` — `unsafe` 境界・契約・パニック方針
- `testing.md` — テスト構成、Miri、`driver-test-stub` スタブ
- `benchmarks.md` — ベンチマークの実行と解釈
- `troubleshooting.md` — よくあるビルド/リンク/検証の問題

## 関連ドキュメント

- `README.md`（プロジェクト直下）— 概要と簡易サンプル
- `SmartVtbl.md` — Smart VTable の設計
- `Example.md` — 例コードと展開対応表
- `Benchmark.md` — 取得済みのベンチマーク結果
- `NOTES.md` — トレードオフや検証方針のメモ
- `FIND.md` — レビュー所見

## feature フラグ（概要）

詳細は `README.md` を参照してください。各ドキュメントでも必要に応じて
`driver` / `async-com` / `async-com-kernel` / `kernel-unicode` /
`refcount-hardening` / `wdk-alloc-align` を明記します。

## Allocator の補足

- `ComObject`/`KBox` はアロケータを割り当て領域内に保持し、解放前に
  `ptr::read` で取り出します。アロケータはビットコピーで安全に移動でき、
  解放対象のメモリを参照しない軽量ハンドル型（`Copy` 相当）が前提です。

## 表記ルール

- Cargo の機能は「feature フラグ」と表記
- VTable は `VTable` と表記
- 非同期は `Async` と表記（コード上の `async` と区別）
- 実行系は `Executor` と表記
- `Work-item` / `DPC` / `IRQL` は原語表記

