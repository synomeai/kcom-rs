#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), feature(alloc_error_handler))]
#![allow(non_snake_case)]

#[cfg(test)]
extern crate std;

mod lib_shared;
mod hsmr;

pub use lib_shared::*;
