use core::alloc::Layout;
use core::convert::TryFrom;
use core::ffi::c_void;
use core::mem::size_of;
#[cfg(not(test))]
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use crate::hsmr;

use kcom::allocator::{Allocator, PoolType, WdkAllocator};
use kcom::irp::{build_async_fsd_request, IrpFuture, IrpFutureOptions, IrpResult, IrpWakeMode};
#[cfg(driver_model__driver_type = "WDM")]
use kcom::spawn_task;
#[cfg(feature = "feature-mdl")]
use kcom::mdl::alloc_nonpaged_mdl;
use kcom::mdl::{free_mdl, MdlKind};
use kcom::STATUS_INVALID_PARAMETER;
use kcom_tests::bench_async_com::{
    run_async_com_bench,
    run_selected_bench,
    BenchConfig,
    BenchResult,
    BenchScenario,
};

use wdk_sys::{
    ACCESS_MASK, CCHAR, DEVICE_OBJECT, DRIVER_OBJECT, HANDLE, IO_NO_INCREMENT, IO_STACK_LOCATION,
    IO_STATUS_BLOCK, IRP, IRP_MJ_CLOSE, IRP_MJ_CREATE, IRP_MJ_DEVICE_CONTROL, IRP_MJ_READ,
    IRP_MJ_WRITE, KIRQL, LARGE_INTEGER, NTSTATUS, OBJECT_ATTRIBUTES, PASSIVE_LEVEL, PFILE_OBJECT,
    PMDL, STATUS_BUFFER_TOO_SMALL, STATUS_CANCELLED, STATUS_INSUFFICIENT_RESOURCES,
    STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_DEVICE_STATE, STATUS_PENDING, STATUS_SUCCESS,
    UNICODE_STRING, ULONG_PTR, DO_BUFFERED_IO, DO_DEVICE_INITIALIZING, FILE_ATTRIBUTE_NORMAL,
    FILE_DEVICE_UNKNOWN, FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_READ_DATA, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_SYNCHRONOUS_IO_NONALERT, FILE_WRITE_DATA, GENERIC_READ, GENERIC_WRITE,
    METHOD_BUFFERED, OBJ_CASE_INSENSITIVE, OBJ_KERNEL_HANDLE, SYNCHRONIZE, IoFileObjectType, _MODE,
};
use wdk_sys::ntddk::{
    IoAcquireCancelSpinLock, IoCancelIrp, IofCompleteRequest, IoCreateDevice,
    IoCreateSymbolicLink, IoDeleteDevice, IoDeleteSymbolicLink, IoFreeIrp,
    IoGetRelatedDeviceObject, IoReleaseCancelSpinLock, KeGetCurrentIrql,
    ObReferenceObjectByHandle, ObfDereferenceObject, PsCreateSystemThread, ZwClose, ZwCreateFile,
    ZwReadFile, ZwWriteFile,
};

#[cfg(not(test))]
use wdk_sys::ntddk::KeBugCheckEx;

#[cfg(not(feature = "bench-async-com"))]
compile_error!("driver-test requires the bench-async-com feature");

const DRIVER_TAG: u32 = u32::from_ne_bytes(*b"BCDR");
const IOCTL_KCOM_BENCH: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA);
const IOCTL_KCOM_IO_ECHO: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA);
const IOCTL_KCOM_IO_FILE: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA);
const IOCTL_KCOM_IO_FILE_ASYNC: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x803, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA);
const IOCTL_KCOM_HSMR_INIT: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x820, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA);
const IOCTL_KCOM_HSMR_SHUTDOWN: u32 =
    ctl_code(FILE_DEVICE_UNKNOWN, 0x821, METHOD_BUFFERED, FILE_READ_DATA | FILE_WRITE_DATA);

#[repr(C)]
struct BenchRequest {
    scenario: u32,
    yield_count: u32,
    iterations: u64,
    parallelism: u64,
    warmup_iterations: u64,
}

#[repr(C)]
struct BenchRequestV1 {
    scenario: u32,
    yield_count: u32,
    iterations: u64,
    parallelism: u64,
}

#[repr(C)]
struct IoFileRequest {
    op: u32,
    size: u32,
    offset: u64,
}

#[repr(C)]
struct IoFileAsyncContext {
    irp: *mut IRP,
    io_irp: AtomicPtr<IRP>,
    file_object: PFILE_OBJECT,
    target_device: *mut DEVICE_OBJECT,
    op: u32,
    buffer_len: u32,
    offset: LARGE_INTEGER,
    io_status: IO_STATUS_BLOCK,
    refs: AtomicU32,
    state: AtomicU32,
    cancel_requested: AtomicU32,
}


const ASYNC_STATE_INIT: u32 = 0;
const ASYNC_STATE_DISPATCHING: u32 = 1;
const ASYNC_STATE_DISPATCHED: u32 = 2;
const ASYNC_STATE_DONE: u32 = 3;

#[cfg(feature = "async-com-fused")]
const DEVICE_NAME: &[u16] = &[
    0x005C, 0x0044, 0x0065, 0x0076, 0x0069, 0x0063, 0x0065, 0x005C, 0x004B, 0x0063, 0x006F,
    0x006D, 0x0042, 0x0065, 0x006E, 0x0063, 0x0068, 0x0046, 0x0075, 0x0073, 0x0065, 0x0064,
    0x0000,
];
#[cfg(not(feature = "async-com-fused"))]
const DEVICE_NAME: &[u16] = &[
    0x005C, 0x0044, 0x0065, 0x0076, 0x0069, 0x0063, 0x0065, 0x005C, 0x004B, 0x0063, 0x006F,
    0x006D, 0x0042, 0x0065, 0x006E, 0x0063, 0x0068, 0x0000,
];

#[cfg(feature = "async-com-fused")]
const SYMLINK_NAME: &[u16] = &[
    0x005C, 0x003F, 0x003F, 0x005C, 0x004B, 0x0063, 0x006F, 0x006D, 0x0042, 0x0065, 0x006E,
    0x0063, 0x0068, 0x0046, 0x0075, 0x0073, 0x0065, 0x0064, 0x0000,
];
#[cfg(not(feature = "async-com-fused"))]
const SYMLINK_NAME: &[u16] = &[
    0x005C, 0x003F, 0x003F, 0x005C, 0x004B, 0x0063, 0x006F, 0x006D, 0x0042, 0x0065, 0x006E,
    0x0063, 0x0068, 0x0000,
];

static mut DEVICE_OBJECT_PTR: *mut DEVICE_OBJECT = ptr::null_mut();
static IO_FILE_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static IO_FILE_HANDLE_ASYNC: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

const IO_FILE_PATH: &[u16] = &[
    0x005C, 0x003F, 0x003F, 0x005C, 0x0043, 0x003A, 0x005C, 0x0044, 0x0072, 0x0069, 0x0076,
    0x0065, 0x0072, 0x005C, 0x006B, 0x0063, 0x006F, 0x006D, 0x005F, 0x0069, 0x006F, 0x005F,
    0x0074, 0x0065, 0x0073, 0x0074, 0x002E, 0x0062, 0x0069, 0x006E, 0x0000,
];

struct KernelGlobalAlloc;

unsafe impl core::alloc::GlobalAlloc for KernelGlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { WdkAllocator::new(PoolType::NonPagedNx, DRIVER_TAG).alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { WdkAllocator::new(PoolType::NonPagedNx, DRIVER_TAG).dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: KernelGlobalAlloc = KernelGlobalAlloc;

#[cfg(not(test))]
#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    unsafe {
        KeBugCheckEx(
            0x4B43_4F4D,
            layout.size() as u64,
            layout.align() as u64,
            0,
            0,
        )
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic_handler(_info: &PanicInfo) -> ! {
    unsafe { KeBugCheckEx(0x4B43_4F4D, 0x5041_4E49, 0, 0, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn DriverEntry(
    driver: *mut DRIVER_OBJECT,
    _registry: *mut UNICODE_STRING,
) -> i32 {
    if driver.is_null() {
        return STATUS_INVALID_DEVICE_REQUEST;
    }

    unsafe {
        let mut device = ptr::null_mut();
        let dev_name = make_unicode_string(DEVICE_NAME);
        let status = IoCreateDevice(
            driver,
            0,
            &dev_name as *const UNICODE_STRING as *mut UNICODE_STRING,
            FILE_DEVICE_UNKNOWN,
            0,
            0,
            &mut device,
        );
        if status < 0 {
            return status;
        }

        let sym_name = make_unicode_string(SYMLINK_NAME);
        let status = IoCreateSymbolicLink(
            &sym_name as *const UNICODE_STRING as *mut UNICODE_STRING,
            &dev_name as *const UNICODE_STRING as *mut UNICODE_STRING,
        );
        if status < 0 {
            IoDeleteDevice(device);
            return status;
        }

        (*device).Flags |= DO_BUFFERED_IO;
        (*device).Flags &= !DO_DEVICE_INITIALIZING;

        (*driver).MajorFunction[IRP_MJ_CREATE as usize] = Some(dispatch_create_close);
        (*driver).MajorFunction[IRP_MJ_CLOSE as usize] = Some(dispatch_create_close);
        (*driver).MajorFunction[IRP_MJ_DEVICE_CONTROL as usize] = Some(dispatch_device_control);
        (*driver).DriverUnload = Some(driver_unload);

        DEVICE_OBJECT_PTR = device;
        STATUS_SUCCESS
    }
}

unsafe extern "C" fn driver_unload(_driver: *mut DRIVER_OBJECT) {
    unsafe {
        close_io_file_handle();
        close_io_file_handle_async();
        hsmr::handle_driver_unload();

        let sym_name = make_unicode_string(SYMLINK_NAME);
        let _ = IoDeleteSymbolicLink(&sym_name as *const UNICODE_STRING as *mut UNICODE_STRING);

        let device = DEVICE_OBJECT_PTR;
        if !device.is_null() {
            IoDeleteDevice(device);
            DEVICE_OBJECT_PTR = ptr::null_mut();
        }
    }
}

unsafe extern "C" fn dispatch_create_close(_device: *mut DEVICE_OBJECT, irp: *mut IRP) -> i32 {
    unsafe { complete_irp(irp, STATUS_SUCCESS, 0) }
}

unsafe extern "C" fn dispatch_device_control(
    device: *mut DEVICE_OBJECT,
    irp: *mut IRP,
) -> i32 {
    let irql = unsafe { KeGetCurrentIrql() };
    if irql != PASSIVE_LEVEL as KIRQL {
        return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_STATE, 0) };
    }

    let stack = unsafe { io_get_current_irp_stack_location(irp) };
    if stack.is_null() {
        return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) };
    }

    let code = unsafe { (*stack).Parameters.DeviceIoControl.IoControlCode };
    let in_len = unsafe { (*stack).Parameters.DeviceIoControl.InputBufferLength as usize };
    let out_len = unsafe { (*stack).Parameters.DeviceIoControl.OutputBufferLength as usize };
    let buffer = unsafe { (*irp).AssociatedIrp.SystemBuffer as *mut u8 };

    match code {
        IOCTL_KCOM_BENCH => {
            if out_len < size_of::<BenchResult>() {
                return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
            }
            if buffer.is_null() {
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) };
            }

            let result = if in_len >= size_of::<BenchRequest>() {
                let req = unsafe { &*(buffer as *const BenchRequest) };
                match build_bench_config(
                    req.scenario,
                    req.yield_count,
                    req.iterations,
                    req.parallelism,
                    req.warmup_iterations,
                ) {
                    Ok(config) => match unsafe { run_async_com_bench(config) } {
                        Ok(result) => result,
                        Err(status) => return unsafe { complete_irp(irp, status, 0) },
                    },
                    Err(status) => return unsafe { complete_irp(irp, status, 0) },
                }
            } else if in_len >= size_of::<BenchRequestV1>() {
                let req = unsafe { &*(buffer as *const BenchRequestV1) };
                match build_bench_config(
                    req.scenario,
                    req.yield_count,
                    req.iterations,
                    req.parallelism,
                    0,
                ) {
                    Ok(config) => match unsafe { run_async_com_bench(config) } {
                        Ok(result) => result,
                        Err(status) => return unsafe { complete_irp(irp, status, 0) },
                    },
                    Err(status) => return unsafe { complete_irp(irp, status, 0) },
                }
            } else {
                match unsafe { run_selected_bench() } {
                    Ok(result) => result,
                    Err(status) => return unsafe { complete_irp(irp, status, 0) },
                }
            };

            let out_ptr = buffer as *mut BenchResult;
            unsafe { out_ptr.write(result) };
            unsafe { complete_irp(irp, STATUS_SUCCESS, size_of::<BenchResult>()) }
        }
        IOCTL_KCOM_IO_ECHO => {
            if buffer.is_null() {
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) };
            }
            let info = if out_len < in_len { out_len } else { in_len };
            unsafe { complete_irp(irp, STATUS_SUCCESS, info) }
        }
        IOCTL_KCOM_IO_FILE => {
            if buffer.is_null() {
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) };
            }
            if in_len < size_of::<IoFileRequest>() {
                return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
            }

            let req = unsafe { &*(buffer as *const IoFileRequest) };
            let size = req.size as usize;
            let total = match size_of::<IoFileRequest>().checked_add(size) {
                Some(total) => total,
                None => return unsafe { complete_irp(irp, STATUS_INVALID_PARAMETER, 0) },
            };

            let offset = match i64::try_from(req.offset) {
                Ok(value) => value,
                Err(_) => return unsafe { complete_irp(irp, STATUS_INVALID_PARAMETER, 0) },
            };

            let handle = match unsafe { get_io_file_handle() } {
                Ok(handle) => handle,
                Err(status) => return unsafe { complete_irp(irp, status, 0) },
            };

            let mut io_status: IO_STATUS_BLOCK = unsafe { core::mem::zeroed() };
            let mut byte_offset = LARGE_INTEGER { QuadPart: offset };
            let payload = unsafe { buffer.add(size_of::<IoFileRequest>()) } as *mut c_void;

            match req.op {
                0 => {
                    if out_len < total {
                        return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
                    }
                    let status = unsafe {
                        ZwReadFile(
                            handle,
                            ptr::null_mut(),
                            None,
                            ptr::null_mut(),
                            &mut io_status as *mut IO_STATUS_BLOCK,
                            payload,
                            req.size,
                            &mut byte_offset as *mut LARGE_INTEGER,
                            ptr::null_mut(),
                        )
                    };
                    if status < 0 {
                        return unsafe { complete_irp(irp, status, 0) };
                    }
                    let bytes = io_status.Information as usize;
                    let info = size_of::<IoFileRequest>().saturating_add(bytes);
                    unsafe { complete_irp(irp, STATUS_SUCCESS, info) }
                }
                1 => {
                    if in_len < total {
                        return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
                    }
                    let status = unsafe {
                        ZwWriteFile(
                            handle,
                            ptr::null_mut(),
                            None,
                            ptr::null_mut(),
                            &mut io_status as *mut IO_STATUS_BLOCK,
                            payload,
                            req.size,
                            &mut byte_offset as *mut LARGE_INTEGER,
                            ptr::null_mut(),
                        )
                    };
                    if status < 0 {
                        return unsafe { complete_irp(irp, status, 0) };
                    }
                    unsafe { complete_irp(irp, STATUS_SUCCESS, 0) }
                }
                _ => unsafe { complete_irp(irp, STATUS_INVALID_PARAMETER, 0) },
            }
        }
        IOCTL_KCOM_IO_FILE_ASYNC => {
            if buffer.is_null() {
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) };
            }
            if in_len < size_of::<IoFileRequest>() {
                return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
            }

            let req = unsafe { &*(buffer as *const IoFileRequest) };
            let size = req.size as usize;
            let total = match size_of::<IoFileRequest>().checked_add(size) {
                Some(total) => total,
                None => return unsafe { complete_irp(irp, STATUS_INVALID_PARAMETER, 0) },
            };

            let offset = match i64::try_from(req.offset) {
                Ok(value) => value,
                Err(_) => return unsafe { complete_irp(irp, STATUS_INVALID_PARAMETER, 0) },
            };

            match req.op {
                0 => {
                    if out_len < total {
                        return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
                    }
                }
                1 => {
                    if in_len < total {
                        return unsafe { complete_irp(irp, STATUS_BUFFER_TOO_SMALL, 0) };
                    }
                }
                _ => return unsafe { complete_irp(irp, STATUS_INVALID_PARAMETER, 0) },
            }

            let handle = match unsafe { get_io_file_handle_async() } {
                Ok(handle) => handle,
                Err(status) => return unsafe { complete_irp(irp, status, 0) },
            };

            if device.is_null() {
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_STATE, 0) };
            }

            let alloc = WdkAllocator::new(PoolType::NonPagedNx, DRIVER_TAG);
            let ctx_layout = Layout::new::<IoFileAsyncContext>();
            let ctx_ptr = unsafe { alloc.alloc(ctx_layout) } as *mut IoFileAsyncContext;
            if ctx_ptr.is_null() {
                return unsafe { complete_irp(irp, STATUS_INSUFFICIENT_RESOURCES, 0) };
            }

            let buffer_len = size as u32;
            let buffer_base = unsafe { (*irp).AssociatedIrp.SystemBuffer as *mut u8 };
            let buffer_ptr = if size == 0 {
                ptr::null_mut()
            } else {
                unsafe { buffer_base.add(size_of::<IoFileRequest>()) }
            };

            let mut file_object: PFILE_OBJECT = ptr::null_mut();
            let status = unsafe {
                ObReferenceObjectByHandle(
                    handle,
                    (FILE_READ_DATA | FILE_WRITE_DATA | SYNCHRONIZE) as ACCESS_MASK,
                    *IoFileObjectType,
                    _MODE::KernelMode as i8,
                    &mut file_object as *mut _ as *mut *mut c_void,
                    ptr::null_mut(),
                )
            };
            if status < 0 {
                unsafe { alloc.dealloc(ctx_ptr as *mut u8, ctx_layout) };
                return unsafe { complete_irp(irp, status, 0) };
            }
            if file_object.is_null() {
                unsafe { alloc.dealloc(ctx_ptr as *mut u8, ctx_layout) };
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_STATE, 0) };
            }

            let target_device = unsafe { IoGetRelatedDeviceObject(file_object) };
            if target_device.is_null() {
                unsafe { ObfDereferenceObject(file_object as *mut c_void) };
                unsafe { alloc.dealloc(ctx_ptr as *mut u8, ctx_layout) };
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_STATE, 0) };
            }

            unsafe {
                core::ptr::write(
                    ctx_ptr,
                    IoFileAsyncContext {
                        irp,
                        io_irp: AtomicPtr::new(ptr::null_mut()),
                        file_object,
                        target_device,
                        op: req.op,
                        buffer_len,
                        offset: LARGE_INTEGER { QuadPart: offset },
                        io_status: core::mem::zeroed(),
                        refs: AtomicU32::new(1),
                        state: AtomicU32::new(ASYNC_STATE_INIT),
                        cancel_requested: AtomicU32::new(0),
                    },
                );
                set_irp_context(irp, ctx_ptr);
            }

            let major = if req.op == 0 { IRP_MJ_READ } else { IRP_MJ_WRITE };
            let io_irp = match unsafe {
                build_async_fsd_request(
                    major as u32,
                    target_device,
                    buffer_ptr as *mut c_void,
                    req.size,
                    &mut (*ctx_ptr).offset as *mut LARGE_INTEGER,
                    &mut (*ctx_ptr).io_status as *mut IO_STATUS_BLOCK,
                )
            } {
                Ok(irp) => irp,
                Err(status) => {
                    unsafe { set_irp_context(irp, ptr::null_mut()) };
                    unsafe { ctx_release(ctx_ptr) };
                    return unsafe { complete_irp(irp, status, 0) };
                }
            };

            #[cfg(feature = "feature-mdl")]
            let mut new_mdl: Option<PMDL> = None;
            unsafe {
                (*ctx_ptr).io_irp.store(io_irp, Ordering::Release);
                (*io_irp).Tail.Overlay.OriginalFileObject = file_object;
                (*io_irp).RequestorMode = _MODE::KernelMode as i8;
                #[cfg(feature = "feature-mdl")]
                {
                    new_mdl = alloc_nonpaged_mdl(buffer_ptr as *mut c_void, req.size);
                    if let Some(mdl) = new_mdl {
                        let old_mdl = (*io_irp).MdlAddress;
                        if !old_mdl.is_null() {
                            free_mdl(old_mdl, MdlKind::NonPaged);
                        }
                        (*io_irp).MdlAddress = mdl;
                    }
                }
                let io_stack = io_get_next_irp_stack_location(io_irp);
                (*io_stack).FileObject = file_object;
            }

            #[cfg(feature = "feature-mdl")]
            if new_mdl.is_none() {
                unsafe { set_irp_context(irp, ptr::null_mut()) };
                unsafe {
                    let old_mdl = (*io_irp).MdlAddress;
                    if !old_mdl.is_null() {
                        free_mdl(old_mdl, MdlKind::NonPaged);
                    }
                    IoFreeIrp(io_irp);
                    ctx_release(ctx_ptr);
                }
                return unsafe { complete_irp(irp, STATUS_INSUFFICIENT_RESOURCES, 0) };
            }

            unsafe {
                let mut cancel_irql: KIRQL = 0;
                IoAcquireCancelSpinLock(&mut cancel_irql as *mut KIRQL);
                if (*irp).Cancel != 0 {
                    (*irp).CancelRoutine = None;
                    IoReleaseCancelSpinLock(cancel_irql);
                    (*ctx_ptr).cancel_requested.store(1, Ordering::Release);
                    (*ctx_ptr).state.store(ASYNC_STATE_DONE, Ordering::Release);
                    set_irp_context(irp, ptr::null_mut());
                    free_io_mdl((*io_irp).MdlAddress);
                    IoFreeIrp(io_irp);
                    ctx_release(ctx_ptr);
                    return complete_irp(irp, STATUS_CANCELLED, 0);
                }
                (*irp).CancelRoutine = Some(io_file_async_cancel);
                (*ctx_ptr)
                    .state
                    .store(ASYNC_STATE_DISPATCHING, Ordering::Release);
                IoReleaseCancelSpinLock(cancel_irql);
            }

            unsafe {
                (*irp).PendingReturned = 1;
                (*irp).IoStatus.__bindgen_anon_1.Status = STATUS_PENDING;
                (*irp).IoStatus.Information = 0;
            }

            unsafe { ctx_add_ref(ctx_ptr) };
            let status = queue_io_file_async(device, ctx_ptr);
            if status < 0 {
                unsafe { ctx_release(ctx_ptr) };
                unsafe {
                    let mut cancel_irql: KIRQL = 0;
                    IoAcquireCancelSpinLock(&mut cancel_irql as *mut KIRQL);
                    (*irp).CancelRoutine = None;
                    IoReleaseCancelSpinLock(cancel_irql);
                }
                unsafe { set_irp_context(irp, ptr::null_mut()) };
                unsafe {
                    let io_irp = (*ctx_ptr).io_irp.swap(ptr::null_mut(), Ordering::AcqRel);
                    free_io_irp(io_irp);
                }
                unsafe { ctx_release(ctx_ptr) };
                return unsafe { complete_irp(irp, status, 0) };
            }
            unsafe { ctx_release(ctx_ptr) };
            STATUS_PENDING
        }
        IOCTL_KCOM_HSMR_INIT => {
            if buffer.is_null() {
                return unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) };
            }
            match unsafe { hsmr::handle_init(buffer, in_len, out_len) } {
                Ok(info) => unsafe { complete_irp(irp, STATUS_SUCCESS, info) },
                Err(status) => unsafe { complete_irp(irp, status, 0) },
            }
        }
        IOCTL_KCOM_HSMR_SHUTDOWN => {
            let status = unsafe { hsmr::handle_shutdown() };
            unsafe { complete_irp(irp, status, 0) }
        }
        _ => unsafe { complete_irp(irp, STATUS_INVALID_DEVICE_REQUEST, 0) },
    }
}

unsafe fn complete_irp(irp: *mut IRP, status: i32, info: usize) -> i32 {
    unsafe {
        (*irp).IoStatus.__bindgen_anon_1.Status = status;
        (*irp).IoStatus.Information = info as ULONG_PTR;
        IofCompleteRequest(irp, IO_NO_INCREMENT as CCHAR);
    }
    status
}

#[inline]
unsafe fn io_get_current_irp_stack_location(irp: *mut IRP) -> *mut IO_STACK_LOCATION {
    unsafe {
        (*irp)
            .Tail
            .Overlay
            .__bindgen_anon_2
            .__bindgen_anon_1
            .CurrentStackLocation
    }
}

#[inline]
unsafe fn io_get_next_irp_stack_location(irp: *mut IRP) -> *mut IO_STACK_LOCATION {
    unsafe { io_get_current_irp_stack_location(irp).offset(-1) }
}

#[inline]
unsafe fn set_irp_context(irp: *mut IRP, ctx: *mut IoFileAsyncContext) {
    unsafe {
        (*irp)
            .Tail
            .Overlay
            .__bindgen_anon_1
            .__bindgen_anon_1
            .DriverContext[0] = ctx as *mut c_void;
    }
}

#[inline]
unsafe fn get_irp_context(irp: *mut IRP) -> *mut IoFileAsyncContext {
    unsafe {
        (*irp)
            .Tail
            .Overlay
            .__bindgen_anon_1
            .__bindgen_anon_1
            .DriverContext[0] as *mut IoFileAsyncContext
    }
}

#[inline]
unsafe fn ctx_add_ref(ctx: *mut IoFileAsyncContext) {
    unsafe {
        (*ctx).refs.fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
unsafe fn ctx_release(ctx: *mut IoFileAsyncContext) {
    if unsafe { (*ctx).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        let file_object = unsafe { (*ctx).file_object };
        if !file_object.is_null() {
            unsafe { ObfDereferenceObject(file_object as *mut c_void) };
        }
        let alloc = WdkAllocator::new(PoolType::NonPagedNx, DRIVER_TAG);
        unsafe { alloc.dealloc(ctx as *mut u8, Layout::new::<IoFileAsyncContext>()) };
    }
}

#[inline]
unsafe fn free_io_mdl(mdl: PMDL) {
    unsafe { free_mdl(mdl as _, MdlKind::NonPaged) };
}

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

fn make_unicode_string(buf: &'static [u16]) -> UNICODE_STRING {
    let len = (buf.len().saturating_sub(1) * 2) as u16;
    UNICODE_STRING {
        Length: len,
        MaximumLength: len + 2,
        Buffer: buf.as_ptr() as *mut u16,
    }
}

unsafe fn close_io_file_handle() {
    let handle = IO_FILE_HANDLE.swap(ptr::null_mut(), Ordering::AcqRel);
    if !handle.is_null() {
        unsafe { let _ = ZwClose(handle as HANDLE); };
    }
}

unsafe fn close_io_file_handle_async() {
    let handle = IO_FILE_HANDLE_ASYNC.swap(ptr::null_mut(), Ordering::AcqRel);
    if !handle.is_null() {
        unsafe { let _ = ZwClose(handle as HANDLE); };
    }
}

unsafe fn get_io_file_handle() -> Result<HANDLE, i32> {
    unsafe {
        get_io_file_handle_with_options(
            &IO_FILE_HANDLE,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
        )
    }
}

unsafe fn get_io_file_handle_async() -> Result<HANDLE, i32> {
    unsafe {
        get_io_file_handle_with_options(
            &IO_FILE_HANDLE_ASYNC,
            FILE_NON_DIRECTORY_FILE,
        )
    }
}

unsafe fn get_io_file_handle_with_options(
    storage: &AtomicPtr<c_void>,
    options: u32,
) -> Result<HANDLE, i32> {
    let handle = storage.load(Ordering::Acquire);
    if !handle.is_null() {
        return Ok(handle as HANDLE);
    }

    let mut name = make_unicode_string(IO_FILE_PATH);
    let mut attrs = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: ptr::null_mut(),
        ObjectName: &mut name as *mut UNICODE_STRING,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE,
        SecurityDescriptor: ptr::null_mut(),
        SecurityQualityOfService: ptr::null_mut(),
    };

    let mut io_status: IO_STATUS_BLOCK = unsafe { core::mem::zeroed() };
    let mut handle_out: HANDLE = ptr::null_mut();
    let status = unsafe {
        ZwCreateFile(
            &mut handle_out as *mut HANDLE,
            (GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE) as ACCESS_MASK,
            &mut attrs as *mut OBJECT_ATTRIBUTES,
            &mut io_status as *mut IO_STATUS_BLOCK,
            ptr::null_mut(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_IF,
            options,
            ptr::null_mut(),
            0,
        )
    };
    if status < 0 {
        return Err(status);
    }
    if handle_out.is_null() {
        return Err(STATUS_INVALID_DEVICE_STATE);
    }

    match storage.compare_exchange(
        ptr::null_mut(),
        handle_out as *mut c_void,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(handle_out),
        Err(existing) => {
            unsafe { let _ = ZwClose(handle_out); };
            Ok(existing as HANDLE)
        }
    }
}

unsafe fn io_file_async_on_complete(context: *mut c_void, _io_irp: *mut IRP) {
    let ctx = context as *mut IoFileAsyncContext;
    if ctx.is_null() {
        return;
    }
    unsafe { (*ctx).io_irp.store(ptr::null_mut(), Ordering::Release) };
}

unsafe fn io_file_async_on_cleanup(context: *mut c_void) {
    let ctx = context as *mut IoFileAsyncContext;
    if ctx.is_null() {
        return;
    }
    unsafe { ctx_release(ctx) };
}

async unsafe fn io_file_async_run(ctx: *mut IoFileAsyncContext) -> NTSTATUS {
    if ctx.is_null() {
        return STATUS_INVALID_DEVICE_STATE;
    }

    let io_irp = unsafe { (*ctx).io_irp.load(Ordering::Acquire) };
    if io_irp.is_null() {
        let result = IrpResult {
            status: STATUS_INVALID_DEVICE_STATE,
            information: 0,
        };
        unsafe { io_file_async_finish(ctx, result) };
        unsafe { ctx_release(ctx) };
        return STATUS_SUCCESS;
    }

    let target_device = unsafe { (*ctx).target_device };
    if target_device.is_null() {
        unsafe { (*ctx).io_irp.store(ptr::null_mut(), Ordering::Release) };
        unsafe { free_io_irp(io_irp) };
        let result = IrpResult {
            status: STATUS_INVALID_DEVICE_STATE,
            information: 0,
        };
        unsafe { io_file_async_finish(ctx, result) };
        unsafe { ctx_release(ctx) };
        return STATUS_SUCCESS;
    }

    let mut opts = IrpFutureOptions::default();
    opts.context = ctx as *mut c_void;
    opts.on_complete = Some(io_file_async_on_complete);
    opts.on_cleanup = Some(io_file_async_on_cleanup);
    #[cfg(driver_model__driver_type = "WDM")]
    {
        opts.wake_mode = IrpWakeMode::WorkItem;
    }
    #[cfg(not(driver_model__driver_type = "WDM"))]
    {
        opts.wake_mode = IrpWakeMode::Direct;
    }

    unsafe { ctx_add_ref(ctx) };
    let future = match unsafe { IrpFuture::new(target_device, io_irp, opts) } {
        Ok(fut) => fut,
        Err(status) => {
            unsafe { ctx_release(ctx) };
            unsafe { (*ctx).io_irp.store(ptr::null_mut(), Ordering::Release) };
            unsafe { free_io_irp(io_irp) };
            let result = IrpResult {
                status,
                information: 0,
            };
            unsafe { io_file_async_finish(ctx, result) };
            unsafe { ctx_release(ctx) };
            return STATUS_SUCCESS;
        }
    };

    unsafe { (*ctx).state.store(ASYNC_STATE_DISPATCHED, Ordering::Release) };
    if unsafe { (*ctx).cancel_requested.load(Ordering::Acquire) != 0 } {
        unsafe { IoCancelIrp(io_irp) };
    }

    let result = future.await;
    unsafe { io_file_async_finish(ctx, result) };
    unsafe { ctx_release(ctx) };
    STATUS_SUCCESS
}

#[cfg(driver_model__driver_type = "WDM")]
fn queue_io_file_async(device: *mut DEVICE_OBJECT, ctx: *mut IoFileAsyncContext) -> NTSTATUS {
    if device.is_null() {
        return STATUS_INVALID_DEVICE_STATE;
    }
    let future = unsafe { io_file_async_run(ctx) };
    spawn_task(device, future)
}

#[cfg(not(driver_model__driver_type = "WDM"))]
fn queue_io_file_async(_device: *mut DEVICE_OBJECT, ctx: *mut IoFileAsyncContext) -> NTSTATUS {
    unsafe extern "C" fn io_file_async_thread(context: *mut c_void) {
        let ctx = context as *mut IoFileAsyncContext;
        let _ = unsafe { io_file_async_run(ctx) };
    }

    let mut thread_handle: HANDLE = ptr::null_mut();
    let status = unsafe {
        PsCreateSystemThread(
            &mut thread_handle as *mut HANDLE,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            Some(io_file_async_thread),
            ctx as *mut c_void,
        )
    };
    if status >= 0 && !thread_handle.is_null() {
        unsafe { let _ = ZwClose(thread_handle); };
    }
    status
}

unsafe fn io_file_async_finish(ctx: *mut IoFileAsyncContext, result: IrpResult) {
    if ctx.is_null() {
        return;
    }

    let prev = unsafe { (*ctx).state.swap(ASYNC_STATE_DONE, Ordering::AcqRel) };
    if prev == ASYNC_STATE_DONE {
        return;
    }

    let irp = unsafe { (*ctx).irp };
    if irp.is_null() {
        return;
    }

    let cancelled = unsafe { (*ctx).cancel_requested.load(Ordering::Acquire) } != 0;
    let status = if cancelled {
        STATUS_CANCELLED
    } else {
        result.status
    };

    let mut info = 0usize;
    if !cancelled && status >= 0 && unsafe { (*ctx).op } == 0 {
        let buf_len = unsafe { (*ctx).buffer_len as usize };
        let copy_len = if result.information > buf_len {
            buf_len
        } else {
            result.information
        };
        info = size_of::<IoFileRequest>().saturating_add(copy_len);
    }

    unsafe {
        let mut cancel_irql: KIRQL = 0;
        IoAcquireCancelSpinLock(&mut cancel_irql as *mut KIRQL);
        (*irp).CancelRoutine = None;
        IoReleaseCancelSpinLock(cancel_irql);
    }

    unsafe {
        (*irp).IoStatus.__bindgen_anon_1.Status = status;
        (*irp).IoStatus.Information = if cancelled { 0 } else { info as ULONG_PTR };
        IofCompleteRequest(irp, IO_NO_INCREMENT as CCHAR);
    }

    unsafe { set_irp_context(irp, ptr::null_mut()) };
}

unsafe fn free_io_irp(io_irp: *mut IRP) {
    if io_irp.is_null() {
        return;
    }
    unsafe {
        let old_mdl = (*io_irp).MdlAddress;
        if !old_mdl.is_null() {
            free_io_mdl(old_mdl);
        }
        IoFreeIrp(io_irp);
    }
}

unsafe extern "C" fn io_file_async_cancel(_device: *mut DEVICE_OBJECT, irp: *mut IRP) {
    let ctx = unsafe { get_irp_context(irp) };
    if ctx.is_null() {
        unsafe { IoReleaseCancelSpinLock((*irp).CancelIrql) };
        return;
    }

    unsafe { (*ctx).cancel_requested.store(1, Ordering::Release) };
    unsafe { (*irp).CancelRoutine = None };

    let state = unsafe { (*ctx).state.load(Ordering::Acquire) };
    if state == ASYNC_STATE_INIT {
        let won = unsafe {
            (*ctx)
                .state
                .compare_exchange(
                    ASYNC_STATE_INIT,
                    ASYNC_STATE_DONE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        };
        unsafe { IoReleaseCancelSpinLock((*irp).CancelIrql) };
        if won {
            let io_irp = unsafe { (*ctx).io_irp.swap(ptr::null_mut(), Ordering::AcqRel) };
            unsafe { free_io_irp(io_irp) };

            unsafe {
                (*irp).IoStatus.__bindgen_anon_1.Status = STATUS_CANCELLED;
                (*irp).IoStatus.Information = 0;
                IofCompleteRequest(irp, IO_NO_INCREMENT as CCHAR);
            }
            unsafe { set_irp_context(irp, ptr::null_mut()) };
            unsafe { ctx_release(ctx) };
        }
        return;
    }

    if state == ASYNC_STATE_DISPATCHING {
        unsafe { IoReleaseCancelSpinLock((*irp).CancelIrql) };
        return;
    }

    unsafe { IoReleaseCancelSpinLock((*irp).CancelIrql) };
    if state == ASYNC_STATE_DISPATCHED {
        let io_irp = unsafe { (*ctx).io_irp.load(Ordering::Acquire) };
        if !io_irp.is_null() {
            unsafe { IoCancelIrp(io_irp) };
        }
    }
}

fn build_bench_config(
    scenario: u32,
    yield_count: u32,
    iterations: u64,
    parallelism: u64,
    warmup_iterations: u64,
) -> Result<BenchConfig, i32> {
    let scenario = match scenario {
        0 => BenchScenario::Immediate,
        1 => BenchScenario::YieldOnce,
        2 => BenchScenario::YieldMany,
        _ => return Err(STATUS_INVALID_PARAMETER),
    };
    let iterations = match usize::try_from(iterations) {
        Ok(value) if value > 0 => value,
        _ => return Err(STATUS_INVALID_PARAMETER),
    };
    let parallelism = match usize::try_from(parallelism) {
        Ok(value) if value > 0 => value,
        _ => return Err(STATUS_INVALID_PARAMETER),
    };
    let warmup_iterations = match usize::try_from(warmup_iterations) {
        Ok(value) => value,
        _ => return Err(STATUS_INVALID_PARAMETER),
    };
    let yield_count = if yield_count == 0 { 1 } else { yield_count };

    Ok(BenchConfig {
        scenario,
        iterations,
        parallelism,
        yield_count,
        warmup_iterations,
    })
}
