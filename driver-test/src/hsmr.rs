use core::alloc::Layout;
use core::ffi::c_void;
use core::future::Future;
use core::pin::Pin;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use kcom::allocator::{Allocator, PoolType, WdkAllocator};
use kcom::irp::{build_async_fsd_request, IrpFuture, IrpFutureOptions, IrpResult, IrpWakeMode};
use kcom::STATUS_INVALID_PARAMETER;

use wdk_sys::{
    ACCESS_MASK, DEVICE_OBJECT, EVENT_MODIFY_STATE, FILE_ATTRIBUTE_NORMAL, FILE_NO_INTERMEDIATE_BUFFERING,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_DATA, FILE_WRITE_THROUGH, GENERIC_READ, GENERIC_WRITE, HANDLE, IO_STACK_LOCATION,
    IO_STATUS_BLOCK, IRP, IRP_MJ_READ, IRP_MJ_WRITE, KEVENT, LARGE_INTEGER,
    NTSTATUS, OBJ_CASE_INSENSITIVE, OBJ_KERNEL_HANDLE, OBJECT_ATTRIBUTES, PFILE_OBJECT, PVOID,
    STATUS_BUFFER_TOO_SMALL, STATUS_DEVICE_BUSY, STATUS_INSUFFICIENT_RESOURCES,
    STATUS_INVALID_DEVICE_STATE, STATUS_PENDING, STATUS_SUCCESS, SYNCHRONIZE, UNICODE_STRING,
    MdlMappingNoExecute, IoFileObjectType, KSPIN_LOCK, PEPROCESS,
    PPS_CREATE_NOTIFY_INFO, _MODE,
};
use wdk_sys::ntddk::{
    IoAllocateMdl, IoFreeIrp, IoFreeMdl, IoGetCurrentProcess, IoGetRelatedDeviceObject,
    KeClearEvent, KeDelayExecutionThread, KeInitializeEvent, KeSetEvent,
    KeStackAttachProcess, KeUnstackDetachProcess, KeWaitForSingleObject,
    KeAcquireSpinLockRaiseToDpc, KeReleaseSpinLock, KeInitializeSpinLock,
    MmAllocatePagesForMdl, MmBuildMdlForNonPagedPool, MmFreePagesFromMdl,
    MmMapLockedPagesSpecifyCache, MmUnmapLockedPages,
    ObReferenceObjectByHandle, ObfDereferenceObject, ObfReferenceObject,
    PsCreateSystemThread, PsGetProcessExitStatus, PsSetCreateProcessNotifyRoutineEx,
    ZwClose, ZwCreateFile, ZwWaitForSingleObject,
};
use wdk_sys::{_MEMORY_CACHING_TYPE, _MM_PAGE_PRIORITY, KAPC_STATE, PKEVENT, PETHREAD, PRKPROCESS};
use wdk_sys::_KWAIT_REASON::Executive;

const HSMR_PACKET_BYTES: usize = 64;
const HSMR_RING_SIZE_MVP: usize = 4096;
const HSMR_RING_SIZE_LARGE: usize = 32768;
const HSMR_OVERFLOW_ENTRIES: usize = 1024;
const HSMR_STATUS_RUNNING: u32 = 0;
const HSMR_STATUS_SLEEPING: u32 = 1;

const HSMR_MODE_MVP: u32 = 0;
const HSMR_MODE_LARGE: u32 = 1;

const HSMR_VERSION_V1: u32 = 1;
const HSMR_VERSION_V2: u32 = 2;
const HSMR_INIT_FLAG_PAYLOAD: u32 = 0x1;
const HSMR_INIT_FLAG_FILE_WRITE_THROUGH: u32 = 0x2;
const HSMR_INIT_FLAG_FILE_NO_BUFFERING: u32 = 0x4;
const HSMR_FLAG_HAS_PAYLOAD: u32 = 0x2;

const HSMR_IO_SLOT_BYTES: usize = 64 * 1024;
const HSMR_IO_SLOTS: usize = 256;
const HSMR_FILE_IO_ALIGN: usize = 4096;

const SPIN_LIMIT: u32 = 10_000;

#[repr(C, align(64))]
struct HsmrControl {
    head: AtomicU32,
    kernel_status: AtomicU32,
    _pad0: [u8; 56],
    tail: AtomicU32,
    _pad1: [u8; 60],
    overflow_head: AtomicU32,
    overflow_tail: AtomicU32,
    _pad2: [u8; 56],
}

#[repr(C)]
struct HsmrInitRequestV1 {
    version: u32,
    mode: u32,
    doorbell_event: HANDLE,
    space_event: HANDLE,
}

#[repr(C)]
struct HsmrInitRequestV2 {
    version: u32,
    mode: u32,
    doorbell_event: HANDLE,
    space_event: HANDLE,
    flags: u32,
    _reserved: u32,
}

#[repr(C)]
struct HsmrInitResponseV1 {
    user_ptr: *mut c_void,
    control_offset: u32,
    ring_offset: u32,
    ring_entries: u32,
    packet_bytes: u32,
    overflow_offset: u32,
    overflow_entries: u32,
    flags: u32,
    total_bytes: u32,
}

#[repr(C)]
struct HsmrInitResponseV2 {
    user_ptr: *mut c_void,
    control_offset: u32,
    ring_offset: u32,
    ring_entries: u32,
    packet_bytes: u32,
    overflow_offset: u32,
    overflow_entries: u32,
    flags: u32,
    total_bytes: u32,
    payload_offset: u32,
    payload_slot_bytes: u32,
    payload_slots: u32,
    _reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HsmrPacket {
    op: u32,
    size: u32,
    offset: u64,
    status: i32,
    bytes: u32,
    _pad: [u8; HSMR_PACKET_BYTES - 24],
}

const HSMR_OP_NOP: u32 = 0;
const HSMR_OP_FILE_WRITE: u32 = 1;
const HSMR_OP_FILE_READ: u32 = 2;

const IO_FILE_PATH: &[u16] = &[
    0x005C, 0x003F, 0x003F, 0x005C, 0x0043, 0x003A, 0x005C, 0x0044, 0x0072, 0x0069, 0x0076,
    0x0065, 0x0072, 0x005C, 0x006B, 0x0063, 0x006F, 0x006D, 0x005F, 0x0069, 0x006F, 0x005F,
    0x0074, 0x0065, 0x0073, 0x0074, 0x002E, 0x0062, 0x0069, 0x006E, 0x0000,
];

struct HsmrContext {
    buffer: *mut u8,
    buffer_len: usize,
    mdl: *mut wdk_sys::MDL,
    user_va: *mut c_void,
    control: *mut HsmrControl,
    ring_base: *mut u8,
    overflow_base: *mut u8,
    ring_entries: u32,
    overflow_entries: u32,
    packet_bytes: u32,
    payload_base: *mut u8,
    payload_slot_bytes: u32,
    payload_slots: u32,
    file_handle: HANDLE,
    file_object: PFILE_OBJECT,
    target_device: *mut DEVICE_OBJECT,
    init_flags: u32,
    doorbell_event: PKEVENT,
    space_event: PKEVENT,
    thread_handle: HANDLE,
    thread_object: PETHREAD,
    stop: AtomicU32,
    owner_process: PRKPROCESS,
    deferred_next: *mut HsmrContext,
}

static HSMR_CTX: AtomicPtr<HsmrContext> = AtomicPtr::new(ptr::null_mut());
static HSMR_DEFERRED_HEAD: AtomicPtr<HsmrContext> = AtomicPtr::new(ptr::null_mut());
static HSMR_DEFERRED_LOCK_INIT: AtomicU32 = AtomicU32::new(0);
static mut HSMR_DEFERRED_LOCK: KSPIN_LOCK = 0;
static HSMR_PROCESS_NOTIFY_REGISTERED: AtomicU32 = AtomicU32::new(0);

fn ensure_deferred_lock() {
    if HSMR_DEFERRED_LOCK_INIT
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        unsafe { KeInitializeSpinLock(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK) };
    }
}

unsafe fn deferred_push(ctx: *mut HsmrContext) {
    if ctx.is_null() {
        return;
    }
    ensure_deferred_lock();
    unsafe {
        let old_irql =
            KeAcquireSpinLockRaiseToDpc(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK);
        let head = HSMR_DEFERRED_HEAD.load(Ordering::Acquire);
        (*ctx).deferred_next = head;
        HSMR_DEFERRED_HEAD.store(ctx, Ordering::Release);
        KeReleaseSpinLock(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK, old_irql);
    }
}

unsafe fn deferred_take_for_process(process: PEPROCESS) -> *mut HsmrContext {
    if process.is_null() {
        return ptr::null_mut();
    }
    ensure_deferred_lock();
    unsafe {
        let old_irql =
            KeAcquireSpinLockRaiseToDpc(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK);
        let mut head = HSMR_DEFERRED_HEAD.load(Ordering::Acquire);
        let mut prev: *mut HsmrContext = ptr::null_mut();
        let mut matched: *mut HsmrContext = ptr::null_mut();
        while !head.is_null() {
            let next = (*head).deferred_next;
            if (*head).owner_process as PEPROCESS == process {
                if prev.is_null() {
                    HSMR_DEFERRED_HEAD.store(next, Ordering::Release);
                } else {
                    (*prev).deferred_next = next;
                }
                (*head).deferred_next = matched;
                matched = head;
            } else {
                prev = head;
            }
            head = next;
        }
        KeReleaseSpinLock(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK, old_irql);
        matched
    }
}

unsafe fn deferred_take_all() -> *mut HsmrContext {
    ensure_deferred_lock();
    unsafe {
        let old_irql =
            KeAcquireSpinLockRaiseToDpc(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK);
        let head = HSMR_DEFERRED_HEAD.swap(ptr::null_mut(), Ordering::AcqRel);
        KeReleaseSpinLock(&raw mut HSMR_DEFERRED_LOCK as *mut KSPIN_LOCK, old_irql);
        head
    }
}

unsafe fn reap_deferred_dead() {
    let mut list = unsafe { deferred_take_all() };
    let mut alive: *mut HsmrContext = ptr::null_mut();
    while !list.is_null() {
        let next = unsafe { (*list).deferred_next };
        let owner = unsafe { (*list).owner_process };
        let is_alive = if !owner.is_null() {
            unsafe { PsGetProcessExitStatus(owner as PEPROCESS) == STATUS_PENDING }
        } else {
            false
        };
        if is_alive {
            unsafe { (*list).deferred_next = alive };
            alive = list;
        } else {
            unsafe { finalize_deferred_context(list) };
        }
        list = next;
    }

    while !alive.is_null() {
        let next = unsafe { (*alive).deferred_next };
        unsafe { deferred_push(alive) };
        alive = next;
    }
}

unsafe fn ensure_process_notify_registered() {
    if HSMR_PROCESS_NOTIFY_REGISTERED.load(Ordering::Acquire) != 0 {
        return;
    }
    let status = unsafe { PsSetCreateProcessNotifyRoutineEx(Some(hsmr_process_notify), 0) };
    if status >= 0 {
        HSMR_PROCESS_NOTIFY_REGISTERED.store(1, Ordering::Release);
    }
}

unsafe fn unregister_process_notify() {
    if HSMR_PROCESS_NOTIFY_REGISTERED.swap(0, Ordering::AcqRel) != 0 {
        let _ = unsafe { PsSetCreateProcessNotifyRoutineEx(Some(hsmr_process_notify), 1) };
    }
}

unsafe extern "C" fn hsmr_process_notify(
    process: PEPROCESS,
    _process_id: HANDLE,
    create_info: PPS_CREATE_NOTIFY_INFO,
) {
    if !create_info.is_null() {
        return;
    }

    let global_ctx = HSMR_CTX.load(Ordering::Acquire);
    if !global_ctx.is_null() {
        let owner = (*global_ctx).owner_process;
        if owner as PEPROCESS == process {
            if HSMR_CTX.compare_exchange(
                global_ctx,
                ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                (*global_ctx).stop.store(1, Ordering::Release);
                if !(*global_ctx).doorbell_event.is_null() {
                    KeSetEvent((*global_ctx).doorbell_event, 0, 0);
                }
                deferred_push(global_ctx);
            }
        }
    }
    

    let mut list = unsafe { deferred_take_for_process(process) };
    while !list.is_null() {
        let next = unsafe { (*list).deferred_next };
        unsafe { finalize_deferred_context(list) };
        list = next;
    }
}

pub unsafe fn handle_init(
    buffer: *mut u8,
    in_len: usize,
    out_len: usize,
) -> Result<usize, NTSTATUS> {
    if in_len < core::mem::size_of::<HsmrInitRequestV1>() {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }

    let (req_version, req_mode, doorbell_handle, space_handle, init_flags) =
        if in_len >= core::mem::size_of::<HsmrInitRequestV2>() {
            let req = unsafe { &*(buffer as *const HsmrInitRequestV2) };
            if req.version != HSMR_VERSION_V2 {
                return Err(STATUS_INVALID_PARAMETER);
            }
            (
                req.version,
                req.mode,
                req.doorbell_event,
                req.space_event,
                req.flags,
            )
        } else {
            let req = unsafe { &*(buffer as *const HsmrInitRequestV1) };
            if req.version != HSMR_VERSION_V1 {
                return Err(STATUS_INVALID_PARAMETER);
            }
            (
                req.version,
                req.mode,
                req.doorbell_event,
                req.space_event,
                0,
            )
        };

    let want_payload = (init_flags & HSMR_INIT_FLAG_PAYLOAD) != 0;
    let file_write_through = (init_flags & HSMR_INIT_FLAG_FILE_WRITE_THROUGH) != 0;
    let file_no_buffering = (init_flags & HSMR_INIT_FLAG_FILE_NO_BUFFERING) != 0;
    if !want_payload && (init_flags & !HSMR_INIT_FLAG_PAYLOAD) != 0 {
        return Err(STATUS_INVALID_PARAMETER);
    }

    let response_size = if req_version == HSMR_VERSION_V2 {
        core::mem::size_of::<HsmrInitResponseV2>()
    } else {
        core::mem::size_of::<HsmrInitResponseV1>()
    };
    if out_len < response_size {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }

    let ring_entries = match req_mode {
        HSMR_MODE_MVP => HSMR_RING_SIZE_MVP,
        HSMR_MODE_LARGE => HSMR_RING_SIZE_LARGE,
        _ => return Err(STATUS_INVALID_PARAMETER),
    };

    let existing = HSMR_CTX.load(Ordering::Acquire);
    if !existing.is_null() {
        return Err(STATUS_DEVICE_BUSY);
    }

    unsafe { ensure_process_notify_registered() };
    unsafe { reap_deferred_dead() };

    let control_size = core::mem::size_of::<HsmrControl>();
    let ring_bytes = ring_entries
        .checked_mul(HSMR_PACKET_BYTES)
        .ok_or(STATUS_INVALID_PARAMETER)
        .unwrap_or(0);
    let overflow_bytes = HSMR_OVERFLOW_ENTRIES
        .checked_mul(HSMR_PACKET_BYTES)
        .ok_or(STATUS_INVALID_PARAMETER)
        .unwrap_or(0);
    if ring_bytes == 0 || overflow_bytes == 0 {
        return Err(STATUS_INVALID_PARAMETER);
    }

    let control_align = if want_payload && file_no_buffering {
        HSMR_FILE_IO_ALIGN
    } else {
        64
    };
    let ring_offset_from_control = align_up(control_size, 64);
    let overflow_offset_from_control = align_up(ring_offset_from_control + ring_bytes, 64);
    let payload_align = if want_payload && file_no_buffering {
        HSMR_FILE_IO_ALIGN
    } else {
        64
    };
    let payload_offset_from_control =
        align_up(overflow_offset_from_control + overflow_bytes, payload_align);
    let payload_bytes = if want_payload {
        HSMR_IO_SLOTS
            .checked_mul(HSMR_IO_SLOT_BYTES)
            .ok_or(STATUS_INVALID_PARAMETER)
            .unwrap_or(0)
    } else {
        0
    };
    if want_payload && payload_bytes == 0 {
        return Err(STATUS_INVALID_PARAMETER);
    }

    let total_from_control = payload_offset_from_control + payload_bytes;

    let alloc_len = match total_from_control.checked_add(control_align) {
        Some(value) => value,
        None => return Err(STATUS_INVALID_PARAMETER),
    };

    // Use MmAllocatePagesForMdl instead of WdkAllocator/ExAllocatePool to allow safer sharing with user mode
    // and avoid 0x4E bugchecks on free.
    let mut low = LARGE_INTEGER { QuadPart: 0 };
    let mut high = LARGE_INTEGER { QuadPart: -1 };
    let mut skip = LARGE_INTEGER { QuadPart: 0 };
    let mdl = unsafe { MmAllocatePagesForMdl(low, high, skip, alloc_len as u64) };
    if mdl.is_null() {
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }
    if unsafe { (*mdl).ByteCount } < (alloc_len as u32) {
        unsafe { MmFreePagesFromMdl(mdl) };
        unsafe { IoFreeMdl(mdl) };
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    // Map to Kernel Mode to get a virtual address for driver use
    let buffer_ptr = unsafe {
        MmMapLockedPagesSpecifyCache(
            mdl,
            _MODE::KernelMode as i8,
            _MEMORY_CACHING_TYPE::MmCached,
            ptr::null_mut(),
            0,
            (_MM_PAGE_PRIORITY::NormalPagePriority as u32) | MdlMappingNoExecute,
        )
    } as *mut u8;

    if buffer_ptr.is_null() {
        unsafe { MmFreePagesFromMdl(mdl) };
        unsafe { IoFreeMdl(mdl) };
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }
    
    // Zero the allocated memory
    unsafe { ptr::write_bytes(buffer_ptr, 0, alloc_len) };

    let base_addr = buffer_ptr as usize;
    let control_addr = align_up(base_addr, control_align);
    let control_offset = control_addr - base_addr;
    let ring_offset = match control_offset.checked_add(ring_offset_from_control) {
        Some(value) => value,
        None => {
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(STATUS_INVALID_PARAMETER);
        }
    };
    let overflow_offset = match control_offset.checked_add(overflow_offset_from_control) {
        Some(value) => value,
        None => {
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(STATUS_INVALID_PARAMETER);
        }
    };
    let payload_offset = match control_offset.checked_add(payload_offset_from_control) {
        Some(value) => value,
        None => {
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(STATUS_INVALID_PARAMETER);
        }
    };
    let _total_used = match control_offset.checked_add(total_from_control) {
        Some(value) => value,
        None => {
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(STATUS_INVALID_PARAMETER);
        }
    };

    // Map to User Mode
    let user_va = unsafe {
        MmMapLockedPagesSpecifyCache(
            mdl,
            _MODE::UserMode as i8,
            _MEMORY_CACHING_TYPE::MmCached,
            ptr::null_mut(),
            0,
            (_MM_PAGE_PRIORITY::NormalPagePriority as u32) | MdlMappingNoExecute,
        )
    };
    if user_va.is_null() {
        unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
        unsafe { MmFreePagesFromMdl(mdl) };
        unsafe { IoFreeMdl(mdl) };
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    let control = unsafe { buffer_ptr.add(control_offset) } as *mut HsmrControl;
    unsafe {
        (*control)
            .kernel_status
            .store(HSMR_STATUS_RUNNING, Ordering::Release);
    }

    let mut doorbell_event: PKEVENT = ptr::null_mut();
    if !doorbell_handle.is_null() {
        let status = unsafe {
            ObReferenceObjectByHandle(
                doorbell_handle,
                (EVENT_MODIFY_STATE | SYNCHRONIZE) as u32,
                *wdk_sys::ExEventObjectType,
                _MODE::UserMode as i8,
                &mut doorbell_event as *mut _ as *mut PVOID,
                ptr::null_mut(),
            )
        };
        if status < 0 {
            unsafe { MmUnmapLockedPages(user_va, mdl) };
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(status);
        }
    }

    let mut space_event: PKEVENT = ptr::null_mut();
    if !space_handle.is_null() {
        let status = unsafe {
            ObReferenceObjectByHandle(
                space_handle,
                EVENT_MODIFY_STATE as u32,
                *wdk_sys::ExEventObjectType,
                _MODE::UserMode as i8,
                &mut space_event as *mut _ as *mut PVOID,
                ptr::null_mut(),
            )
        };
        if status < 0 {
            if !doorbell_event.is_null() {
                unsafe { ObfDereferenceObject(doorbell_event as *mut c_void) };
            }
            unsafe { MmUnmapLockedPages(user_va, mdl) };
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(status);
        }
    }

    let (file_handle, file_object, target_device) = if want_payload {
        let handle = match unsafe { open_io_file_handle(file_write_through, file_no_buffering) } {
            Ok(handle) => handle,
            Err(status) => {
                if !space_event.is_null() {
                    unsafe { ObfDereferenceObject(space_event as *mut c_void) };
                }
                if !doorbell_event.is_null() {
                    unsafe { ObfDereferenceObject(doorbell_event as *mut c_void) };
                }
                unsafe { MmUnmapLockedPages(user_va, mdl) };
                unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
                unsafe { MmFreePagesFromMdl(mdl) };
                unsafe { IoFreeMdl(mdl) };
                return Err(status);
            }
        };

        let mut file_object: PFILE_OBJECT = ptr::null_mut();
        let status = unsafe {
            ObReferenceObjectByHandle(
                handle,
                (FILE_READ_DATA | FILE_WRITE_DATA | SYNCHRONIZE) as ACCESS_MASK,
                *IoFileObjectType,
                _MODE::KernelMode as i8,
                &mut file_object as *mut _ as *mut PVOID,
                ptr::null_mut(),
            )
        };
        if status < 0 || file_object.is_null() {
            if !space_event.is_null() {
                unsafe { ObfDereferenceObject(space_event as *mut c_void) };
            }
            if !doorbell_event.is_null() {
                unsafe { ObfDereferenceObject(doorbell_event as *mut c_void) };
            }
            unsafe {
                let _ = ZwClose(handle);
            }
            unsafe { MmUnmapLockedPages(user_va, mdl) };
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(if status < 0 { status } else { STATUS_INVALID_DEVICE_STATE });
        }

        let target_device = unsafe { IoGetRelatedDeviceObject(file_object) };
        if target_device.is_null() {
            if !space_event.is_null() {
                unsafe { ObfDereferenceObject(space_event as *mut c_void) };
            }
            if !doorbell_event.is_null() {
                unsafe { ObfDereferenceObject(doorbell_event as *mut c_void) };
            }
            unsafe {
                ObfDereferenceObject(file_object as *mut c_void);
                let _ = ZwClose(handle);
            }
            unsafe { MmUnmapLockedPages(user_va, mdl) };
            unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
            unsafe { MmFreePagesFromMdl(mdl) };
            unsafe { IoFreeMdl(mdl) };
            return Err(STATUS_INVALID_DEVICE_STATE);
        }

        (handle, file_object, target_device)
    } else {
        (ptr::null_mut(), ptr::null_mut(), ptr::null_mut())
    };

    let owner_process = unsafe { IoGetCurrentProcess() as PRKPROCESS };
    unsafe { ObfReferenceObject(owner_process as *mut c_void) };

    let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"HSMR"));
    let ctx_layout = Layout::new::<HsmrContext>();
    let ctx_ptr = unsafe { alloc.alloc(ctx_layout) } as *mut HsmrContext;
    if ctx_ptr.is_null() {
        unsafe { ObfDereferenceObject(owner_process as *mut c_void) };
        if !file_object.is_null() {
            unsafe { ObfDereferenceObject(file_object as *mut c_void) };
        }
        if !file_handle.is_null() {
            unsafe { let _ = ZwClose(file_handle); };
        }
        if !space_event.is_null() {
            unsafe { ObfDereferenceObject(space_event as *mut c_void) };
        }
        if !doorbell_event.is_null() {
            unsafe { ObfDereferenceObject(doorbell_event as *mut c_void) };
        }
        unsafe { MmUnmapLockedPages(user_va, mdl) };
        unsafe { MmUnmapLockedPages(buffer_ptr as *mut c_void, mdl) };
        unsafe { MmFreePagesFromMdl(mdl) };
        unsafe { IoFreeMdl(mdl) };
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    unsafe {
        let ring_base = buffer_ptr.add(ring_offset);
        let overflow_base = buffer_ptr.add(overflow_offset);
        let payload_base = if want_payload {
            buffer_ptr.add(payload_offset)
        } else {
            ptr::null_mut()
        };

        ptr::write(
            ctx_ptr,
            HsmrContext {
                buffer: buffer_ptr,
                buffer_len: alloc_len,
                mdl,
                user_va,
                control,
                ring_base,
                overflow_base,
                ring_entries: ring_entries as u32,
                overflow_entries: HSMR_OVERFLOW_ENTRIES as u32,
                packet_bytes: HSMR_PACKET_BYTES as u32,
                payload_base,
                payload_slot_bytes: if want_payload {
                    HSMR_IO_SLOT_BYTES as u32
                } else {
                    0
                },
                payload_slots: if want_payload { HSMR_IO_SLOTS as u32 } else { 0 },
                file_handle,
                file_object,
                target_device,
                init_flags,
                doorbell_event,
                space_event,
                thread_handle: ptr::null_mut(),
                thread_object: ptr::null_mut(),
                stop: AtomicU32::new(0),
                owner_process,
                deferred_next: ptr::null_mut(),
            },
        );
    }

    let inserted = HSMR_CTX.compare_exchange(
        ptr::null_mut(),
        ctx_ptr,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    if inserted.is_err() {
        unsafe { cleanup_context(ctx_ptr) };
        return Err(STATUS_DEVICE_BUSY);
    }

    let mut thread_handle: HANDLE = ptr::null_mut();
    let status = unsafe {
        PsCreateSystemThread(
            &mut thread_handle as *mut HANDLE,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            Some(hsmr_thread),
            ctx_ptr as *mut c_void,
        )
    };
    if status < 0 {
        let _ = HSMR_CTX.compare_exchange(ctx_ptr, ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire);
        unsafe { cleanup_context(ctx_ptr) };
        return Err(status);
    }
    unsafe { (*ctx_ptr).thread_handle = thread_handle };

    let mut thread_object: PETHREAD = ptr::null_mut();
    let status = unsafe {
        ObReferenceObjectByHandle(
            thread_handle,
            SYNCHRONIZE,
            ptr::null_mut(),
            _MODE::KernelMode as i8,
            &mut thread_object as *mut _ as *mut PVOID,
            ptr::null_mut(),
        )
    };
    if status < 0 {
        let _ = HSMR_CTX.compare_exchange(ctx_ptr, ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire);
        unsafe { cleanup_context(ctx_ptr) };
        return Err(status);
    }
    unsafe { (*ctx_ptr).thread_object = thread_object };

    if req_version == HSMR_VERSION_V2 {
        let resp = HsmrInitResponseV2 {
            user_ptr: user_va,
            control_offset: control_offset as u32,
            ring_offset: ring_offset as u32,
            ring_entries: ring_entries as u32,
            packet_bytes: HSMR_PACKET_BYTES as u32,
            overflow_offset: overflow_offset as u32,
            overflow_entries: HSMR_OVERFLOW_ENTRIES as u32,
            flags: (if req_mode == HSMR_MODE_LARGE { 1 } else { 0 })
                | (if want_payload { HSMR_FLAG_HAS_PAYLOAD } else { 0 }),
            total_bytes: alloc_len as u32,
            payload_offset: if want_payload { payload_offset as u32 } else { 0 },
            payload_slot_bytes: if want_payload { HSMR_IO_SLOT_BYTES as u32 } else { 0 },
            payload_slots: if want_payload { HSMR_IO_SLOTS as u32 } else { 0 },
            _reserved: 0,
        };
        unsafe {
            let out_ptr = buffer as *mut HsmrInitResponseV2;
            out_ptr.write(resp);
        }
        Ok(core::mem::size_of::<HsmrInitResponseV2>())
    } else {
        let resp = HsmrInitResponseV1 {
            user_ptr: user_va,
            control_offset: control_offset as u32,
            ring_offset: ring_offset as u32,
            ring_entries: ring_entries as u32,
            packet_bytes: HSMR_PACKET_BYTES as u32,
            overflow_offset: overflow_offset as u32,
            overflow_entries: HSMR_OVERFLOW_ENTRIES as u32,
            flags: if req_mode == HSMR_MODE_LARGE { 1 } else { 0 },
            total_bytes: alloc_len as u32,
        };
        unsafe {
            let out_ptr = buffer as *mut HsmrInitResponseV1;
            out_ptr.write(resp);
        }
        Ok(core::mem::size_of::<HsmrInitResponseV1>())
    }
}

pub unsafe fn handle_shutdown() -> NTSTATUS {
    let ctx = HSMR_CTX.swap(ptr::null_mut(), Ordering::AcqRel);
    if ctx.is_null() {
        return STATUS_INVALID_DEVICE_STATE;
    }
    unsafe { cleanup_context(ctx) };
    STATUS_SUCCESS
}

pub unsafe fn handle_driver_unload() {
    let _ = unsafe { handle_shutdown() };
    let mut deferred = unsafe { deferred_take_all() };
    unsafe { unregister_process_notify() };
    while !deferred.is_null() {
        let next = unsafe { (*deferred).deferred_next };
        let owner = unsafe { (*deferred).owner_process };
        let owner_alive = if !owner.is_null() {
            unsafe { PsGetProcessExitStatus(owner as PEPROCESS) == STATUS_PENDING }
        } else {
            false
        };
        if owner_alive {
            if !owner.is_null() {
                unsafe { ObfDereferenceObject(owner as *mut c_void) };
            }
            let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"HSMR"));
            unsafe { alloc.dealloc(deferred as *mut u8, Layout::new::<HsmrContext>()) };
        } else {
            unsafe { finalize_deferred_context(deferred) };
        }
        deferred = next;
    }
}

unsafe fn cleanup_context(ctx: *mut HsmrContext) {
    let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"HSMR"));
    unsafe {
        (*ctx).stop.store(1, Ordering::Release);
        if !(*ctx).doorbell_event.is_null() {
            KeSetEvent((*ctx).doorbell_event, 0, 0);
        }
        if !(*ctx).thread_handle.is_null() {
            let _ = ZwWaitForSingleObject((*ctx).thread_handle, 0, ptr::null_mut());
            let _ = ZwClose((*ctx).thread_handle);
        }
        if !(*ctx).thread_object.is_null() {
            ObfDereferenceObject((*ctx).thread_object as *mut c_void);
        }
        if !(*ctx).file_object.is_null() {
            ObfDereferenceObject((*ctx).file_object as *mut c_void);
        }
        if !(*ctx).file_handle.is_null() {
            let _ = ZwClose((*ctx).file_handle);
        }
        if !(*ctx).space_event.is_null() {
            ObfDereferenceObject((*ctx).space_event as *mut c_void);
        }
        if !(*ctx).doorbell_event.is_null() {
            ObfDereferenceObject((*ctx).doorbell_event as *mut c_void);
        }
    }

    let has_mapping = unsafe { !(*ctx).user_va.is_null() && !(*ctx).mdl.is_null() };
    let owner_process = unsafe { (*ctx).owner_process };
    
    if has_mapping && !owner_process.is_null() {
        let current_process = unsafe { IoGetCurrentProcess() as PRKPROCESS };
        
        // 修正1: プロセス終了状態のチェックを強化
        // 所有プロセスが終了処理中の場合、MmUnmapLockedPagesを呼ぶのは危険(4E/9Aの原因)。
        let status = unsafe { PsGetProcessExitStatus(owner_process as PEPROCESS) };
        if status != STATUS_PENDING {
            // プロセスは終了中または終了済み。
            // deferred_pushして、finalize_deferred_contextでの安全な破棄（バッファリーク）に任せる。
            unsafe { deferred_push(ctx) };
            return;
        }

        if current_process == owner_process {
            // We are already in the correct process context AND process is alive.
            unsafe { MmUnmapLockedPages((*ctx).user_va, (*ctx).mdl) };
            unsafe { (*ctx).user_va = ptr::null_mut() };
        } else {
            // Different process. Process is alive (checked above).
            let mut apc_state: KAPC_STATE = unsafe { core::mem::zeroed() };
            unsafe { KeStackAttachProcess(owner_process, &mut apc_state) };
            unsafe { MmUnmapLockedPages((*ctx).user_va, (*ctx).mdl) };
            unsafe { KeUnstackDetachProcess(&mut apc_state) };
            unsafe { (*ctx).user_va = ptr::null_mut() };
        }
    } else if has_mapping {
        // Mapping exists but no owner process
        unsafe { MmUnmapLockedPages((*ctx).user_va, (*ctx).mdl) };
        unsafe { (*ctx).user_va = ptr::null_mut() };
    }

    if !owner_process.is_null() {
        unsafe { ObfDereferenceObject(owner_process as *mut c_void) };
    }
    
    // Unmap Kernel Mapping
    if unsafe { !(*ctx).buffer.is_null() && !(*ctx).mdl.is_null() } {
        unsafe { MmUnmapLockedPages((*ctx).buffer as *mut c_void, (*ctx).mdl) };
    }

    // Free Pages and MDL
    if unsafe { !(*ctx).mdl.is_null() } {
        unsafe { MmFreePagesFromMdl((*ctx).mdl) };
        unsafe { IoFreeMdl((*ctx).mdl) };
    }
    
    unsafe { alloc.dealloc(ctx as *mut u8, Layout::new::<HsmrContext>()) };
}

unsafe fn finalize_deferred_context(ctx: *mut HsmrContext) {
    if ctx.is_null() {
        return;
    }
    let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"HSMR"));
    unsafe {
        let mut unmapped_user = false;
        let mdl = (*ctx).mdl;
        let user_va = (*ctx).user_va;
        let owner = (*ctx).owner_process;

        if !mdl.is_null() && !user_va.is_null() {
            if !owner.is_null() {
                let current = IoGetCurrentProcess() as PRKPROCESS;
                
                if current == owner {
                    // 現在のプロセスが所有者なら安全にアンマップ可能
                    MmUnmapLockedPages(user_va, mdl);
                    (*ctx).user_va = ptr::null_mut();
                    unmapped_user = true;
                } else {
                    // 別プロセスの場合、生存確認を行う
                    let status = PsGetProcessExitStatus(owner as PEPROCESS);
                    if status == STATUS_PENDING {
                        // プロセスが生きていればアタッチしてアンマップ
                        let mut apc_state: KAPC_STATE = core::mem::zeroed();
                        KeStackAttachProcess(owner, &mut apc_state);
                        MmUnmapLockedPages(user_va, mdl);
                        KeUnstackDetachProcess(&mut apc_state);
                        (*ctx).user_va = ptr::null_mut();
                        unmapped_user = true;
                    } else {
                        // プロセスが終了済み（STATUS_PENDING以外）
                        // この状態で MmUnmapLockedPages を呼ぶとクラッシュする可能性があるため
                        // アンマップを諦める（リーク許容）。
                        unmapped_user = false;
                    }
                }
            } else {
                // 所有者なし（System Mapping等）
                MmUnmapLockedPages(user_va, mdl);
                (*ctx).user_va = ptr::null_mut();
                unmapped_user = true;
            }
        } else {
            unmapped_user = true;
        }
        
        // Always unmap kernel mapping
        if !(*ctx).buffer.is_null() && !mdl.is_null() {
            MmUnmapLockedPages((*ctx).buffer as *mut c_void, mdl);
        }

        if !mdl.is_null() {
            if unmapped_user {
                 // Safe to free physical pages
                 MmFreePagesFromMdl(mdl);
            } else {
                 // User mapping leaked; cannot free physical pages safely without risking system corruption.
                 // We leak the pages.
            }
            IoFreeMdl(mdl);
        }

        // コンテキスト自体の解放
        if !owner.is_null() {
            ObfDereferenceObject(owner as *mut c_void);
        }
        alloc.dealloc(ctx as *mut u8, Layout::new::<HsmrContext>());
    }
}

#[repr(C)]
struct HsmrIoContext {
    io_status: IO_STATUS_BLOCK,
}

#[inline]
unsafe fn alloc_io_context() -> *mut HsmrIoContext {
    let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"HSMR"));
    let layout = Layout::new::<HsmrIoContext>();
    let ptr = unsafe { alloc.alloc(layout) } as *mut HsmrIoContext;
    if !ptr.is_null() {
        unsafe {
            ptr::write(
                ptr,
                HsmrIoContext {
                    io_status: core::mem::zeroed(),
                },
            );
        }
    }
    ptr
}

unsafe fn free_io_context(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let alloc = WdkAllocator::new(PoolType::NonPagedNx, u32::from_ne_bytes(*b"HSMR"));
    unsafe { alloc.dealloc(ctx as *mut u8, Layout::new::<HsmrIoContext>()) };
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
unsafe fn init_irp_file_object(irp: *mut IRP, file_object: PFILE_OBJECT) {
    if irp.is_null() || file_object.is_null() {
        return;
    }
    unsafe {
        (*irp).Tail.Overlay.OriginalFileObject = file_object;
        (*irp).RequestorMode = _MODE::KernelMode as i8;
        let io_stack = io_get_next_irp_stack_location(irp);
        (*io_stack).FileObject = file_object;
    }
}

fn block_on<F>(mut future: F) -> F::Output
where
    F: Future,
{
    let mut event: KEVENT = unsafe { core::mem::zeroed() };
    unsafe { KeInitializeEvent(&mut event, wdk_sys::_EVENT_TYPE::SynchronizationEvent, 0) };

    let waker = unsafe { event_waker(&mut event as *mut KEVENT) };
    let mut cx = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };

    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => unsafe {
                let _ = KeWaitForSingleObject(
                    &mut event as *mut KEVENT as *mut c_void,
                    Executive,
                    _MODE::KernelMode as i8,
                    0,
                    ptr::null_mut(),
                );
            },
        }
    }
}

unsafe fn event_waker(event: *mut KEVENT) -> Waker {
    unsafe fn clone(data: *const ()) -> RawWaker {
        RawWaker::new(data, &VTABLE)
    }
    unsafe fn wake(data: *const ()) {
        unsafe { wake_by_ref(data) };
    }
    unsafe fn wake_by_ref(data: *const ()) {
        let event = data as *mut KEVENT;
        if !event.is_null() {
            unsafe { KeSetEvent(event, 0, 0) };
        }
    }
    unsafe fn drop(_data: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

    unsafe { Waker::from_raw(RawWaker::new(event as *const (), &VTABLE)) }
}

unsafe fn submit_file_irp(
    ctx: *mut HsmrContext,
    op: u32,
    buffer: *mut c_void,
    size: u32,
    offset: u64,
) -> IrpResult {
    if ctx.is_null() {
        return IrpResult {
            status: STATUS_INVALID_DEVICE_STATE,
            information: 0,
        };
    }

    let file_object = unsafe { (*ctx).file_object };
    let target_device = unsafe { (*ctx).target_device };
    if file_object.is_null() || target_device.is_null() {
        return IrpResult {
            status: STATUS_INVALID_DEVICE_STATE,
            information: 0,
        };
    }

    let io_ctx = unsafe { alloc_io_context() };
    if io_ctx.is_null() {
        return IrpResult {
            status: STATUS_INSUFFICIENT_RESOURCES,
            information: 0,
        };
    }
    let io_status = unsafe { &mut (*io_ctx).io_status as *mut IO_STATUS_BLOCK };

    let mut byte_offset = LARGE_INTEGER {
        QuadPart: offset as i64,
    };
    let major = match op {
        HSMR_OP_FILE_WRITE => IRP_MJ_WRITE,
        HSMR_OP_FILE_READ => IRP_MJ_READ,
        _ => {
            unsafe { free_io_context(io_ctx as *mut c_void) };
            return IrpResult {
                status: STATUS_INVALID_PARAMETER,
                information: 0,
            };
        }
    };
    let irp = match unsafe {
        build_async_fsd_request(
            major as u32,
            target_device,
            buffer,
            size,
            &mut byte_offset as *mut LARGE_INTEGER,
            io_status,
        )
    } {
        Ok(irp) => irp,
        Err(status) => {
            unsafe { free_io_context(io_ctx as *mut c_void) };
            return IrpResult {
                status,
                information: 0,
            };
        }
    };

    unsafe { init_irp_file_object(irp, file_object) };

    let mut opts = IrpFutureOptions::default();
    opts.context = io_ctx as *mut c_void;
    opts.on_cleanup = Some(free_io_context);
    opts.wake_mode = IrpWakeMode::Direct;

    let future = match unsafe { IrpFuture::new(target_device, irp, opts) } {
        Ok(future) => future,
        Err(status) => {
            unsafe {
                free_io_context(io_ctx as *mut c_void);
                IoFreeIrp(irp);
            }
            return IrpResult {
                status,
                information: 0,
            };
        }
    };

    block_on(future)
}

unsafe extern "C" fn hsmr_thread(context: *mut c_void) {
    let ctx = context as *mut HsmrContext;
    if ctx.is_null() {
        return;
    }

    let control = unsafe { (*ctx).control };
    let ring_mask = unsafe { (*ctx).ring_entries - 1 };
    let overflow_mask = unsafe { (*ctx).overflow_entries - 1 };
    let file_handle = unsafe { (*ctx).file_handle };
    let init_flags = unsafe { (*ctx).init_flags };
    let file_no_buffering = (init_flags & HSMR_INIT_FLAG_FILE_NO_BUFFERING) != 0;
    let payload_base = unsafe { (*ctx).payload_base };
    let payload_slot_bytes = unsafe { (*ctx).payload_slot_bytes } as usize;
    let payload_slots = unsafe { (*ctx).payload_slots };
    let payload_mask = payload_slots.wrapping_sub(1);
    let ring_base = unsafe { (*ctx).ring_base };
    let overflow_base = unsafe { (*ctx).overflow_base };

    let mut spins = 0u32;
    loop {
        if unsafe { (*ctx).stop.load(Ordering::Acquire) } != 0 {
            break;
        }

        let mut progressed = false;
        let mut should_signal_space = false;

        // If this session doesn't have a payload mapping, we are in "raw" (no-op) mode:
        // just drain indices as quickly as possible.
        if payload_base.is_null() || file_handle.is_null() {
            // Always service both rings each loop iteration. If the primary ring remains busy,
            // we still want to make forward progress on the overflow lane.
            let head = unsafe { (*control).head.load(Ordering::Acquire) };
            let tail = unsafe { (*control).tail.load(Ordering::Acquire) };
            if head != tail {
                unsafe { (*control).kernel_status.store(HSMR_STATUS_RUNNING, Ordering::Release) };
                let _ = ring_mask;
                unsafe { (*control).head.store(tail, Ordering::Release) };
                progressed = true;
                should_signal_space = true;
            }

            let ohead = unsafe { (*control).overflow_head.load(Ordering::Acquire) };
            let otail = unsafe { (*control).overflow_tail.load(Ordering::Acquire) };
            if ohead != otail {
                unsafe { (*control).kernel_status.store(HSMR_STATUS_RUNNING, Ordering::Release) };
                let _ = overflow_mask;
                unsafe { (*control).overflow_head.store(otail, Ordering::Release) };
                progressed = true;
                should_signal_space = true;
            }
        } else {
            // File I/O mode: consume packets in-order and only advance head after the operation
            // has completed, using the head index as the completion sequence.
            let head = unsafe { (*control).head.load(Ordering::Acquire) };
            let tail = unsafe { (*control).tail.load(Ordering::Acquire) };
            if head != tail {
                unsafe { (*control).kernel_status.store(HSMR_STATUS_RUNNING, Ordering::Release) };
                let mut local_head = head;
                while local_head != tail {
                    let idx = (local_head & ring_mask) as usize;
                    let packet_ptr = unsafe { ring_base.add(idx * HSMR_PACKET_BYTES) } as *mut HsmrPacket;
                    let mut packet = unsafe { ptr::read_unaligned(packet_ptr) };
                    let op = packet.op;
                    if op != HSMR_OP_NOP {
                        let slot_idx = (local_head & payload_mask) as usize;
                        let buf = unsafe { payload_base.add(slot_idx * payload_slot_bytes) } as *mut c_void;
                        let req_len = packet.size as usize;
                        if req_len > payload_slot_bytes {
                            packet.status = STATUS_INVALID_PARAMETER;
                            packet.bytes = 0;
                        } else if packet.offset > i64::MAX as u64 {
                            packet.status = STATUS_INVALID_PARAMETER;
                            packet.bytes = 0;
                        } else if file_no_buffering
                            && (((buf as usize) & (HSMR_FILE_IO_ALIGN - 1)) != 0
                                || (req_len & (HSMR_FILE_IO_ALIGN - 1)) != 0
                                || ((packet.offset as usize) & (HSMR_FILE_IO_ALIGN - 1)) != 0)
                        {
                            packet.status = STATUS_INVALID_PARAMETER;
                            packet.bytes = 0;
                        } else {
                            let result = unsafe {
                                submit_file_irp(
                                    ctx,
                                    op,
                                    buf,
                                    packet.size,
                                    packet.offset,
                                )
                            };
                            packet.status = result.status;
                            packet.bytes = result.information as u32;
                        }
                        unsafe { ptr::write_unaligned(packet_ptr, packet) };
                    }
                    local_head = local_head.wrapping_add(1);
                    // Publish completion in-order. The producer uses `head` for both
                    // completion tracking and space calculations (shadow head).
                    unsafe { (*control).head.store(local_head, Ordering::Release) };
                }
                progressed = true;
                should_signal_space = true;
            }

            // For now, just drain overflow to avoid producer stalls; file I/O submissions should
            // use the primary ring to avoid payload slot aliasing.
            let ohead = unsafe { (*control).overflow_head.load(Ordering::Acquire) };
            let otail = unsafe { (*control).overflow_tail.load(Ordering::Acquire) };
            if ohead != otail {
                let _ = overflow_mask;
                let _ = overflow_base;
                unsafe { (*control).overflow_head.store(otail, Ordering::Release) };
                progressed = true;
                should_signal_space = true;
            }
        }

        if progressed {
            if should_signal_space && unsafe { !(*ctx).space_event.is_null() } {
                unsafe { KeSetEvent((*ctx).space_event, 0, 0) };
            }
            spins = 0;
            continue;
        }

        if spins < SPIN_LIMIT {
            spins += 1;
            core::hint::spin_loop();
            continue;
        }

        spins = 0;
        unsafe { (*control).kernel_status.store(HSMR_STATUS_SLEEPING, Ordering::Release) };
        core::sync::atomic::fence(Ordering::SeqCst);

        let head_now = unsafe { (*control).head.load(Ordering::Acquire) };
        let tail_now = unsafe { (*control).tail.load(Ordering::Acquire) };
        let ohead_now = unsafe { (*control).overflow_head.load(Ordering::Acquire) };
        let otail_now = unsafe { (*control).overflow_tail.load(Ordering::Acquire) };
        if head_now != tail_now || ohead_now != otail_now {
            unsafe { (*control).kernel_status.store(HSMR_STATUS_RUNNING, Ordering::Release) };
            continue;
        }

        if unsafe { (*ctx).doorbell_event.is_null() } {
            let mut interval = LARGE_INTEGER { QuadPart: -10_000 };
            unsafe { let _ = KeDelayExecutionThread(_MODE::KernelMode as i8, 0, &mut interval); };
        } else {
            unsafe {
                let _ = KeWaitForSingleObject(
                    (*ctx).doorbell_event as *mut c_void,
                    Executive,
                    _MODE::KernelMode as i8,
                    0,
                    ptr::null_mut(),
                );
                KeClearEvent((*ctx).doorbell_event);
            }
        }
        unsafe { (*control).kernel_status.store(HSMR_STATUS_RUNNING, Ordering::Release) };
    }
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn make_unicode_string(buf: &'static [u16]) -> UNICODE_STRING {
    let len = (buf.len().saturating_sub(1) * 2) as u16;
    UNICODE_STRING {
        Length: len,
        MaximumLength: len + 2,
        Buffer: buf.as_ptr() as *mut u16,
    }
}

unsafe fn open_io_file_handle(
    write_through: bool,
    no_buffering: bool,
) -> Result<HANDLE, NTSTATUS> {
    let mut name = make_unicode_string(IO_FILE_PATH);
    let mut attrs = OBJECT_ATTRIBUTES {
        Length: core::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: ptr::null_mut(),
        ObjectName: &mut name as *mut UNICODE_STRING,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE,
        SecurityDescriptor: ptr::null_mut(),
        SecurityQualityOfService: ptr::null_mut(),
    };

    let mut io_status: IO_STATUS_BLOCK = unsafe { core::mem::zeroed() };
    let mut handle_out: HANDLE = ptr::null_mut();
    let mut options = FILE_NON_DIRECTORY_FILE;
    if write_through {
        options |= FILE_WRITE_THROUGH;
    }
    if no_buffering {
        options |= FILE_NO_INTERMEDIATE_BUFFERING;
    }
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

    Ok(handle_out)
}