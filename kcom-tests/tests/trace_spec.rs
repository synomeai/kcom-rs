use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use kcom::{clear_trace_hook, ensure, set_trace_hook, NTSTATUS};
use kcom::iunknown::STATUS_UNSUCCESSFUL;

static TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
static TRACE_MSG: Mutex<Option<String>> = Mutex::new(None);
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn trace_hook(args: fmt::Arguments<'_>) {
    TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut msg = String::new();
    let _ = fmt::write(&mut msg, args);
    *TRACE_MSG.lock().unwrap() = Some(msg);
}

fn fail_with_ensure() -> Result<(), NTSTATUS> {
    ensure!(false, STATUS_UNSUCCESSFUL, "boom {}", 7);
    Ok(())
}

struct TraceGuard;

impl Drop for TraceGuard {
    fn drop(&mut self) {
        clear_trace_hook();
    }
}

#[test]
fn ensure_reports_trace_when_debug() {
    let _guard = TEST_LOCK.lock().unwrap();
    let _trace_guard = TraceGuard;

    TRACE_COUNT.store(0, Ordering::Relaxed);
    *TRACE_MSG.lock().unwrap() = None;
    set_trace_hook(trace_hook);

    let err = fail_with_ensure().unwrap_err();
    assert_eq!(err, STATUS_UNSUCCESSFUL);

    #[cfg(debug_assertions)]
    {
        assert_eq!(TRACE_COUNT.load(Ordering::Relaxed), 1);
        let msg = TRACE_MSG.lock().unwrap().clone().unwrap_or_default();
        assert!(msg.contains("kcom error"));
        assert!(msg.contains("boom 7"));
    }

    #[cfg(not(debug_assertions))]
    {
        assert_eq!(TRACE_COUNT.load(Ordering::Relaxed), 0);
    }
}
