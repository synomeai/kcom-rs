// trace.rs
//
// Lightweight tracing hooks for no_std builds.

use core::fmt;
use core::sync::atomic::{AtomicPtr, Ordering};

/// Trace hook invoked with formatted arguments.
pub type TraceHook = for<'a> fn(fmt::Arguments<'a>);

static TRACE_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers a trace hook.
#[inline]
pub fn set_trace_hook(hook: TraceHook) {
    TRACE_HOOK.store(hook as *const () as *mut (), Ordering::Release);
}

/// Clears the trace hook.
#[inline]
pub fn clear_trace_hook() {
    TRACE_HOOK.store(core::ptr::null_mut(), Ordering::Release);
}

/// Emits a trace event if a hook is installed.
#[inline]
pub fn trace(args: fmt::Arguments<'_>) {
    let ptr = TRACE_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        return;
    }
    let hook: TraceHook = unsafe { core::mem::transmute(ptr) };
    hook(args);
}

/// Debug-only error report helper.
#[inline]
pub fn report_error(file: &str, line: u32, status: crate::NTSTATUS) {
    trace(format_args!("kcom error {:#x} at {}:{}", status, file, line));
}

/// Debug-only error report helper with message.
#[inline]
pub fn report_error_msg(
    file: &str,
    line: u32,
    status: crate::NTSTATUS,
    msg: fmt::Arguments<'_>,
) {
    trace(format_args!(
        "kcom error {:#x} at {}:{} - {}",
        status, file, line, msg
    ));
}
