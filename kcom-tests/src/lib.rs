// Test-only crate for exercising kcom from a host environment.
#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(all(
    feature = "bench-async-com",
    feature = "driver",
    feature = "async-com-kernel",
    not(miri)
))]
pub mod bench_async_com;
