// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0


#[macro_export]
/// Implements `ComImpl::query_interface` for a single primary interface.
///
/// Additional interfaces must provide explicit pointers (tear-offs or aggregated objects).
/// Returning `this` is only valid when the caller will interpret the vtable at offset 0
/// as the requested interface.
macro_rules! impl_query_interface {
    (
        $ty:ty,
        $this:ident,
        $riid:ident,
        [$primary:ident $(=> $primary_ptr:expr)? $(, $iface:ident => $ptr:expr)* $(,)?],
        fallback = $fallback:ty $(,)?
    ) => {
        #[inline]
        fn query_interface(
            &self,
            $this: *mut core::ffi::c_void,
            $riid: &$crate::GUID,
        ) -> Option<*mut core::ffi::c_void> {
            $crate::paste::paste! {
                if *$riid == <[<$primary Interface>] as $crate::vtable::ComInterfaceInfo>::IID {
                    return $crate::impl_query_interface!(@return $this $(, $primary_ptr)?);
                }
                $(
                    if *$riid == <[<$iface Interface>] as $crate::vtable::ComInterfaceInfo>::IID {
                        return $crate::impl_query_interface!(@return $this, $ptr);
                    }
                )*
            }
            <Self as $crate::traits::ComImpl<$fallback>>::query_interface(self, $this, $riid)
        }
    };
    (@return $this:ident, this) => {{
        debug_assert!(!$this.is_null(), "query_interface returned null for primary interface");
        Some($this)
    }};
    (@return $this:ident, $ptr:expr) => {{
        let ptr = $ptr as *mut core::ffi::c_void;
        if ptr.is_null() {
            debug_assert!(!ptr.is_null(), "query_interface returned null pointer");
            None
        } else {
            Some(ptr)
        }
    }};
    (@return $this:ident) => {{
        debug_assert!(!$this.is_null(), "query_interface returned null for primary interface");
        Some($this)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_vtbl_tuple {
    (($($sec:ident),+ $(,)?)) => {
        $crate::paste::paste! { ( $([<$sec Vtbl>],)+ ) }
    };
    (()) => { () };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_qi_match_secondaries_from_wrapper {
    (
        $ty:ty,
        $primary:ident,
        ($($all:ident),+),
        $alloc:ty,
        $wrapper:ident,
        $riid:ident,
        ($head:ident $(, $tail:ident)*),
        $index:expr
    ) => {
        $crate::paste::paste! {
            if *$riid == <[<$head Interface>] as $crate::vtable::ComInterfaceInfo>::IID {
                let ptr = unsafe {
                    $crate::wrapper::ComObjectN::<$ty, [<$primary Vtbl>], $crate::__kcom_vtbl_tuple!(($($all),+)), $alloc>::secondary_ptr::<[<$head Vtbl>], { $index }>($wrapper)
                };
                if ptr.is_null() {
                    return None;
                }
                return Some(ptr);
            }
        }
        $crate::__kcom_qi_match_secondaries_from_wrapper!(
            $ty,
            $primary,
            ($($all),+),
            $alloc,
            $wrapper,
            $riid,
            ($($tail),*),
            ($index + 1)
        );
    };
    (
        $ty:ty,
        $primary:ident,
        ($($all:ident),+),
        $alloc:ty,
        $wrapper:ident,
        $riid:ident,
        (),
        $index:expr
    ) => {};
}

#[macro_export]
/// Implements `ComImpl` and builds the VTABLE for a declared COM interface.
///
/// When `secondaries` is provided, the primary vtable uses `ComObjectN` shims and
/// `QueryInterface` is auto-generated for the primary + all secondaries.
macro_rules! impl_com_interface {
    (
        impl $ty:ty: $trait_name:ident {
            parent = $parent_vtbl:ty,
            secondaries = ($($sec:ident),+ $(,)?),
            allocator = $alloc:ty,
            methods = [$($method:ident),* $(,)?],
            $(fallback = $fallback:ty,)?
        }
    ) => {
        $crate::impl_com_interface!(
            @impl_primary
            $ty,
            $trait_name,
            $parent_vtbl,
            ($($sec),+),
            $alloc,
            [$($method),*],
            $crate::impl_com_interface!(@fallback $parent_vtbl $(, $fallback)?)
        );
    };
    (
        impl $ty:ty: $trait_name:ident {
            parent = $parent_vtbl:ty,
            secondaries = ($($sec:ident),+ $(,)?),
            methods = [$($method:ident),* $(,)?],
            $(fallback = $fallback:ty,)?
        }
    ) => {
        $crate::impl_com_interface!(
            @impl_primary
            $ty,
            $trait_name,
            $parent_vtbl,
            ($($sec),+),
            $crate::allocator::GlobalAllocator,
            [$($method),*],
            $crate::impl_com_interface!(@fallback $parent_vtbl $(, $fallback)?)
        );
    };
    (
        impl $ty:ty: $trait_name:ident {
            parent = $parent_vtbl:ty,
            methods = [$($method:ident),* $(,)?],
            $(fallback = $fallback:ty,)?
        }
    ) => {
        $crate::impl_com_interface!(
            @impl_single
            $ty,
            $trait_name,
            $parent_vtbl,
            [$($method),*],
            $crate::impl_com_interface!(@fallback $parent_vtbl $(, $fallback)?)
        );
    };
    (@fallback $parent_vtbl:ty) => { $parent_vtbl };
    (@fallback $parent_vtbl:ty, $fallback:ty) => { $fallback };
    (@impl_single
        $ty:ty,
        $trait_name:ident,
        $parent_vtbl:ty,
        [$($method:ident),*],
        $fallback:ty
    ) => {
        $crate::paste::paste! {
            impl $crate::ComImpl<[<$trait_name Vtbl>]> for $ty {
                const VTABLE: &'static [<$trait_name Vtbl>] =
                    &[<$trait_name Vtbl>]::new::<Self>();

                #[inline]
                fn query_interface(
                    &self,
                    this: *mut core::ffi::c_void,
                    riid: &$crate::GUID,
                ) -> Option<*mut core::ffi::c_void> {
                    if this.is_null() {
                        return None;
                    }
                    if *riid == <[<$trait_name Interface>] as $crate::vtable::ComInterfaceInfo>::IID {
                        return Some(this);
                    }
                    <Self as $crate::traits::ComImpl<$fallback>>::query_interface(self, this, riid)
                }
            }
        }
    };
    (@impl_primary
        $ty:ty,
        $trait_name:ident,
        $parent_vtbl:ty,
        ($($sec:ident),+),
        $alloc:ty,
        [$($method:ident),*],
        $fallback:ty
    ) => {
        $crate::paste::paste! {
            impl $crate::ComImpl<[<$trait_name Vtbl>]> for $ty {
                const VTABLE: &'static [<$trait_name Vtbl>] =
                    &[<$trait_name Vtbl>]::new_primary::<Self, [<$trait_name Vtbl>], $crate::__kcom_vtbl_tuple!(($($sec),+)), $alloc>();

                #[inline]
                fn query_interface(
                    &self,
                    this: *mut core::ffi::c_void,
                    riid: &$crate::GUID,
                ) -> Option<*mut core::ffi::c_void> {
                    if this.is_null() {
                        return None;
                    }
                    if *riid == <[<$trait_name Interface>] as $crate::vtable::ComInterfaceInfo>::IID {
                        return Some(this);
                    }

                    let wrapper = this as *mut $crate::wrapper::ComObjectN<
                        $ty,
                        [<$trait_name Vtbl>],
                        $crate::__kcom_vtbl_tuple!(($($sec),+)),
                        $alloc,
                    >;

                    $crate::__kcom_qi_match_secondaries_from_wrapper!(
                        $ty,
                        $trait_name,
                        ($($sec),+),
                        $alloc,
                        wrapper,
                        riid,
                        ($($sec),+),
                        0usize
                    );

                    <Self as $crate::traits::ComImpl<$fallback>>::query_interface(self, this, riid)
                }
            }
        }
    };
}

#[macro_export]
/// Implements `ComImpl` and builds the VTABLE for a non-primary interface on `ComObjectN`.
///
/// Provide the primary interface and secondary list so shims can locate the object.
/// `QueryInterface` is auto-generated for the primary + all secondaries.
macro_rules! impl_com_interface_multiple {
    (
        impl $ty:ty: $trait_name:ident {
            parent = $parent_vtbl:ty,
            primary = $primary:ident,
            $(index = $index:expr,)?
            secondaries = ($($sec:ident),+ $(,)?),
            allocator = $alloc:ty,
            methods = [$($method:ident),* $(,)?],
            $(fallback = $fallback:ty,)?
        }
    ) => {
        $crate::impl_com_interface_multiple!(
            @impl
            $ty,
            $trait_name,
            $parent_vtbl,
            $primary,
            $crate::impl_com_interface_multiple!(@index $( $index )?),
            ($($sec),+),
            $alloc,
            [$($method),*],
            $crate::impl_com_interface!(@fallback $parent_vtbl $(, $fallback)?)
        );
    };
    (
        impl $ty:ty: $trait_name:ident {
            parent = $parent_vtbl:ty,
            primary = $primary:ident,
            $(index = $index:expr,)?
            secondaries = ($($sec:ident),+ $(,)?),
            methods = [$($method:ident),* $(,)?],
            $(fallback = $fallback:ty,)?
        }
    ) => {
        $crate::impl_com_interface_multiple!(
            @impl
            $ty,
            $trait_name,
            $parent_vtbl,
            $primary,
            $crate::impl_com_interface_multiple!(@index $( $index )?),
            ($($sec),+),
            $crate::allocator::GlobalAllocator,
            [$($method),*],
            $crate::impl_com_interface!(@fallback $parent_vtbl $(, $fallback)?)
        );
    };
    (@index $index:expr) => { $index };
    (@index) => { 0usize };
    (@impl
        $ty:ty,
        $trait_name:ident,
        $parent_vtbl:ty,
        $primary:ident,
        $index:expr,
        ($($sec:ident),+),
        $alloc:ty,
        [$($method:ident),*],
        $fallback:ty
    ) => {
        $crate::paste::paste! {
            impl $crate::ComImpl<[<$trait_name Vtbl>]> for $ty {
                const VTABLE: &'static [<$trait_name Vtbl>] =
                    &[<$trait_name Vtbl>]::new_secondary::<Self, [<$primary Vtbl>], $crate::__kcom_vtbl_tuple!(($($sec),+)), $alloc, { $index }>();

                #[inline]
                fn query_interface(
                    &self,
                    this: *mut core::ffi::c_void,
                    riid: &$crate::GUID,
                ) -> Option<*mut core::ffi::c_void> {
                    if this.is_null() {
                        return None;
                    }
                    let wrapper = this as *mut $crate::wrapper::ComObjectN<
                        $ty,
                        [<$primary Vtbl>],
                        $crate::__kcom_vtbl_tuple!(($($sec),+)),
                        $alloc,
                    >;
                    let primary_ptr = this;

                    if *riid == <[<$primary Interface>] as $crate::vtable::ComInterfaceInfo>::IID {
                        return Some(primary_ptr);
                    }

                    $crate::__kcom_qi_match_secondaries_from_wrapper!(
                        $ty,
                        $primary,
                        ($($sec),+),
                        $alloc,
                        wrapper,
                        riid,
                        ($($sec),+),
                        0usize
                    );

                    <Self as $crate::traits::ComImpl<$fallback>>::query_interface(self, this, riid)
                }
            }
        }
    };
}
