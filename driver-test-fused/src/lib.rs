#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), feature(alloc_error_handler))]
#![allow(non_snake_case)]

#[cfg(test)]
extern crate std;

#[path = "../../driver-test/src/lib_shared.rs"]
mod lib_shared;
#[path = "../../driver-test/src/hsmr.rs"]
mod hsmr;

pub use lib_shared::*;
