# Benchmarks

Benchmark sources live in `benches/`:

- `comparison.rs` / `comparison.cpp` (sync comparison)
- `comparison_async.rs` / `comparison_async.cpp` (async comparison)
- `async_benchmark.rs` (criterion benchmark, async-com feature)

## Running (Rust)

```text
cargo bench --bench comparison
cargo bench --bench comparison_async --features async-com
```

## Running (C++)

The C++ benchmarks build to `benches/*.exe`. Run from the repo root:

```text
.\benches\comparison.exe
.\benches\comparison_async.exe
```

## Interpretation guidance

- Always report the environment: CPU, power plan, build profile.
- Consider subtracting empty-loop overhead to get adjusted values.
- For kernel-oriented paths (DPC or WDK calls), document any kernel API cost
  that dominates the timing.

Recorded numbers live in `Benchmark.md`.

## Kernel async COM bench (kcom-tests)

`kcom-tests` exposes a small kernel-only benchmark helper that can be embedded
into a driver and used to compare `async-com` vs `async-com-fused`.

Build-time knobs (choose one scenario):
- `bench-scenario-immediate`
- `bench-scenario-yield1`
- `bench-scenario-yieldN`

Iteration/parallelism presets (optional, defaults are medium/4):
- `bench-iter-small` | `bench-iter-medium` | `bench-iter-large`
- `bench-par-1` | `bench-par-4` | `bench-par-16`

Optional overrides (compile-time env vars):
- `KCOM_BENCH_ITERS`
- `KCOM_BENCH_PAR`
- `KCOM_BENCH_YIELDS`

Example build (fused enabled, 1-yield, custom size):
```text
set KCOM_BENCH_ITERS=200000
set KCOM_BENCH_PAR=8
cargo build -p kcom-tests --features "bench-async-com bench-scenario-yield1 async-com-fused"
```

Call `kcom_tests::bench_async_com::run_selected_bench()` from a driver
entrypoint at PASSIVE_LEVEL and report the returned `BenchResult` fields.
