# Executor

`executor` モジュールは 2 種類の実行モデルを提供します。

- DPC 実行（DISPATCH_LEVEL）
- Work-item 実行（PASSIVE_LEVEL / WDM）

ホスト/Miri ではスタブ実装が使われ、Future を 1 回だけ poll します。

## DPC Executor

`driver + async-com-kernel` で有効（Miri ではスタブ）。

API:

- `spawn_dpc_task` / `spawn_dpc_task_tracked`
- `spawn_dpc_task_cancellable` / `spawn_dpc_task_cancellable_tracked`
- `CancelHandle`, `TaskTracker`
- `is_cancellation_requested`, `take_cancellation_request`

安全性:

- DPC は `DISPATCH_LEVEL` で実行されるため **unsafe**
- pageable メモリやブロッキング API の利用は禁止
- Async ステートマシン内で大きなスタック変数を避ける

キャンセル:

- DPC タスクはキャンセルフラグを保持
- `take_cancellation_request` は 1 回だけ true を返す
- `try_finally` でクリーンアップを安全に走らせる

CPU インデックス:

- CPU ごとのテーブルでキャンセル状態を管理
- group/number から index を算出
- 範囲外の場合はデバッグ trace を出し、追跡を無効化

## Work-item Executor (WDM)

`driver + async-com-kernel + driver_model__driver_type=WDM` で有効。

API:

- `spawn_task` / `spawn_task_tracked`
- `spawn_task_cancellable` / `spawn_task_cancellable_tracked`
- `WorkItemTracker`, `WorkItemCancelHandle`

動作:

- PASSIVE_LEVEL で実行
- `device` は **必ず非 null**
  - null の場合は `STATUS_INVALID_PARAMETER` を返す
- `WorkItemTracker::drain` で unload 前に処理を待つ

## ホスト/Miri スタブ

ホスト/Miri では:

- Future を 1 回だけ poll
- 即座に戻る

完了型のテストには十分ですが、完全なスケジューリングではありません。

