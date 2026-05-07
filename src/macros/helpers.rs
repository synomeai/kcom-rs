// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0



#[macro_export]
/// Returns early with `Err(status)` when `cond` is false.
macro_rules! ensure {
    ($cond:expr, $status:expr $(,)?) => {
        if !$cond {
            #[cfg(debug_assertions)]
            $crate::trace::report_error(file!(), line!(), $status);
            return Err($status);
        }
    };
    ($cond:expr, $status:expr, $($arg:tt)+) => {
        if !$cond {
            #[cfg(debug_assertions)]
            $crate::trace::report_error_msg(
                file!(),
                line!(),
                $status,
                core::format_args!($($arg)+),
            );
            return Err($status);
        }
    };
}

#[macro_export]
/// Emits a trace event if a hook is installed.
macro_rules! trace {
    ($($arg:tt)*) => {
        $crate::trace::trace(core::format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! iunknown_vtbl {
    ($ty:ty, $vtbl:ty $(,)?) => {
        $crate::IUnknownVtbl {
            QueryInterface: $crate::wrapper::ComObject::<$ty, $vtbl>::shim_query_interface,
            AddRef: $crate::wrapper::ComObject::<$ty, $vtbl>::shim_add_ref,
            Release: $crate::wrapper::ComObject::<$ty, $vtbl>::shim_release,
        }
    };
}

#[macro_export]
macro_rules! pin_init {
    ($value:expr) => {
        $crate::allocator::PinInitOnce::new(|ptr| {
            // SAFETY: caller guarantees `ptr` is valid for writes.
            unsafe { core::ptr::write(ptr, $value) };
            ::core::result::Result::<(), _>::Ok(())
        })
    };
    (|$ptr:ident| $body:block) => {
        $crate::allocator::PinInitOnce::new(|$ptr| $body)
    };
}

#[macro_export]
macro_rules! pin_init_async {
    ($body:expr) => {
        $crate::pin_init!(async move { $body })
    };
}

#[macro_export]
macro_rules! init_box {
    ($alloc:expr, $init:expr) => {
        $crate::allocator::InitBox::new($alloc, $init)
    };
}

