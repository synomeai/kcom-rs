// driver_test_stub.rs
//
// Stub DriverEntry for user-mode test builds with driver features enabled.

#![cfg(all(feature = "driver", any(test, feature = "driver-test-stub")))]

use core::ffi::c_void;

use crate::iunknown::{NTSTATUS, STATUS_SUCCESS};

#[no_mangle]
pub extern "system" fn DriverEntry(
    _driver_object: *mut c_void,
    _registry_path: *mut c_void,
) -> NTSTATUS {
    STATUS_SUCCESS
}
