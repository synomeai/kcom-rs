use std::ffi::{c_void, OsStr};
use std::collections::VecDeque;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::FileExt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows_sys::Win32::System::IO::DeviceIoControl;

const FILE_DEVICE_UNKNOWN: u32 = 0x00000022;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_DATA: u32 = 0x0001;
const FILE_WRITE_DATA: u32 = 0x0002;

const IOCTL_KCOM_IO_ECHO: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x801,
    METHOD_BUFFERED,
    FILE_READ_DATA | FILE_WRITE_DATA,
);
const IOCTL_KCOM_IO_FILE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x802,
    METHOD_BUFFERED,
    FILE_READ_DATA | FILE_WRITE_DATA,
);
const IOCTL_KCOM_IO_FILE_ASYNC: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x803,
    METHOD_BUFFERED,
    FILE_READ_DATA | FILE_WRITE_DATA,
);
const IOCTL_KCOM_HSMR_INIT: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x820,
    METHOD_BUFFERED,
    FILE_READ_DATA | FILE_WRITE_DATA,
);
const IOCTL_KCOM_HSMR_SHUTDOWN: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x821,
    METHOD_BUFFERED,
    FILE_READ_DATA | FILE_WRITE_DATA,
);

const HSMR_PACKET_BYTES: usize = 64;
const HSMR_STATUS_SLEEPING: u32 = 1;
const HSMR_VERSION_V1: u32 = 1;
const HSMR_VERSION_V2: u32 = 2;
const HSMR_INIT_FLAG_PAYLOAD: u32 = 0x1;
const HSMR_INIT_FLAG_FILE_WRITE_THROUGH: u32 = 0x2;
const HSMR_INIT_FLAG_FILE_NO_BUFFERING: u32 = 0x4;
const HSMR_FLAG_HAS_PAYLOAD: u32 = 0x2;
const HSMR_MODE_MVP: u32 = 0;
const HSMR_MODE_LARGE: u32 = 1;
const HSMR_IO_SLOT_BYTES: usize = 64 * 1024;
const HSMR_FILE_IO_ALIGN: u64 = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct IoFileRequest {
    op: u32,
    size: u32,
    offset: u64,
}

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
struct HsmrInitRequest {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    IoctlEcho,
    IoctlFile,
    IoctlFileAsync,
    Hsmr,
    HsmrFile,
    File,
}

impl Mode {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "ioctl-echo" => Some(Self::IoctlEcho),
            "ioctl-file" => Some(Self::IoctlFile),
            "ioctl-file-async" => Some(Self::IoctlFileAsync),
            "hsmr" => Some(Self::Hsmr),
            "hsmr-file" => Some(Self::HsmrFile),
            "file" => Some(Self::File),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::IoctlEcho => "ioctl-echo",
            Self::IoctlFile => "ioctl-file",
            Self::IoctlFileAsync => "ioctl-file-async",
            Self::Hsmr => "hsmr",
            Self::HsmrFile => "hsmr-file",
            Self::File => "file",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum HsmrMode {
    Mvp,
    Large,
}

impl HsmrMode {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "mvp" => Some(Self::Mvp),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    fn to_code(self) -> u32 {
        match self {
            Self::Mvp => HSMR_MODE_MVP,
            Self::Large => HSMR_MODE_LARGE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Read,
    Write,
    None,
}

impl Op {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::None => "-",
        }
    }
}

struct Counters {
    sum_ns: AtomicU64,
    min_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl Counters {
    fn new() -> Self {
        Self {
            sum_ns: AtomicU64::new(0),
            min_ns: AtomicU64::new(u64::MAX),
            max_ns: AtomicU64::new(0),
        }
    }

    fn record(&self, ns: u64) {
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        update_min(&self.min_ns, ns);
        update_max(&self.max_ns, ns);
    }
}

#[derive(Debug)]
struct BenchResult {
    device: String,
    path: String,
    mode: Mode,
    op: Op,
    size: u64,
    iterations: u64,
    parallelism: u64,
    elapsed_ns: u64,
    avg_latency_ns: u64,
    min_latency_ns: u64,
    max_latency_ns: u64,
    throughput_ops_per_s: f64,
    throughput_bytes_per_s: f64,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    op: Op,
    size: u64,
    iterations: u64,
    parallelism: u64,
    warmup: u64,
    runs: u64,
    threads: usize,
    device: String,
    path: String,
    offset: u64,
    offset_step: u64,
    prepare: bool,
    out_csv: Option<String>,
    append: bool,
    hsmr_mode: HsmrMode,
    hsmr_file_write_through: bool,
    hsmr_file_no_buffering: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut mode = Mode::IoctlEcho;
        let mut op = Op::Write;
        let mut size = 4096u64;
        let mut iterations = 200_000u64;
        let mut parallelism = 8u64;
        let mut warmup = 0u64;
        let mut runs = 1u64;
        let mut threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let mut device = "\\\\.\\KcomBench".to_string();
        let mut path = "C:\\Driver\\kcom_io_test.bin".to_string();
        let mut offset = 0u64;
        let mut offset_step: Option<u64> = None;
        let mut prepare = false;
        let mut out_csv = None;
        let mut append = false;
        let mut hsmr_mode = HsmrMode::Mvp;
        let mut hsmr_file_write_through = false;
        let mut hsmr_file_no_buffering = false;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--mode" => {
                    let value = args.next().ok_or("missing value for --mode")?;
                    mode =
                        Mode::from_str(&value).ok_or_else(|| format!("invalid mode: {}", value))?;
                }
                "--op" => {
                    let value = args.next().ok_or("missing value for --op")?;
                    op = Op::from_str(&value).ok_or_else(|| format!("invalid op: {}", value))?;
                }
                "--size" => {
                    let value = args.next().ok_or("missing value for --size")?;
                    size = parse_u64(&value, "--size")?;
                }
                "--iterations" => {
                    let value = args.next().ok_or("missing value for --iterations")?;
                    iterations = parse_u64(&value, "--iterations")?;
                }
                "--parallelism" => {
                    let value = args.next().ok_or("missing value for --parallelism")?;
                    parallelism = parse_u64(&value, "--parallelism")?;
                }
                "--warmup" => {
                    let value = args.next().ok_or("missing value for --warmup")?;
                    warmup = parse_u64(&value, "--warmup")?;
                }
                "--runs" => {
                    let value = args.next().ok_or("missing value for --runs")?;
                    runs = parse_u64(&value, "--runs")?;
                }
                "--threads" => {
                    let value = args.next().ok_or("missing value for --threads")?;
                    let parsed = parse_u64(&value, "--threads")?;
                    threads = parsed
                        .try_into()
                        .map_err(|_| "threads out of range".to_string())?;
                }
                "--device" => {
                    let value = args.next().ok_or("missing value for --device")?;
                    device = value;
                }
                "--path" => {
                    let value = args.next().ok_or("missing value for --path")?;
                    path = value;
                }
                "--offset" => {
                    let value = args.next().ok_or("missing value for --offset")?;
                    offset = parse_u64(&value, "--offset")?;
                }
                "--offset-step" => {
                    let value = args.next().ok_or("missing value for --offset-step")?;
                    offset_step = Some(parse_u64(&value, "--offset-step")?);
                }
                "--prepare" => {
                    prepare = true;
                }
                "--out-csv" => {
                    let value = args.next().ok_or("missing value for --out-csv")?;
                    out_csv = Some(value);
                }
                "--append" => {
                    append = true;
                }
                "--hsmr-mode" => {
                    let value = args.next().ok_or("missing value for --hsmr-mode")?;
                    hsmr_mode = HsmrMode::from_str(&value)
                        .ok_or_else(|| format!("invalid hsmr mode: {}", value))?;
                }
                "--hsmr-file-write-through" => {
                    hsmr_file_write_through = true;
                }
                "--hsmr-file-no-buffering" => {
                    hsmr_file_no_buffering = true;
                }
                "--help" | "-h" => {
                    return Err(usage());
                }
                other => {
                    return Err(format!("unknown argument: {}", other));
                }
            }
        }

        if iterations == 0 || parallelism == 0 || runs == 0 {
            return Err("iterations/parallelism/runs must be > 0".to_string());
        }
        if size == 0 {
            return Err("size must be > 0".to_string());
        }
        if size > u32::MAX as u64 {
            return Err("size out of range".to_string());
        }

        let offset_step = offset_step.unwrap_or(size);

        let op = match mode {
            Mode::IoctlEcho => Op::None,
            Mode::Hsmr => Op::Write,
            _ => op,
        };

        if mode == Mode::Hsmr {
            if op != Op::Write {
                return Err("hsmr mode supports write only".to_string());
            }
            if size != HSMR_PACKET_BYTES as u64 {
                return Err(format!("hsmr mode requires --size {}", HSMR_PACKET_BYTES));
            }
            if parallelism != 1 {
                return Err("hsmr mode requires --parallelism 1 (single producer)".to_string());
            }
            if threads != 1 {
                return Err("hsmr mode requires --threads 1".to_string());
            }
        } else if mode == Mode::HsmrFile {
            if op == Op::None {
                return Err("hsmr-file mode requires --op read|write".to_string());
            }
            if size as usize > HSMR_IO_SLOT_BYTES {
                return Err(format!(
                    "hsmr-file mode requires --size <= {}",
                    HSMR_IO_SLOT_BYTES
                ));
            }
            if threads != 1 {
                return Err("hsmr-file mode requires --threads 1".to_string());
            }
            if hsmr_file_no_buffering {
                if (size % HSMR_FILE_IO_ALIGN) != 0 {
                    return Err(format!(
                        "hsmr-file --hsmr-file-no-buffering requires --size multiple of {}",
                        HSMR_FILE_IO_ALIGN
                    ));
                }
                if (offset % HSMR_FILE_IO_ALIGN) != 0 {
                    return Err(format!(
                        "hsmr-file --hsmr-file-no-buffering requires --offset multiple of {}",
                        HSMR_FILE_IO_ALIGN
                    ));
                }
                if (offset_step % HSMR_FILE_IO_ALIGN) != 0 {
                    return Err(format!(
                        "hsmr-file --hsmr-file-no-buffering requires --offset-step multiple of {}",
                        HSMR_FILE_IO_ALIGN
                    ));
                }
            }
        }

        Ok(Self {
            mode,
            op,
            size,
            iterations,
            parallelism,
            warmup,
            runs,
            threads,
            device,
            path,
            offset,
            offset_step,
            prepare,
            out_csv,
            append,
            hsmr_mode,
            hsmr_file_write_through,
            hsmr_file_no_buffering,
        })
    }
}

fn usage() -> String {
    [
        "io-bench",
        "",
        "Usage:",
        "  cargo run -p io-bench -- [options]",
        "",
        "Options:",
        "  --mode <ioctl-echo|ioctl-file|ioctl-file-async|hsmr|hsmr-file|file>",
        "  --op <read|write>",
        "  --size <bytes>",
        "  --iterations <n>",
        "  --parallelism <n>",
        "  --warmup <n>",
        "  --runs <n>",
        "  --threads <n>",
        "  --device <\\\\.\\KcomBench>",
        "  --path <C:\\Driver\\kcom_io_test.bin>",
        "  --offset <n>",
        "  --offset-step <n>",
        "  --hsmr-mode <mvp|large>",
        "  --hsmr-file-write-through",
        "  --hsmr-file-no-buffering",
        "  --prepare",
        "  --out-csv <path>",
        "  --append",
    ]
    .join("\n")
}

fn parse_u64(value: &str, name: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid value for {}: {}", name, value))
}

fn now_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0));
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    format!("{}.{:09}", secs, nanos)
}

struct DeviceHandle {
    handle: HANDLE,
}

unsafe impl Send for DeviceHandle {}
unsafe impl Sync for DeviceHandle {}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        unsafe {
            if self.handle != INVALID_HANDLE_VALUE {
                CloseHandle(self.handle);
            }
        }
    }
}

struct HsmrShared {
    control: *mut HsmrControl,
    ring_ptr: *mut u8,
    overflow_ptr: *mut u8,
    ring_entries: u32,
    overflow_entries: u32,
    packet_bytes: u32,
    payload_ptr: *mut u8,
    payload_slot_bytes: u32,
    payload_slots: u32,
    doorbell: HANDLE,
    space: HANDLE,
}

unsafe impl Send for HsmrShared {}
unsafe impl Sync for HsmrShared {}

struct HsmrSession {
    handle: HANDLE,
    shared: Arc<HsmrShared>,
    active: bool,
}

impl HsmrSession {
    fn init(handle: HANDLE, mode: HsmrMode, init_flags: u32) -> Result<Self, String> {
        let doorbell = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) };
        if doorbell.is_null() {
            return Err(last_error("CreateEventW(doorbell)"));
        }
        let space = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) };
        if space.is_null() {
            unsafe { CloseHandle(doorbell) };
            return Err(last_error("CreateEventW(space)"));
        }

        let mut bytes = 0u32;
        let (
            user_ptr,
            control_offset,
            ring_offset,
            ring_entries,
            packet_bytes,
            overflow_offset,
            overflow_entries,
            payload_offset,
            payload_slot_bytes,
            payload_slots,
        ) = if (init_flags & HSMR_INIT_FLAG_PAYLOAD) != 0 {
            let mut req = HsmrInitRequestV2 {
                version: HSMR_VERSION_V2,
                mode: mode.to_code(),
                doorbell_event: doorbell,
                space_event: space,
                flags: init_flags,
                _reserved: 0,
            };
            let mut resp = HsmrInitResponseV2 {
                user_ptr: std::ptr::null_mut(),
                control_offset: 0,
                ring_offset: 0,
                ring_entries: 0,
                packet_bytes: 0,
                overflow_offset: 0,
                overflow_entries: 0,
                flags: 0,
                total_bytes: 0,
                payload_offset: 0,
                payload_slot_bytes: 0,
                payload_slots: 0,
                _reserved: 0,
            };

            let ok = unsafe {
                DeviceIoControl(
                    handle,
                    IOCTL_KCOM_HSMR_INIT,
                    &mut req as *mut _ as *mut c_void,
                    std::mem::size_of::<HsmrInitRequestV2>() as u32,
                    &mut resp as *mut _ as *mut c_void,
                    std::mem::size_of::<HsmrInitResponseV2>() as u32,
                    &mut bytes as *mut u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err(last_error("DeviceIoControl(HSMR_INIT)"));
            }
            if (bytes as usize) < std::mem::size_of::<HsmrInitResponseV2>() {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err(format!("HSMR_INIT returned short response: {}", bytes));
            }
            if (resp.flags & HSMR_FLAG_HAS_PAYLOAD) == 0 {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err("HSMR_INIT did not return payload flag".to_string());
            }

            (
                resp.user_ptr,
                resp.control_offset,
                resp.ring_offset,
                resp.ring_entries,
                resp.packet_bytes,
                resp.overflow_offset,
                resp.overflow_entries,
                resp.payload_offset,
                resp.payload_slot_bytes,
                resp.payload_slots,
            )
        } else {
            let mut req = HsmrInitRequest {
                version: HSMR_VERSION_V1,
                mode: mode.to_code(),
                doorbell_event: doorbell,
                space_event: space,
            };
            let mut resp = HsmrInitResponseV1 {
                user_ptr: std::ptr::null_mut(),
                control_offset: 0,
                ring_offset: 0,
                ring_entries: 0,
                packet_bytes: 0,
                overflow_offset: 0,
                overflow_entries: 0,
                flags: 0,
                total_bytes: 0,
            };

            let ok = unsafe {
                DeviceIoControl(
                    handle,
                    IOCTL_KCOM_HSMR_INIT,
                    &mut req as *mut _ as *mut c_void,
                    std::mem::size_of::<HsmrInitRequest>() as u32,
                    &mut resp as *mut _ as *mut c_void,
                    std::mem::size_of::<HsmrInitResponseV1>() as u32,
                    &mut bytes as *mut u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err(last_error("DeviceIoControl(HSMR_INIT)"));
            }
            if (bytes as usize) < std::mem::size_of::<HsmrInitResponseV1>() {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err(format!("HSMR_INIT returned short response: {}", bytes));
            }

            (
                resp.user_ptr,
                resp.control_offset,
                resp.ring_offset,
                resp.ring_entries,
                resp.packet_bytes,
                resp.overflow_offset,
                resp.overflow_entries,
                0u32,
                0u32,
                0u32,
            )
        };
        let want_payload = (init_flags & HSMR_INIT_FLAG_PAYLOAD) != 0;

        if user_ptr.is_null() {
            unsafe {
                CloseHandle(space);
                CloseHandle(doorbell);
            }
            return Err("HSMR_INIT returned null user pointer".to_string());
        }
        if ring_entries == 0 || overflow_entries == 0 {
            unsafe {
                CloseHandle(space);
                CloseHandle(doorbell);
            }
            return Err("HSMR_INIT returned zero ring entries".to_string());
        }
        if ring_entries & (ring_entries - 1) != 0 {
            unsafe {
                CloseHandle(space);
                CloseHandle(doorbell);
            }
            return Err("HSMR ring entries must be power of two".to_string());
        }
        if overflow_entries & (overflow_entries - 1) != 0 {
            unsafe {
                CloseHandle(space);
                CloseHandle(doorbell);
            }
            return Err("HSMR overflow entries must be power of two".to_string());
        }
        if packet_bytes as usize != HSMR_PACKET_BYTES {
            unsafe {
                CloseHandle(space);
                CloseHandle(doorbell);
            }
            return Err(format!("unexpected packet size: {}", packet_bytes));
        }

        if want_payload {
            if (payload_slots & (payload_slots - 1)) != 0 {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err("HSMR payload slots must be power of two".to_string());
            }
            if payload_slot_bytes as usize != HSMR_IO_SLOT_BYTES {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err(format!(
                    "unexpected HSMR payload slot size: {}",
                    payload_slot_bytes
                ));
            }
            if (payload_slot_bytes as usize) < HSMR_PACKET_BYTES {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err("HSMR payload slot size too small".to_string());
            }
            if (payload_slots == 0) || (payload_offset == 0) {
                unsafe {
                    CloseHandle(space);
                    CloseHandle(doorbell);
                }
                return Err("HSMR payload mapping missing in response".to_string());
            }
        }

        let base = user_ptr as *mut u8;
        let control = unsafe { base.add(control_offset as usize) } as *mut HsmrControl;
        let ring_ptr = unsafe { base.add(ring_offset as usize) };
        let overflow_ptr = unsafe { base.add(overflow_offset as usize) };
        let payload_ptr = if want_payload {
            unsafe { base.add(payload_offset as usize) }
        } else {
            std::ptr::null_mut()
        };

        let shared = Arc::new(HsmrShared {
            control,
            ring_ptr,
            overflow_ptr,
            ring_entries,
            overflow_entries,
            packet_bytes,
            payload_ptr,
            payload_slot_bytes,
            payload_slots,
            doorbell,
            space,
        });

        Ok(Self {
            handle,
            shared,
            active: true,
        })
    }

    fn shutdown(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        let mut bytes = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_KCOM_HSMR_SHUTDOWN,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                0,
                &mut bytes as *mut u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error("DeviceIoControl(HSMR_SHUTDOWN)"));
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for HsmrSession {
    fn drop(&mut self) {
        if self.active {
            let _ = self.shutdown();
        }
        unsafe {
            CloseHandle(self.shared.space);
            CloseHandle(self.shared.doorbell);
        }
    }
}

struct HsmrProducer {
    shared: Arc<HsmrShared>,
    cached_head: u32,
    cached_overflow_head: u32,
}

impl HsmrProducer {
    fn new(shared: Arc<HsmrShared>) -> Self {
        let control = unsafe { &*shared.control };
        let cached_head = control.head.load(Ordering::Acquire);
        let cached_overflow_head = control.overflow_head.load(Ordering::Acquire);
        Self {
            shared,
            cached_head,
            cached_overflow_head,
        }
    }

    fn submit_raw(&mut self, packet: &[u8]) -> Result<(), String> {
        if packet.len() != self.shared.packet_bytes as usize {
            return Err("packet size mismatch".to_string());
        }
        let control = unsafe { &*self.shared.control };
        let tail = control.tail.load(Ordering::Acquire);
        let ring_entries = self.shared.ring_entries;
        let space = ring_entries.wrapping_sub(tail.wrapping_sub(self.cached_head));
        let mut space = space;
        if space == 0 {
            self.cached_head = control.head.load(Ordering::Acquire);
            space = ring_entries.wrapping_sub(tail.wrapping_sub(self.cached_head));
        }
        if space == 0 {
            return self.submit_overflow(packet);
        }

        let idx = (tail & (ring_entries - 1)) as usize;
        let dest = unsafe {
            self.shared
                .ring_ptr
                .add(idx * self.shared.packet_bytes as usize)
        };
        unsafe {
            std::ptr::copy_nonoverlapping(packet.as_ptr(), dest, packet.len());
        }
        std::sync::atomic::fence(Ordering::Release);
        control.tail.store(tail.wrapping_add(1), Ordering::Release);

        if control.kernel_status.load(Ordering::Acquire) == HSMR_STATUS_SLEEPING {
            unsafe {
                SetEvent(self.shared.doorbell);
            }
        }
        Ok(())
    }

    fn submit_primary(&mut self, packet: &[u8]) -> Result<u32, String> {
        if packet.len() != self.shared.packet_bytes as usize {
            return Err("packet size mismatch".to_string());
        }
        let control = unsafe { &*self.shared.control };
        for _ in 0..1000 {
            let tail = control.tail.load(Ordering::Acquire);
            let ring_entries = self.shared.ring_entries;
            let space = ring_entries.wrapping_sub(tail.wrapping_sub(self.cached_head));
            let mut space = space;
            if space == 0 {
                self.cached_head = control.head.load(Ordering::Acquire);
                space = ring_entries.wrapping_sub(tail.wrapping_sub(self.cached_head));
            }
            if space == 0 {
                unsafe {
                    if !self.shared.space.is_null() {
                        let _ = WaitForSingleObject(self.shared.space, 1);
                    } else {
                        std::thread::yield_now();
                    }
                }
                continue;
            }

            let idx = (tail & (ring_entries - 1)) as usize;
            let dest = unsafe {
                self.shared
                    .ring_ptr
                    .add(idx * self.shared.packet_bytes as usize)
            };
            unsafe {
                std::ptr::copy_nonoverlapping(packet.as_ptr(), dest, packet.len());
            }
            std::sync::atomic::fence(Ordering::Release);
            control.tail.store(tail.wrapping_add(1), Ordering::Release);

            if control.kernel_status.load(Ordering::Acquire) == HSMR_STATUS_SLEEPING {
                unsafe {
                    SetEvent(self.shared.doorbell);
                }
            }
            return Ok(tail);
        }
        Err("primary ring is full".to_string())
    }

    fn wait_for_head_at_least(&self, target: u32) -> Result<(), String> {
        let control = unsafe { &*self.shared.control };
        let mut spins = 0u32;
        loop {
            let head = control.head.load(Ordering::Acquire);
            if seq_ge(head, target) {
                return Ok(());
            }

            if spins < 10_000 {
                spins += 1;
                std::hint::spin_loop();
                continue;
            }
            spins = 0;
            unsafe {
                if !self.shared.space.is_null() {
                    let _ = WaitForSingleObject(self.shared.space, 1);
                } else {
                    std::thread::yield_now();
                }
            }
        }
    }

    fn read_packet(&self, seq: u32) -> HsmrPacket {
        let idx = (seq & (self.shared.ring_entries - 1)) as usize;
        let ptr = unsafe {
            self.shared
                .ring_ptr
                .add(idx * self.shared.packet_bytes as usize)
        } as *const HsmrPacket;
        unsafe { std::ptr::read_unaligned(ptr) }
    }

    fn payload_slot_ptr(&self, seq: u32) -> Result<*mut u8, String> {
        if self.shared.payload_ptr.is_null() {
            return Err("payload mapping is not available".to_string());
        }
        if self.shared.payload_slots == 0
            || (self.shared.payload_slots & (self.shared.payload_slots - 1)) != 0
        {
            return Err("payload slots must be power of two".to_string());
        }
        let slot_mask = self.shared.payload_slots - 1;
        let slot_idx = (seq & slot_mask) as usize;
        let slot_bytes = self.shared.payload_slot_bytes as usize;
        Ok(unsafe { self.shared.payload_ptr.add(slot_idx * slot_bytes) })
    }

    fn submit_overflow(&mut self, packet: &[u8]) -> Result<(), String> {
        let control = unsafe { &*self.shared.control };
        for _ in 0..1000 {
            let tail = control.overflow_tail.load(Ordering::Acquire);
            let entries = self.shared.overflow_entries;
            let space = entries.wrapping_sub(tail.wrapping_sub(self.cached_overflow_head));
            let mut space = space;
            if space == 0 {
                self.cached_overflow_head = control.overflow_head.load(Ordering::Acquire);
                space = entries.wrapping_sub(tail.wrapping_sub(self.cached_overflow_head));
            }
            if space > 0 {
                let idx = (tail & (entries - 1)) as usize;
                let dest = unsafe {
                    self.shared
                        .overflow_ptr
                        .add(idx * self.shared.packet_bytes as usize)
                };
                unsafe {
                    std::ptr::copy_nonoverlapping(packet.as_ptr(), dest, packet.len());
                }
                std::sync::atomic::fence(Ordering::Release);
                control
                    .overflow_tail
                    .store(tail.wrapping_add(1), Ordering::Release);
                if control.kernel_status.load(Ordering::Acquire) == HSMR_STATUS_SLEEPING {
                    unsafe {
                        SetEvent(self.shared.doorbell);
                    }
                }
                return Ok(());
            }

            unsafe {
                if !self.shared.space.is_null() {
                    let _ = WaitForSingleObject(self.shared.space, 1);
                } else {
                    std::thread::yield_now();
                }
            }
        }

        Err("overflow ring is full".to_string())
    }
}

#[inline]
fn seq_ge(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}

fn open_device(path: &str) -> Result<DeviceHandle, String> {
    let wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW"));
    }
    Ok(DeviceHandle { handle })
}

fn ioctl_echo(handle: HANDLE, size: usize) -> Result<(), String> {
    let mut buffer = vec![0u8; size];
    let mut bytes = 0u32;
    let in_ptr = if size == 0 {
        std::ptr::null_mut()
    } else {
        buffer.as_mut_ptr() as *mut core::ffi::c_void
    };
    let out_ptr = in_ptr;

    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_KCOM_IO_ECHO,
            in_ptr,
            size as u32,
            out_ptr,
            size as u32,
            &mut bytes as *mut u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("DeviceIoControl"));
    }
    Ok(())
}

fn ioctl_file(handle: HANDLE, op: Op, size: usize, offset: u64) -> Result<(), String> {
    ioctl_file_with_code(handle, IOCTL_KCOM_IO_FILE, op, size, offset)
}

fn ioctl_file_async(handle: HANDLE, op: Op, size: usize, offset: u64) -> Result<(), String> {
    ioctl_file_with_code(handle, IOCTL_KCOM_IO_FILE_ASYNC, op, size, offset)
}

fn ioctl_file_with_code(
    handle: HANDLE,
    code: u32,
    op: Op,
    size: usize,
    offset: u64,
) -> Result<(), String> {
    let total = std::mem::size_of::<IoFileRequest>()
        .checked_add(size)
        .ok_or_else(|| "buffer size overflow".to_string())?;
    let mut buffer = vec![0u8; total];

    let req = IoFileRequest {
        op: match op {
            Op::Read => 0,
            Op::Write => 1,
            Op::None => 0,
        },
        size: size as u32,
        offset,
    };
    unsafe {
        let req_ptr = buffer.as_mut_ptr() as *mut IoFileRequest;
        req_ptr.write(req);
    }

    let in_len = total as u32;
    let out_len = match op {
        Op::Read => total as u32,
        _ => 0,
    };

    let mut bytes = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            code,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            in_len,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            out_len,
            &mut bytes as *mut u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("DeviceIoControl"));
    }
    Ok(())
}

fn prepare_file(path: &str, required_len: u64) -> io::Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)?;
    file.set_len(required_len)
}

fn open_file(path: &str) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
}

fn file_read(file: &std::fs::File, size: usize, offset: u64) -> io::Result<()> {
    let mut buffer = vec![0u8; size];
    let bytes = file.seek_read(&mut buffer, offset)?;
    if bytes != size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read"));
    }
    Ok(())
}

fn file_write(file: &std::fs::File, size: usize, offset: u64) -> io::Result<()> {
    let buffer = vec![0u8; size];
    let bytes = file.seek_write(&buffer, offset)?;
    if bytes != size {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "short write"));
    }
    Ok(())
}

fn run_hsmr_bench(
    args: &Args,
    device: &DeviceHandle,
    iterations: u64,
) -> Result<BenchResult, String> {
    let mut session = HsmrSession::init(device.handle, args.hsmr_mode, 0)?;
    let shared = session.shared.clone();
    let mut producer = HsmrProducer::new(shared.clone());
    let packet = vec![0u8; shared.packet_bytes as usize];

    let counters = Counters::new();
    let bench_start = Instant::now();
    for _ in 0..iterations {
        let start = Instant::now();
        producer.submit_raw(&packet)?;
        let elapsed = start.elapsed().as_nanos() as u64;
        counters.record(elapsed);
    }

    let elapsed_ns = bench_start.elapsed().as_nanos() as u64;
    let sum_ns = counters.sum_ns.load(Ordering::Relaxed);
    let min_ns = counters.min_ns.load(Ordering::Relaxed);
    let max_ns = counters.max_ns.load(Ordering::Relaxed);
    let avg_latency_ns = if iterations == 0 {
        0
    } else {
        sum_ns / iterations
    };
    let min_latency_ns = if min_ns == u64::MAX { 0 } else { min_ns };
    let throughput_ops_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        (iterations as f64) * 1_000_000_000.0 / (elapsed_ns as f64)
    };
    let bytes_total = (args.size as f64) * (iterations as f64);
    let throughput_bytes_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        bytes_total * 1_000_000_000.0 / (elapsed_ns as f64)
    };

    session.shutdown()?;

    Ok(BenchResult {
        device: args.device.clone(),
        path: "-".to_string(),
        mode: args.mode,
        op: args.op,
        size: args.size,
        iterations,
        parallelism: args.parallelism,
        elapsed_ns,
        avg_latency_ns,
        min_latency_ns,
        max_latency_ns: max_ns,
        throughput_ops_per_s,
        throughput_bytes_per_s,
    })
}

fn run_hsmr_file_bench(
    args: &Args,
    device: &DeviceHandle,
    iterations: u64,
) -> Result<BenchResult, String> {
    let mut init_flags = HSMR_INIT_FLAG_PAYLOAD;
    if args.hsmr_file_write_through {
        init_flags |= HSMR_INIT_FLAG_FILE_WRITE_THROUGH;
    }
    if args.hsmr_file_no_buffering {
        init_flags |= HSMR_INIT_FLAG_FILE_NO_BUFFERING;
    }

    let mut session = HsmrSession::init(device.handle, args.hsmr_mode, init_flags)?;
    let shared = session.shared.clone();
    let control = unsafe { &*shared.control };
    let mut producer = HsmrProducer::new(shared.clone());

    if shared.payload_ptr.is_null() {
        return Err("HSMR payload mapping missing".to_string());
    }
    if shared.payload_slot_bytes as usize != HSMR_IO_SLOT_BYTES {
        return Err(format!(
            "unexpected payload slot bytes: {}",
            shared.payload_slot_bytes
        ));
    }
    if args.hsmr_file_no_buffering {
        let align_mask = (HSMR_FILE_IO_ALIGN as usize).saturating_sub(1);
        if (shared.payload_ptr as usize) & align_mask != 0 {
            return Err(format!(
                "hsmr-file payload pointer is not {}-byte aligned: 0x{:X}",
                HSMR_FILE_IO_ALIGN, shared.payload_ptr as usize
            ));
        }
        if (shared.payload_slot_bytes as u64) % HSMR_FILE_IO_ALIGN != 0 {
            return Err(format!(
                "hsmr-file payload slot size {} is not multiple of {}",
                shared.payload_slot_bytes, HSMR_FILE_IO_ALIGN
            ));
        }
    }

    let op_code = match args.op {
        Op::Write => HSMR_OP_FILE_WRITE,
        Op::Read => HSMR_OP_FILE_READ,
        Op::None => return Err("hsmr-file requires --op read|write".to_string()),
    };

    let size = args.size as usize;
    if size == 0 || size > HSMR_IO_SLOT_BYTES {
        return Err(format!("invalid hsmr-file size: {}", args.size));
    }

    fn drain_completed(
        producer: &HsmrProducer,
        control: &HsmrControl,
        in_flight: &mut VecDeque<(u32, Instant)>,
        size: usize,
        counters: &Counters,
    ) -> Result<(), String> {
        loop {
            let Some(&(seq, _)) = in_flight.front() else {
                return Ok(());
            };

            let head = control.head.load(Ordering::Acquire);
            if !seq_ge(head, seq.wrapping_add(1)) {
                return Ok(());
            }

            let (seq, start) = in_flight.pop_front().unwrap();
            let done = producer.read_packet(seq);
            if done.status < 0 {
                return Err(format!(
                    "hsmr-file op failed: status=0x{:08X}",
                    done.status as u32
                ));
            }
            if done.bytes as usize != size {
                return Err(format!("hsmr-file short io: {}", done.bytes));
            }

            let elapsed = start.elapsed().as_nanos() as u64;
            counters.record(elapsed);
        }
    }

    let queue_depth = usize::try_from(args.parallelism)
        .map_err(|_| format!("invalid queue depth: {}", args.parallelism))?;
    if queue_depth == 0 {
        return Err("hsmr-file requires --parallelism >= 1".to_string());
    }
    if queue_depth > shared.payload_slots as usize {
        return Err(format!(
            "hsmr-file queue depth {} exceeds payload slots {}",
            queue_depth, shared.payload_slots
        ));
    }

    let mut in_flight: VecDeque<(u32, Instant)> = VecDeque::new();
    let counters = Counters::new();
    let bench_start = Instant::now();

    for idx in 0..iterations {
        // Maintain a fixed number of in-flight operations (queue depth).
        if in_flight.len() >= queue_depth {
            let target = in_flight
                .front()
                .map(|(seq, _)| seq.wrapping_add(1))
                .unwrap_or(0);
            producer.wait_for_head_at_least(target)?;
            drain_completed(&producer, control, &mut in_flight, size, &counters)?;
        }

        let start = Instant::now();
        let offset = args
            .offset
            .checked_add(args.offset_step.saturating_mul(idx))
            .ok_or_else(|| "offset overflow".to_string())?;

        // Compute the payload slot from the next sequence number.
        let seq = control.tail.load(Ordering::Acquire);
        let slot_ptr = producer.payload_slot_ptr(seq)?;
        if args.op == Op::Write {
            unsafe { std::ptr::write_bytes(slot_ptr, 0, size) };
        }

        let packet = HsmrPacket {
            op: op_code,
            size: size as u32,
            offset,
            status: 0,
            bytes: 0,
            _pad: [0u8; HSMR_PACKET_BYTES - 24],
        };
        let packet_bytes = unsafe {
            std::slice::from_raw_parts(
                &packet as *const HsmrPacket as *const u8,
                HSMR_PACKET_BYTES,
            )
        };

        let submitted = producer.submit_primary(packet_bytes)?;
        in_flight.push_back((submitted, start));

        // Opportunistically drain completions to reduce latency skew.
        drain_completed(&producer, control, &mut in_flight, size, &counters)?;
    }

    while let Some((seq, _)) = in_flight.front().copied() {
        producer.wait_for_head_at_least(seq.wrapping_add(1))?;
        drain_completed(&producer, control, &mut in_flight, size, &counters)?;
    }

    let elapsed_ns = bench_start.elapsed().as_nanos() as u64;
    let sum_ns = counters.sum_ns.load(Ordering::Relaxed);
    let min_ns = counters.min_ns.load(Ordering::Relaxed);
    let max_ns = counters.max_ns.load(Ordering::Relaxed);
    let avg_latency_ns = if iterations == 0 {
        0
    } else {
        sum_ns / iterations
    };
    let min_latency_ns = if min_ns == u64::MAX { 0 } else { min_ns };
    let throughput_ops_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        (iterations as f64) * 1_000_000_000.0 / (elapsed_ns as f64)
    };
    let bytes_total = (args.size as f64) * (iterations as f64);
    let throughput_bytes_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        bytes_total * 1_000_000_000.0 / (elapsed_ns as f64)
    };

    session.shutdown()?;

    Ok(BenchResult {
        device: args.device.clone(),
        path: args.path.clone(),
        mode: args.mode,
        op: args.op,
        size: args.size,
        iterations,
        parallelism: args.parallelism,
        elapsed_ns,
        avg_latency_ns,
        min_latency_ns,
        max_latency_ns: max_ns,
        throughput_ops_per_s,
        throughput_bytes_per_s,
    })
}

async fn run_bench_loop(
    args: &Args,
    device: Option<Arc<DeviceHandle>>,
    file: Option<Arc<std::fs::File>>,
    iterations: u64,
) -> Result<BenchResult, String> {
    match args.mode {
        Mode::Hsmr => {
            let device = device.as_ref().ok_or("device handle missing")?;
            return run_hsmr_bench(args, device, iterations);
        }
        Mode::HsmrFile => {
            let device = device.as_ref().ok_or("device handle missing")?;
            return run_hsmr_file_bench(args, device, iterations);
        }
        _ => {}
    }

    let mode = args.mode;
    let op = args.op;
    let size_u64 = args.size;
    let offset_base = args.offset;
    let offset_step = args.offset_step;

    let counters = Arc::new(Counters::new());
    let semaphore = Arc::new(Semaphore::new(args.parallelism as usize));
    let mut join_set = JoinSet::new();

    let bench_start = Instant::now();

    for idx in 0..iterations {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let counters = counters.clone();
        let device = device.clone();
        let file = file.clone();

        join_set.spawn(async move {
            let _permit = permit;
            let start = Instant::now();
            let offset = offset_base
                .checked_add(offset_step.saturating_mul(idx))
                .ok_or_else(|| "offset overflow".to_string())?;
            let size = size_u64 as usize;

            match mode {
                Mode::IoctlEcho => {
                    let handle = device.as_ref().ok_or("device handle missing")?;
                    let handle = handle.handle as usize;
                    tokio::task::spawn_blocking(move || ioctl_echo(handle as HANDLE, size))
                        .await
                        .map_err(|e| format!("join error: {}", e))??;
                }
                Mode::IoctlFile => {
                    let handle = device.as_ref().ok_or("device handle missing")?;
                    let handle = handle.handle as usize;
                    tokio::task::spawn_blocking(move || {
                        ioctl_file(handle as HANDLE, op, size, offset)
                    })
                    .await
                    .map_err(|e| format!("join error: {}", e))??;
                }
                Mode::IoctlFileAsync => {
                    let handle = device.as_ref().ok_or("device handle missing")?;
                    let handle = handle.handle as usize;
                    tokio::task::spawn_blocking(move || {
                        ioctl_file_async(handle as HANDLE, op, size, offset)
                    })
                    .await
                    .map_err(|e| format!("join error: {}", e))??;
                }
                Mode::File => {
                    let file = file.as_ref().ok_or("file handle missing")?;
                    let file = file.clone();
                    tokio::task::spawn_blocking(move || match op {
                        Op::Read => file_read(&file, size, offset).map_err(|e| e.to_string()),
                        Op::Write => file_write(&file, size, offset).map_err(|e| e.to_string()),
                        Op::None => Ok(()),
                    })
                    .await
                    .map_err(|e| format!("join error: {}", e))??;
                }
                Mode::Hsmr | Mode::HsmrFile => {
                    return Err("internal error: hsmr must use sequential runner".to_string())
                }
            }

            let elapsed = start.elapsed().as_nanos() as u64;
            counters.record(elapsed);
            Ok::<(), String>(())
        });
    }

    let mut first_error: Option<String> = None;
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(format!("join error: {}", err));
                }
            }
        }
    }

    if let Some(err) = first_error {
        return Err(err);
    }

    let elapsed_ns = bench_start.elapsed().as_nanos() as u64;
    let sum_ns = counters.sum_ns.load(Ordering::Relaxed);
    let min_ns = counters.min_ns.load(Ordering::Relaxed);
    let max_ns = counters.max_ns.load(Ordering::Relaxed);
    let avg_latency_ns = if iterations == 0 {
        0
    } else {
        sum_ns / iterations
    };
    let min_latency_ns = if min_ns == u64::MAX { 0 } else { min_ns };
    let throughput_ops_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        (iterations as f64) * 1_000_000_000.0 / (elapsed_ns as f64)
    };
    let bytes_total = (args.size as f64) * (iterations as f64);
    let throughput_bytes_per_s = if elapsed_ns == 0 {
        0.0
    } else {
        bytes_total * 1_000_000_000.0 / (elapsed_ns as f64)
    };

    let device_name = match args.mode {
        Mode::File => "-".to_string(),
        _ => args.device.clone(),
    };
    let path_name = match args.mode {
        Mode::IoctlEcho => "-".to_string(),
        _ => args.path.clone(),
    };

    Ok(BenchResult {
        device: device_name,
        path: path_name,
        mode: args.mode,
        op: args.op,
        size: args.size,
        iterations,
        parallelism: args.parallelism,
        elapsed_ns,
        avg_latency_ns,
        min_latency_ns,
        max_latency_ns: max_ns,
        throughput_ops_per_s,
        throughput_bytes_per_s,
    })
}

fn print_result(result: &BenchResult) {
    println!("timestamp            : {}", now_timestamp());
    println!("device               : {}", result.device);
    println!("path                 : {}", result.path);
    println!("mode                 : {}", result.mode.as_str());
    println!("op                   : {}", result.op.as_str());
    println!("size_bytes           : {}", result.size);
    println!("iterations           : {}", result.iterations);
    println!("parallelism          : {}", result.parallelism);
    println!("elapsed_ns           : {}", result.elapsed_ns);
    println!("avg_latency_ns       : {}", result.avg_latency_ns);
    println!("min_latency_ns       : {}", result.min_latency_ns);
    println!("max_latency_ns       : {}", result.max_latency_ns);
    println!("throughput_ops_per_s : {:.3}", result.throughput_ops_per_s);
    println!(
        "throughput_bytes_s   : {:.3}",
        result.throughput_bytes_per_s
    );
    println!();
}

fn csv_escape(value: &str) -> String {
    let escaped = value.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

fn write_csv_header<W: std::io::Write>(writer: &mut W) -> std::io::Result<()> {
    writeln!(
        writer,
        "timestamp,device,path,mode,op,size_bytes,iterations,parallelism,elapsed_ns,avg_latency_ns,min_latency_ns,max_latency_ns,throughput_ops_per_s,throughput_bytes_s"
    )
}

fn write_csv_row<W: std::io::Write>(writer: &mut W, result: &BenchResult) -> std::io::Result<()> {
    let timestamp = csv_escape(&now_timestamp());
    let device = csv_escape(&result.device);
    let path = csv_escape(&result.path);
    let mode = csv_escape(result.mode.as_str());
    let op = csv_escape(result.op.as_str());
    let size = csv_escape(&result.size.to_string());
    let iterations = csv_escape(&result.iterations.to_string());
    let parallelism = csv_escape(&result.parallelism.to_string());
    let elapsed_ns = csv_escape(&result.elapsed_ns.to_string());
    let avg_latency_ns = csv_escape(&result.avg_latency_ns.to_string());
    let min_latency_ns = csv_escape(&result.min_latency_ns.to_string());
    let max_latency_ns = csv_escape(&result.max_latency_ns.to_string());
    let throughput_ops = csv_escape(&format!("{:.3}", result.throughput_ops_per_s));
    let throughput_bytes = csv_escape(&format!("{:.3}", result.throughput_bytes_per_s));
    writeln!(
        writer,
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        timestamp,
        device,
        path,
        mode,
        op,
        size,
        iterations,
        parallelism,
        elapsed_ns,
        avg_latency_ns,
        min_latency_ns,
        max_latency_ns,
        throughput_ops,
        throughput_bytes
    )
}

fn prepare_csv(path: &str, append: bool) -> std::io::Result<()> {
    if append {
        if !std::path::Path::new(path).exists() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?;
            write_csv_header(&mut file)?;
        }
    } else {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        write_csv_header(&mut file)?;
    }
    Ok(())
}

fn append_csv(path: &str, result: &BenchResult) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    write_csv_row(&mut file, result)
}

fn calc_required_len(args: &Args, ops: u64) -> Option<u64> {
    if ops == 0 {
        return Some(args.offset.saturating_add(args.size));
    }
    let step = args.offset_step;
    let span = step.checked_mul(ops.saturating_sub(1))?;
    let end = args.offset.checked_add(span)?;
    end.checked_add(args.size)
}

fn last_error(context: &str) -> String {
    let err = unsafe { GetLastError() };
    format!("{} failed: {}", context, err)
}

fn main() {
    let args = match Args::parse() {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("{}", msg);
            std::process::exit(2);
        }
    };

    if let Some(ref path) = args.out_csv {
        if let Err(err) = prepare_csv(path, args.append) {
            eprintln!("failed to prepare csv {}: {}", path, err);
            std::process::exit(2);
        }
    }

    if args.prepare {
        let ops = std::cmp::max(args.iterations, args.warmup);
        let required = match calc_required_len(&args, ops) {
            Some(value) => value,
            None => {
                eprintln!("file length overflow");
                std::process::exit(2);
            }
        };
        if let Err(err) = prepare_file(&args.path, required) {
            eprintln!("failed to prepare file {}: {}", args.path, err);
            std::process::exit(2);
        }
    }

    let device = match args.mode {
        Mode::IoctlEcho | Mode::IoctlFile | Mode::IoctlFileAsync | Mode::Hsmr | Mode::HsmrFile => {
            match open_device(&args.device) {
                Ok(handle) => Some(Arc::new(handle)),
                Err(err) => {
                    eprintln!("{}", err);
                    std::process::exit(2);
                }
            }
        }
        Mode::File => None,
    };

    let file = match args.mode {
        Mode::File => match open_file(&args.path) {
            Ok(file) => Some(Arc::new(file)),
            Err(err) => {
                eprintln!("failed to open file {}: {}", args.path, err);
                std::process::exit(2);
            }
        },
        _ => None,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_all()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async {
        if args.warmup > 0 {
            let _ = run_bench_loop(&args, device.clone(), file.clone(), args.warmup).await;
        }

        for _ in 0..args.runs {
            match run_bench_loop(&args, device.clone(), file.clone(), args.iterations).await {
                Ok(result) => {
                    print_result(&result);
                    if let Some(ref path) = args.out_csv {
                        if let Err(err) = append_csv(path, &result) {
                            eprintln!("failed to append csv {}: {}", path, err);
                            std::process::exit(2);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("bench failed: {}", err);
                    std::process::exit(2);
                }
            }
        }
    });
}

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

fn update_min(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value < current {
        match target.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}

fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}
