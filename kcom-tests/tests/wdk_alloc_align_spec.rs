#[cfg(all(feature = "wdk-alloc-align", debug_assertions))]
mod wdk_alloc_align_spec {
    use core::alloc::Layout;

    use kcom::allocator::debug_assert_overaligned_layout;

    #[test]
    fn debug_header_accepts_matching_align() {
        let layout = Layout::from_size_align(8, 64).expect("layout");
        debug_assert_overaligned_layout(64, layout);
    }

    #[test]
    fn debug_header_rejects_mismatched_align() {
        let layout = Layout::from_size_align(8, 8).expect("layout");
        let result = std::panic::catch_unwind(|| {
            debug_assert_overaligned_layout(64, layout);
        });
        assert!(result.is_err());
    }
}
