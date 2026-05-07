# アーキテクチャ

このドキュメントでは `kcom` の高レベル構造とデータレイアウトを説明します。
ABI 安全性、アロケーション、カーネル実行時の挙動を理解するための
実装寄りの説明になっています。

## 目標

- `no_std` Rust で COM 互換インターフェースを提供する
- 1 つのアロケーションに VTable / 参照カウント / inner をまとめたゼロコピー設計
- 実行時リフレクションではなく、コンパイル時に VTable/shim を生成
- Aggregation と多重インターフェースを決定的なレイアウトで扱う
- カーネル実行系で駆動できる Async COM サーフェスを提供する

## 主要コンポーネント

- `macros/`：インターフェース/VTable/shim を生成する宣言マクロ
- `wrapper`：`ComObject` / `ComObjectN`（IUnknown 実装と参照カウント管理）
- `VTable`：`ComInterfaceInfo` と VTable レイアウトマーカー
- `traits`：`ComImpl` と `query_interface` の契約
- `smart_ptr`：`ComRc<T>` と `ComInterface` マーカー
- `async_com`：`AsyncOperation` と spawn ヘルパー
- `executor`：DPC / Work-item 実行系 + ホストスタブ
- `allocator`：`Allocator`、`WdkAllocator`、`KBox`
- `unicode`：`UNICODE_STRING` ヘルパー（feature）
- `trace`：デバッグ用トレースフック

## オブジェクトレイアウト

### プライマリ (`ComObject<T, Vtbl>`)

`ComObject` は `repr(C)` の単一アロケーションです。

- 先頭にプライマリ VTable ポインタ（COM 期待）
- aggregation 用の non-delegating IUnknown
- 参照カウント
- 外側 IUnknown（オプション）
- inner Rust オブジェクト (`T`)
- アロケータ（Release で drop/dealloc）

この構成により「COM ポインタとしての互換性」と
「内部状態の近接配置」を両立します。

### 多重インターフェース (`ComObjectN<T, Primary, Secondaries>`)

`ComObjectN` は `secondaries` タプルを追加し、
各セカンダリ VTable と `this` 調整のための parent を保持します。

初期化時に:
- secondary VTable を shim に結び付け
- secondary entry の parent を設定

プライマリ VTable は常にオフセット 0 で保持されます。

## QueryInterface の流れ

`ComObject` / `ComObjectN` が IUnknown shim を提供します。

- `shim_query_interface` が IID を検証し、
  `ComImpl::query_interface` でカスタムの IID 対応を行う
- IID がプライマリの場合は `this` を返す
- 非プライマリは、IID に一致する VTable を持つ安定ポインタを返す
- 返したポインタには `AddRef` が適用される

`ComObjectN` では primary + secondaries の QI を自動生成できます。

## Aggregation

Aggregation では non-delegating IUnknown (NDI) を内包します。

- 通常の IUnknown 呼び出しは outer に委譲可能
- outer は NDI を保持して inner の寿命を管理
- `new_aggregated*` は raw outer IUnknown を受け取るため `unsafe`

## Async パイプライン（概要）

- Async メソッドは `AsyncOperationRaw<T>` を返す
- shim が `InitBox` で Future を生成
- Executor にタスクを登録し結果を格納
- `AsyncOperationRaw` が `get_status` / `get_result` を提供

## キャンセルとトラッキング

`async_com` は Executor を通じて駆動されます。

- DPC Executor（DISPATCH_LEVEL）
- Work-item Executor（PASSIVE_LEVEL, WDM）
- `CancelHandle` / `WorkItemCancelHandle`
- `TaskTracker` / `WorkItemTracker`

キャンセル判定は CPU ごとのテーブルを参照します。
CPU インデックスが範囲外の場合はデバッグ trace を出し、追跡を無効化します。

## アロケータ

`Allocator` は `alloc / alloc_zeroed / dealloc` の最小構成です。

- `GlobalAllocator` がデフォルト
  - ドライバ: `WdkAllocator` + `NonPagedNx`
  - ホスト: `alloc::alloc`
- `WdkAllocator` は `wdk-alloc-align` で over-aligned をサポート

## Unicode ユーティリティ

対象 feature: `kernel-unicode`

以下を提供:

- `OwnedUnicodeString`（ヒープ）
- `LocalUnicodeString<N>`（スタック）
- `kstr!` 静的 literal

## 診断

`trace` はデバッグトレースを差し込める軽量フックです。
`ensure!` マクロは file/line を添えたメッセージを出力できます。

