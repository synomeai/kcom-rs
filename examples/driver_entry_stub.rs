// Stub DriverEntry for example binaries when building with driver features.

#[cfg(feature = "driver")]
#[no_mangle]
pub extern "system" fn DriverEntry(
    _driver_object: *mut core::ffi::c_void,
    _registry_path: *mut core::ffi::c_void,
) -> kcom::NTSTATUS {
    kcom::STATUS_SUCCESS
}

#[allow(dead_code)]
fn main() {}
