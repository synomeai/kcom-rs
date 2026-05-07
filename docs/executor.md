# Executor

The executor module provides two execution models for async work:

- DPC-based executor (DISPATCH_LEVEL)
- Work-item executor (PASSIVE_LEVEL, WDM only)

Host/Miri builds use stubs that poll once to keep tests deterministic.

## DPC executor

Available with `driver + async-com-kernel` (not Miri).

APIs:

- `spawn_dpc_task` / `spawn_dpc_task_tracked`
- `spawn_dpc_task_cancellable` / `spawn_dpc_task_cancellable_tracked`
- `CancelHandle`, `TaskTracker`
- `is_cancellation_requested`, `take_cancellation_request`

Safety:

- DPC runs at `DISPATCH_LEVEL`. Futures must not touch pageable memory or
  blocking kernel APIs.
- Large stack locals in `async fn` are dangerous. Use heap allocation for
  large buffers.

Cancellation:

- Each DPC task records a cancellation flag.
- `take_cancellation_request` returns true once per request and marks it
  acknowledged.
- `try_finally` combines a main future with async cleanup.

Budgeting:

- The DPC executor enforces a per-run budget.
- Default mode is poll-count based (64 polls per run).
- Use `set_task_budget(TaskBudget::Polls(n))` or
  `set_task_budget(TaskBudget::TimeUs(us))` to configure poll-based or
  time-based limits.

CPU indexing:

- DPC cancellation tracking uses a per-CPU table.
- CPU index is derived from processor group and number.
- If an index is out of range, debug builds emit a trace and cancellation
  tracking is disabled for that CPU.

## Work-item executor (WDM)

Available with `driver + async-com-kernel + driver_model__driver_type=WDM`.

APIs:

- `spawn_task` / `spawn_task_tracked`
- `spawn_task_cancellable` / `spawn_task_cancellable_tracked`
- `WorkItemTracker`, `WorkItemCancelHandle`

Behavior:

- Work items execute at PASSIVE_LEVEL.
- `device` must be non-null; null is rejected with `STATUS_INVALID_PARAMETER`.
- `WorkItemTracker::drain` should be used during driver unload to ensure all
  work is complete before freeing device objects.

## Host/Miri stubs

In non-driver or Miri builds, the executor:

- Polls each future once.
- Returns immediately.

This is sufficient for tests that complete immediately or check partial state.
It does not provide full async scheduling.

