# kcom Documentation

This directory contains design and usage documentation for the `kcom` crate.
It complements the project `README.md` with deeper architecture notes, safety
boundaries, and kernel‑specific constraints.

## Contents

- `architecture.md` — High‑level component overview and object layout.
- `macros.md` — Macro APIs (`declare_com_interface!`, `impl_com_interface!`, etc.).
- `async.md` — Async COM surface, `AsyncOperation`, cancellation, host vs driver.
- `executor.md` — DPC/work‑item executors, IRQL rules, cancellation tracking.
- `allocator.md` — `Allocator` trait, `WdkAllocator`, alignment, OOM handling.
- `unicode.md` — `UNICODE_STRING` helpers, `OwnedUnicodeString`, `LocalUnicodeString`.
- `safety.md` — Unsafe boundaries, aggregation rules, panic handling, refcount hardening.
- `testing.md` — Test matrix, `test_all.ps1`, Miri usage, driver stubs.
- `benchmarks.md` — Benchmark layout and interpretation pointers.
- `troubleshooting.md` — Common build/link/Miri issues.

## Related project docs

- `README.md` — Top‑level overview and quick usage examples.
- `SmartVtbl.md` — Smart VTable design and usage.
- `Example.md` — Example code and macro expansion mapping.
- `Benchmark.md` — Recorded benchmark numbers.
- `NOTES.md` — Internal notes (trade‑offs, Miri/provenance policies).
- `FIND.md` — Review findings and architectural assessment.

## Feature flags (quick map)

See `README.md` for details. Most docs below annotate feature requirements
inline, e.g. `driver`, `async-com`, `async-com-kernel`, `kernel-unicode`,
`refcount-hardening`, and `wdk-alloc-align`.

