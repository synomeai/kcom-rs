# Testing

This project uses a mix of unit tests, integration tests, and Miri runs.

## Test layout

- `src/*` unit tests (lib tests)
- `tests/*` integration tests
- `kcom-tests/` workspace member for higher-level, realistic usage tests

## Miri test runner

`scripts/test_all.ps1` runs Miri across multiple feature sets:

- default
- async-com
- kernel-unicode
- refcount-hardening
- combo (async-com + kernel-unicode + refcount-hardening)
- wdk-alloc-align (driver + wdk-alloc-align + driver-test-stub)
- driver-miri (driver + async-com-kernel + driver-test-stub)

## Driver stubs

The `driver-test-stub` feature provides a minimal `DriverEntry` so that driver
feature builds can link in user-mode test environments.

## Host vs driver test expectations

Host/Miri executor stubs poll once. Tests that require a full kernel executor
must be marked ignored or run in a real driver environment.

## Suggested commands

```text
cargo test
cargo +nightly miri test -p kcom-tests --features "async-com"
cargo +nightly miri test -p kcom-tests --features "driver async-com-kernel driver-test-stub"
```

