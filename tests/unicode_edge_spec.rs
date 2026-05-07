#[cfg(feature = "kernel-unicode")]
mod unicode_edge_spec {
    use kcom::{
        unicode_string_as_slice, LocalUnicodeString, UnicodeStringError,
    };
    use kcom::ntddk::UNICODE_STRING;

    #[test]
    fn local_unicode_string_overflow_is_rejected() {
        let mut local = LocalUnicodeString::<4>::new();
        assert!(local.try_push_str("abc").is_ok());
        let err = local.try_push_str("d").unwrap_err();
        assert_eq!(err, UnicodeStringError::TooLong);
    }

    #[test]
    fn local_unicode_string_clear_resets_length() {
        let mut local = LocalUnicodeString::<8>::from_str("hi").expect("init");
        assert_eq!(local.len(), 2);
        local.clear();
        assert_eq!(local.len(), 0);
        assert!(local.is_empty());

        let unicode = local.as_unicode_ref();
        assert_eq!(unicode.as_ref().Length, 0);
    }

    #[test]
    fn unicode_string_as_slice_handles_null_buffer() {
        let unicode = UNICODE_STRING {
            Length: 0,
            MaximumLength: 0,
            Buffer: core::ptr::null_mut(),
        };
        let slice = unsafe { unicode_string_as_slice(&unicode) };
        assert!(slice.is_empty());
    }
}
