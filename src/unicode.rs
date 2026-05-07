// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use alloc::string::String;
use core::alloc::Layout;
use core::fmt;
use core::mem::ManuallyDrop;
use core::slice;

use crate::allocator::{Allocator, GlobalAllocator};
use crate::ntddk::UNICODE_STRING;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnicodeStringError {
    TooLong,
    AllocationFailed,
    InvalidUtf16,
}

impl fmt::Display for UnicodeStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => f.write_str("UNICODE_STRING exceeds u16 length"),
            Self::AllocationFailed => f.write_str("UNICODE_STRING allocation failed"),
            Self::InvalidUtf16 => f.write_str("UNICODE_STRING contains invalid UTF-16"),
        }
    }
}

#[macro_export]
/// Returns a reference to a compile-time `UNICODE_STRING` built from a string literal.
///
/// The backing buffer is UTF-16 with a trailing NUL. The returned `UNICODE_STRING`
/// references static storage and is safe to share across threads.
///
/// Requires the `kernel-unicode` feature.
macro_rules! kstr {
    ($lit:literal) => {{
        const BUF: &[u16] = &$crate::utf16_lit::utf16_null!($lit);
        static UNICODE: $crate::ntddk::UNICODE_STRING = $crate::ntddk::UNICODE_STRING {
            Length: ((BUF.len() - 1) * 2) as u16,
            MaximumLength: (BUF.len() * 2) as u16,
            Buffer: BUF.as_ptr() as *mut u16,
        };
        &UNICODE
    }};
}

/// An owned UNICODE_STRING backed by a UTF-16 buffer.
///
/// The buffer is constructed once, boxed, and never resized.
/// Mutable accessors are intentionally omitted to keep the backing storage
/// stable for the lifetime of the UNICODE_STRING.
///
/// The returned `UNICODE_STRING` references memory owned by this type. Callers
/// must not free it (e.g., via `RtlFreeUnicodeString`) or store it beyond the
/// lifetime of the `OwnedUnicodeString` instance.
pub struct OwnedUnicodeString<A: Allocator + Send + Sync = GlobalAllocator> {
    inner: UNICODE_STRING,
    buffer: *mut u16,
    buffer_len: usize,
    alloc: ManuallyDrop<A>,
}

/// A stack-backed UNICODE_STRING with a fixed UTF-16 buffer.
///
/// The buffer stores a trailing NUL. The returned UNICODE_STRING value is
/// constructed on demand and borrows the internal buffer.
pub struct LocalUnicodeString<const N: usize> {
    buffer: [u16; N],
    len: usize,
}

/// A borrowed UNICODE_STRING view tied to a backing UTF-16 buffer lifetime.
pub struct UnicodeStringRef<'a> {
    inner: UNICODE_STRING,
    _marker: core::marker::PhantomData<&'a [u16]>,
}

impl<'a> UnicodeStringRef<'a> {
    #[inline]
    pub fn as_ref(&self) -> &UNICODE_STRING {
        &self.inner
    }

    #[inline]
    pub fn as_ptr(&self) -> *const UNICODE_STRING {
        &self.inner
    }
}

impl<A: Allocator + Send + Sync> OwnedUnicodeString<A> {
    pub fn new_in(value: &str, alloc: A) -> Result<Self, UnicodeStringError> {
        let len = value.encode_utf16().count();
        let max_units = (u16::MAX as usize) / 2;
        if len + 1 > max_units {
            return Err(UnicodeStringError::TooLong);
        }
        let layout = Layout::array::<u16>(len + 1).map_err(|_| UnicodeStringError::TooLong)?;
        let buffer = unsafe { alloc.alloc(layout) } as *mut u16;
        if buffer.is_null() {
            return Err(UnicodeStringError::AllocationFailed);
        }
        for (idx, unit) in value.encode_utf16().enumerate() {
            unsafe {
                buffer.add(idx).write(unit);
            }
        }
        unsafe {
            buffer.add(len).write(0);
        }
        let inner = UNICODE_STRING {
            Length: (len * 2) as u16,
            MaximumLength: ((len + 1) * 2) as u16,
            Buffer: buffer,
        };
        Ok(Self {
            inner,
            buffer,
            buffer_len: len + 1,
            alloc: ManuallyDrop::new(alloc),
        })
    }

    /// Returns a borrowed UNICODE_STRING view.
    ///
    /// The underlying buffer remains owned by this instance and must not be freed
    /// by the caller.
    #[inline]
    pub fn as_unicode(&self) -> &UNICODE_STRING {
        &self.inner
    }

    #[inline]
    pub fn as_ptr(&self) -> *const UNICODE_STRING {
        &self.inner
    }

    #[inline]
    pub fn as_utf16(&self) -> &[u16] {
        let len = self.inner.Length as usize / 2;
        unsafe { slice::from_raw_parts(self.buffer, len) }
    }
}

impl<const N: usize> LocalUnicodeString<N> {
    #[inline]
    pub fn new() -> Self {
        Self {
            buffer: [0; N],
            len: 0,
        }
    }

    pub fn from_str(value: &str) -> Result<Self, UnicodeStringError> {
        let mut out = Self::new();
        out.try_push_str(value)?;
        Ok(out)
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
        if !self.buffer.is_empty() {
            self.buffer[0] = 0;
        }
    }

    pub fn try_push_str(&mut self, value: &str) -> Result<(), UnicodeStringError> {
        let max_units = (u16::MAX as usize) / 2;
        let cap_units = core::cmp::min(N, max_units);
        let value_len = value.encode_utf16().count();

        if self.len + value_len + 1 > cap_units {
            return Err(UnicodeStringError::TooLong);
        }

        for (idx, unit) in value.encode_utf16().enumerate() {
            self.buffer[self.len + idx] = unit;
        }
        self.len += value_len;
        if self.len < N {
            self.buffer[self.len] = 0;
        }
        Ok(())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns a UNICODE_STRING view tied to this value's lifetime.
    #[inline]
    pub fn as_unicode_ref(&self) -> UnicodeStringRef<'_> {
        let max_units = (u16::MAX as usize) / 2;
        let cap_units = core::cmp::min(N, max_units);
        UnicodeStringRef {
            inner: UNICODE_STRING {
                Length: (self.len * 2) as u16,
                MaximumLength: (cap_units * 2) as u16,
                Buffer: self.buffer.as_ptr() as *mut u16,
            },
            _marker: core::marker::PhantomData,
        }
    }

    #[inline]
    pub fn as_utf16(&self) -> &[u16] {
        &self.buffer[..self.len]
    }
}

impl<const N: usize> fmt::Write for LocalUnicodeString<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.try_push_str(s).map_err(|_| fmt::Error)
    }
}

impl OwnedUnicodeString<GlobalAllocator> {
    pub fn new(value: &str) -> Result<Self, UnicodeStringError> {
        Self::new_in(value, GlobalAllocator)
    }
}

impl<A: Allocator + Send + Sync> Drop for OwnedUnicodeString<A> {
    fn drop(&mut self) {
        if self.buffer.is_null() {
            return;
        }
        let layout = match Layout::array::<u16>(self.buffer_len) {
            Ok(layout) => layout,
            Err(_) => return,
        };
        let alloc = unsafe { core::ptr::read(&self.alloc) };
        let alloc = ManuallyDrop::into_inner(alloc);
        unsafe {
            alloc.dealloc(self.buffer as *mut u8, layout);
        }
        drop(alloc);
    }
}

/// Returns the UTF-16 slice referenced by a UNICODE_STRING.
///
/// # Safety
/// Caller must ensure the UNICODE_STRING buffer is valid for reads.
pub unsafe fn unicode_string_as_slice(unicode: &UNICODE_STRING) -> &[u16] {
    if unicode.Buffer.is_null() || unicode.Length == 0 {
        return &[];
    }
    let len = unicode.Length as usize / 2;
    unsafe { slice::from_raw_parts(unicode.Buffer, len) }
}

/// Converts a UNICODE_STRING into an owned Rust String without panicking on OOM.
///
/// # Safety
/// Caller must ensure the UNICODE_STRING buffer is valid for reads.
pub unsafe fn unicode_string_to_string(
    unicode: &UNICODE_STRING,
) -> Result<String, UnicodeStringError> {
    let slice = unsafe { unicode_string_as_slice(unicode) };
    let reserve = slice
        .len()
        .checked_mul(3)
        .ok_or(UnicodeStringError::TooLong)?;
    let mut out = String::new();
    out.try_reserve(reserve)
        .map_err(|_| UnicodeStringError::AllocationFailed)?;
    for unit in core::char::decode_utf16(slice.iter().copied()) {
        match unit {
            Ok(ch) => out.push(ch),
            Err(_) => return Err(UnicodeStringError::InvalidUtf16),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn unicode_string_allows_max_len_minus_one() {
        let value: String = std::iter::repeat('a').take(32_766).collect();
        let owned = OwnedUnicodeString::new(&value).expect("expected string to fit");
        assert_eq!(owned.as_unicode().Length, (32_766 * 2) as u16);
        assert_eq!(owned.as_unicode().MaximumLength, ((32_766 + 1) * 2) as u16);
    }

    #[test]
    fn unicode_string_rejects_overflow() {
        let value: String = std::iter::repeat('a').take(32_767).collect();
        assert!(matches!(
            OwnedUnicodeString::new(&value),
            Err(UnicodeStringError::TooLong)
        ));
    }

    #[test]
    fn kstr_macro_builds_unicode_string() {
        let unicode = kstr!("Test");
        assert_eq!(unicode.Length, 8);
        assert_eq!(unicode.MaximumLength, 10);

        let expected: Vec<u16> = "Test".encode_utf16().collect();
        let slice = unsafe { unicode_string_as_slice(unicode) };
        assert_eq!(slice, expected.as_slice());

        let len = (unicode.Length / 2) as usize;
        unsafe {
            assert_eq!(*unicode.Buffer.add(len), 0);
        }
    }

    #[test]
    fn local_unicode_string_from_str() {
        let local = LocalUnicodeString::<8>::from_str("Test").expect("local string");
        let unicode = local.as_unicode_ref();
        assert_eq!(unicode.as_ref().Length, 8);
        assert_eq!(unicode.as_ref().MaximumLength, 16);
        let slice = local.as_utf16();
        assert_eq!(slice, "Test".encode_utf16().collect::<Vec<u16>>());
    }

    #[test]
    fn local_unicode_string_fmt_write() {
        let mut local = LocalUnicodeString::<16>::new();
        use core::fmt::Write;
        write!(&mut local, "Err: {:x}", 0x2a).expect("write");
        let text = String::from_utf16(local.as_utf16()).expect("utf16");
        assert_eq!(text, "Err: 2a");
    }
}
