// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0


#[macro_export]
/// Declares a COM interface trait and generates its vtable definition.
///
/// When using `ComRc`, define a raw COM pointer struct (e.g., `IFooRaw` with an
/// `lpVtbl` field) and add `unsafe impl ComInterface for IFooRaw` to satisfy the
/// layout contract.
macro_rules! declare_com_interface {
    (
        $(#[$interface_attr:meta])*
        $vis:vis trait $trait_name:ident: IUnknown {
            const IID: $guid_ty:ty = $guid:expr;
            $($methods:tt)*
        }
    ) => {
        $crate::__kcom_define_interface! {
            @entry
            attrs [$(#[$interface_attr])*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($crate::IUnknown),
            parent_kind (IUnknown),
            parent_vtable (<
                $crate::IUnknownInterface as $crate::vtable::ComInterfaceInfo
            >::Vtable),
            iid ($guid),
            methods { $($methods)* }
        }
    };
    (
        $(#[$interface_attr:meta])*
        $vis:vis trait $trait_name:ident: $parent_trait:ident {
            const IID: $guid_ty:ty = $guid:expr;
            $($methods:tt)*
        }
    ) => {
        $crate::__kcom_define_interface! {
            @entry
            attrs [$(#[$interface_attr])*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind (Other),
            parent_vtable (<
                [<$parent_trait Interface>] as $crate::vtable::ComInterfaceInfo
            >::Vtable),
            iid ($guid),
            methods { $($methods)* }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_define_interface {
    (@entry
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        methods { $($methods:tt)* }
    ) => {
        $crate::__kcom_define_interface!(
            @parse
            attrs [$($attrs)*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind ($parent_kind),
            parent_vtable ($($parent_vtable)+),
            iid ($guid),
            trait_docs [],
            trait_safety [],
            trait_methods [],
            vtable_fields [],
            vtable_inits [],
            vtable_inits_secondary [],
            shim_funcs [],
            ;
            $($methods)*
        );
    };

    (@parse
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        trait_docs [$($trait_docs:tt)*],
        trait_safety [$($trait_safety:tt)*],
        trait_methods [$($trait_methods:tt)*],
        vtable_fields [$($vtable_fields:tt)*],
        vtable_inits [$($vtable_inits:tt)*],
        vtable_inits_secondary [$($vtable_inits_secondary:tt)*],
        shim_funcs [$($shim_funcs:tt)*],
        ;
    ) => {
        $($attrs)*
        $($trait_docs)*
        $vis $($trait_safety)* trait $trait_name: $parent_trait + Sync {
            #[allow(dead_code)]
            const IID: $crate::GUID = $guid;
            $($trait_methods)*
        }

        $crate::paste::paste! {
            #[repr(C)]
            #[derive(Clone, Copy)]
            #[allow(non_snake_case)]
            $vis struct [<$trait_name Vtbl>] {
                pub parent: $($parent_vtable)+,
                $($vtable_fields)*
            }

            impl [<$trait_name Vtbl>] {
                pub const fn new<T>() -> Self
                where
                    T: $trait_name
                        + $crate::ComImpl<[<$trait_name Vtbl>]>
                        + $crate::ComImpl<$($parent_vtable)+>,
                {
                    Self {
                        parent: $crate::__kcom_parent_vtbl!(
                            $parent_kind,
                            $($parent_vtable)+,
                            T,
                            [<$trait_name Vtbl>]
                        ),
                        $($vtable_inits)*
                    }
                }

                $crate::__kcom_vtbl_impl_primary!(
                    $parent_kind,
                    $trait_name,
                    [<$trait_name Vtbl>],
                    $($parent_vtable)+,
                    { $($vtable_inits)* }
                );

                $crate::__kcom_vtbl_impl_secondary!(
                    $parent_kind,
                    $trait_name,
                    [<$trait_name Vtbl>],
                    $($parent_vtable)+,
                    { $($vtable_inits_secondary)* }
                );
            }

            #[repr(C)]
            #[derive(Clone, Copy)]
            #[allow(non_snake_case)]
            $vis struct [<$trait_name Raw>] {
                pub lpVtbl: *mut [<$trait_name Vtbl>],
            }

            unsafe impl $crate::ComInterface for [<$trait_name Raw>] {}

            impl $crate::vtable::ComInterfaceInfo for [<$trait_name Raw>] {
                type Vtable = [<$trait_name Vtbl>];
                const IID: $crate::GUID = $guid;
                const IID_STR: &'static str = stringify!($guid);
            }

            #[allow(dead_code)]
            impl [<$trait_name Raw>] {
                /// Queries for another COM interface and returns a smart pointer on success.
                #[inline]
                pub fn query_interface<U>(&self) -> $crate::StatusResult<$crate::ComRc<U>>
                where
                    U: $crate::smart_ptr::ComInterface + $crate::vtable::ComInterfaceInfo,
                {
                    let mut out = core::ptr::null_mut();
                    let vtbl = unsafe { *(self as *const _ as *mut *mut $crate::IUnknownVtbl) };
                    let status = unsafe {
                        ((*vtbl).QueryInterface)(
                            self as *const _ as *mut core::ffi::c_void,
                            &U::IID,
                            &mut out,
                        )
                    };
                    let status = $crate::Status::from_raw(status);
                    if status.is_error() {
                        return Err(status);
                    }
                    unsafe { $crate::ComRc::<U>::from_raw_or_status(out as *mut U) }
                }

                /// Takes ownership of a raw COM pointer and calls `AddRef` first.
                ///
                /// # Safety
                /// `ptr` must be a valid COM interface pointer.
                #[inline]
                pub unsafe fn from_raw_addref(ptr: *mut Self) -> Option<$crate::ComRc<Self>> {
                    $crate::ComRc::from_raw_addref(ptr)
                }

                /// Takes ownership of a raw COM pointer without calling `AddRef`.
                ///
                /// # Safety
                /// `ptr` must be a valid COM interface pointer.
                #[inline]
                pub unsafe fn from_raw(ptr: *mut Self) -> Option<$crate::ComRc<Self>> {
                    $crate::ComRc::from_raw(ptr)
                }
            }

            $vis struct [<$trait_name Interface>];

            impl $crate::vtable::ComInterfaceInfo for [<$trait_name Interface>] {
                type Vtable = [<$trait_name Vtbl>];
                const IID: $crate::GUID = $guid;
                const IID_STR: &'static str = stringify!($guid);
            }

            unsafe impl $crate::vtable::InterfaceVtable for [<$trait_name Vtbl>] {}

            $($shim_funcs)*
        }
    };

    (@parse
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        trait_docs [$($trait_docs:tt)*],
        trait_safety [$($trait_safety:tt)*],
        trait_methods [$($trait_methods:tt)*],
        vtable_fields [$($vtable_fields:tt)*],
        vtable_inits [$($vtable_inits:tt)*],
        vtable_inits_secondary [$($vtable_inits_secondary:tt)*],
        shim_funcs [$($shim_funcs:tt)*],
        ;
        $(#[$method_attr:meta])* async fn $method_name:ident(&self $(, $arg_name:ident : $arg_ty:ty)*) -> $ret_ty:ty; $($rest:tt)*
    ) => {
        $crate::__kcom_define_interface!(
            @parse
            attrs [$($attrs)*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind ($parent_kind),
            parent_vtable ($($parent_vtable)+),
            iid ($guid),
            trait_docs [
                #[doc = "Async interfaces spawn tasks and return AsyncOperation objects."]
                #[doc = "Futures must be Send + 'static; allocators must be Send + Sync."]
                #[doc = "A non-blocking executor is required to drive completion."]
            ],
            trait_safety [unsafe],
            trait_methods [
                $($trait_methods)*
                $(#[$method_attr])*
                #[cfg(feature = "async-com")]
                $crate::paste::paste! {
                    type [<$method_name:camel Future>]: ::core::future::Future<Output = $ret_ty> + Send + 'static;
                }
                #[cfg(feature = "async-com")]
                type Allocator: $crate::allocator::Allocator + Send + Sync;
                #[cfg(feature = "async-com")]
                $crate::paste::paste! {
                    fn $method_name(&self $(, $arg_name : $arg_ty)*) -> impl $crate::allocator::InitBoxTrait<
                        Self::[<$method_name:camel Future>],
                        Self::Allocator,
                        $crate::NTSTATUS
                    >;
                }
            ],
            vtable_fields [
                $($vtable_fields)*
                #[cfg(feature = "async-com")]
                pub $method_name: unsafe extern "system" fn(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> *mut $crate::async_com::AsyncOperationRaw<$ret_ty>,
            ],
            vtable_inits [
                $($vtable_inits)*
                #[cfg(feature = "async-com")]
                $method_name: [<shim_ $trait_name _ $method_name>]::<T>,
            ],
            vtable_inits_secondary [
                $($vtable_inits_secondary)*
                #[cfg(feature = "async-com")]
                $method_name: [<shim_ $trait_name _ $method_name _secondary>]::<T, P, S, A, INDEX>,
            ],
            shim_funcs [
                $($shim_funcs)*
                #[cfg(not(feature = "async-com"))]
                compile_error!("async-com feature is required to use async methods in declare_com_interface!");
                #[cfg(feature = "async-com")]
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name>]<T: $trait_name>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> *mut $crate::async_com::AsyncOperationRaw<$ret_ty>
                where
                    T: $crate::ComImpl<[<$trait_name Vtbl>]>,
                {
                    if this.is_null() {
                        return core::ptr::null_mut();
                    }
                    let wrapper = unsafe {
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::from_ptr(this)
                    };
                    unsafe {
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::shim_add_ref(this);
                    }
                    let init = wrapper.inner.$method_name($($arg_name),*);
                    let release_fn: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 =
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::shim_release;
                    let guard_ptr = $crate::GuardPtr::new(this);
                    #[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
                    {
                        let op = $crate::async_com::spawn_async_operation_raw_with_init::<$ret_ty, _, _>(
                            init,
                            guard_ptr,
                            release_fn,
                        );
                        match op {
                            Ok(ptr) => ptr,
                            Err(status) => match $crate::async_com::spawn_async_operation_error_raw::<
                                $ret_ty,
                                <T as $trait_name>::[<$method_name:camel Future>],
                            >(status) {
                                Ok(ptr) => ptr,
                                Err(_status) => core::ptr::null_mut(),
                            },
                        }
                    }
                    #[cfg(any(not(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel")), miri))]
                    {
                        let mut future = match init.try_pin() {
                            Ok(future) => future,
                            Err(err) => {
                                unsafe {
                                    $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::shim_release(this);
                                }
                                let status: $crate::NTSTATUS = err.into();
                                return match $crate::async_com::spawn_async_operation_error_raw::<
                                    $ret_ty,
                                    <T as $trait_name>::[<$method_name:camel Future>],
                                >(status) {
                                    Ok(ptr) => ptr,
                                    Err(_status) => core::ptr::null_mut(),
                                };
                            }
                        };
                        let op = $crate::async_com::spawn_async_operation_raw::<$ret_ty, _>(async move {
                            struct ReleaseGuard {
                                ptr: $crate::GuardPtr,
                                release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
                            }
                            impl Drop for ReleaseGuard {
                                fn drop(&mut self) {
                                    unsafe { (self.release)(self.ptr.as_ptr()) };
                                }
                            }
                            let _guard = ReleaseGuard {
                                ptr: guard_ptr,
                                release: release_fn,
                            };
                            future.as_mut().await
                        });
                        match op {
                            Ok(ptr) => ptr,
                            Err(_status) => {
                                unsafe { (release_fn)(this) };
                                core::ptr::null_mut()
                            }
                        }
                    }
                }
                #[allow(non_snake_case)]
                #[allow(dead_code)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name _secondary>]<T, P, S, A, const INDEX: usize>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> *mut $crate::async_com::AsyncOperationRaw<$ret_ty>
                where
                    T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
                    P: $crate::vtable::InterfaceVtable,
                    S: $crate::wrapper::SecondaryVtables,
                    S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, [<$trait_name Vtbl>]>,
                    A: $crate::allocator::Allocator + Send + Sync,
                {
                    if this.is_null() {
                        return core::ptr::null_mut();
                    }
                    let wrapper = unsafe {
                        $crate::wrapper::ComObjectN::<T, P, S, A>::from_secondary_ptr::<[<$trait_name Vtbl>], INDEX>(this)
                    };
                    let primary = wrapper as *const _ as *mut core::ffi::c_void;
                    unsafe {
                        $crate::wrapper::ComObjectN::<T, P, S, A>::shim_add_ref(primary);
                    }
                    let init = wrapper.inner.$method_name($($arg_name),*);
                    let release_fn: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 =
                        $crate::wrapper::ComObjectN::<T, P, S, A>::shim_release;
                    let guard_ptr = $crate::GuardPtr::new(primary);
                    #[cfg(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel", not(miri)))]
                    {
                        let op = $crate::async_com::spawn_async_operation_raw_with_init::<$ret_ty, _, _>(
                            init,
                            guard_ptr,
                            release_fn,
                        );
                        match op {
                            Ok(ptr) => ptr,
                            Err(status) => match $crate::async_com::spawn_async_operation_error_raw::<
                                $ret_ty,
                                <T as $trait_name>::[<$method_name:camel Future>],
                            >(status) {
                                Ok(ptr) => ptr,
                                Err(_status) => core::ptr::null_mut(),
                            },
                        }
                    }
                    #[cfg(any(not(all(feature = "async-com-fused", feature = "driver", feature = "async-com-kernel")), miri))]
                    {
                        let mut future = match init.try_pin() {
                            Ok(future) => future,
                            Err(err) => {
                                unsafe {
                                    $crate::wrapper::ComObjectN::<T, P, S, A>::shim_release(primary);
                                }
                                let status: $crate::NTSTATUS = err.into();
                                return match $crate::async_com::spawn_async_operation_error_raw::<
                                    $ret_ty,
                                    <T as $trait_name>::[<$method_name:camel Future>],
                                >(status) {
                                    Ok(ptr) => ptr,
                                    Err(_status) => core::ptr::null_mut(),
                                };
                            }
                        };
                        let op = $crate::async_com::spawn_async_operation_raw::<$ret_ty, _>(async move {
                            struct ReleaseGuard {
                                ptr: $crate::GuardPtr,
                                release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
                            }
                            impl Drop for ReleaseGuard {
                                fn drop(&mut self) {
                                    unsafe { (self.release)(self.ptr.as_ptr()) };
                                }
                            }
                            let _guard = ReleaseGuard {
                                ptr: guard_ptr,
                                release: release_fn,
                            };
                            future.as_mut().await
                        });
                        match op {
                            Ok(ptr) => ptr,
                            Err(_status) => {
                                unsafe { (release_fn)(primary) };
                                core::ptr::null_mut()
                            }
                        }
                    }
                }
            ],
            ;
            $($rest)*
        );
    };

    (@parse
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        trait_docs [$($trait_docs:tt)*],
        trait_safety [$($trait_safety:tt)*],
        trait_methods [$($trait_methods:tt)*],
        vtable_fields [$($vtable_fields:tt)*],
        vtable_inits [$($vtable_inits:tt)*],
        vtable_inits_secondary [$($vtable_inits_secondary:tt)*],
        shim_funcs [$($shim_funcs:tt)*],
        ;
        $(#[$method_attr:meta])* fn $method_name:ident(&self $(, $arg_name:ident : $arg_ty:ty)*) -> Result<$ok:ty, $err:ty>; $($rest:tt)*
    ) => {
        $crate::__kcom_define_interface!(
            @parse
            attrs [$($attrs)*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind ($parent_kind),
            parent_vtable ($($parent_vtable)+),
            iid ($guid),
            trait_docs [$($trait_docs)*],
            trait_safety [$($trait_safety)*],
            trait_methods [
                $($trait_methods)*
                $(#[$method_attr])* fn $method_name(&self $(, $arg_name : $arg_ty)*) -> Result<$ok, $err>;
            ],
            vtable_fields [
                $($vtable_fields)*
                pub $method_name: unsafe extern "system" fn(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS,
            ],
            vtable_inits [
                $($vtable_inits)*
                $method_name: [<shim_ $trait_name _ $method_name>]::<T>,
            ],
            vtable_inits_secondary [
                $($vtable_inits_secondary)*
                $method_name: [<shim_ $trait_name _ $method_name _secondary>]::<T, P, S, A, INDEX>,
            ],
            shim_funcs [
                $($shim_funcs)*
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name>]<T: $trait_name>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS
                where
                    T: $crate::ComImpl<[<$trait_name Vtbl>]>,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::from_ptr(this)
                    };
                    $crate::iunknown::IntoNtStatus::into_ntstatus(wrapper.inner.$method_name($($arg_name),*))
                }
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name _secondary>]<T, P, S, A, const INDEX: usize>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS
                where
                    T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
                    P: $crate::vtable::InterfaceVtable,
                    S: $crate::wrapper::SecondaryVtables,
                    S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, [<$trait_name Vtbl>]>,
                    A: $crate::allocator::Allocator + Send + Sync,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObjectN::<T, P, S, A>::from_secondary_ptr::<[<$trait_name Vtbl>], INDEX>(this)
                    };
                    $crate::iunknown::IntoNtStatus::into_ntstatus(wrapper.inner.$method_name($($arg_name),*))
                }
            ],
            ;
            $($rest)*
        );
    };

    (@parse
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        trait_docs [$($trait_docs:tt)*],
        trait_safety [$($trait_safety:tt)*],
        trait_methods [$($trait_methods:tt)*],
        vtable_fields [$($vtable_fields:tt)*],
        vtable_inits [$($vtable_inits:tt)*],
        vtable_inits_secondary [$($vtable_inits_secondary:tt)*],
        shim_funcs [$($shim_funcs:tt)*],
        ;
        $(#[$method_attr:meta])* fn $method_name:ident(&self $(, $arg_name:ident : $arg_ty:ty)*) -> ::core::result::Result<$ok:ty, $err:ty>; $($rest:tt)*
    ) => {
        $crate::__kcom_define_interface!(
            @parse
            attrs [$($attrs)*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind ($parent_kind),
            parent_vtable ($($parent_vtable)+),
            iid ($guid),
            trait_docs [$($trait_docs)*],
            trait_safety [$($trait_safety)*],
            trait_methods [
                $($trait_methods)*
                $(#[$method_attr])* fn $method_name(&self $(, $arg_name : $arg_ty)*) -> ::core::result::Result<$ok, $err>;
            ],
            vtable_fields [
                $($vtable_fields)*
                pub $method_name: unsafe extern "system" fn(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS,
            ],
            vtable_inits [
                $($vtable_inits)*
                $method_name: [<shim_ $trait_name _ $method_name>]::<T>,
            ],
            vtable_inits_secondary [
                $($vtable_inits_secondary)*
                $method_name: [<shim_ $trait_name _ $method_name _secondary>]::<T, P, S, A, INDEX>,
            ],
            shim_funcs [
                $($shim_funcs)*
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name>]<T: $trait_name>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS
                where
                    T: $crate::ComImpl<[<$trait_name Vtbl>]>,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::from_ptr(this)
                    };
                    $crate::iunknown::IntoNtStatus::into_ntstatus(wrapper.inner.$method_name($($arg_name),*))
                }
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name _secondary>]<T, P, S, A, const INDEX: usize>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS
                where
                    T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
                    P: $crate::vtable::InterfaceVtable,
                    S: $crate::wrapper::SecondaryVtables,
                    S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, [<$trait_name Vtbl>]>,
                    A: $crate::allocator::Allocator + Send + Sync,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObjectN::<T, P, S, A>::from_secondary_ptr::<[<$trait_name Vtbl>], INDEX>(this)
                    };
                    $crate::iunknown::IntoNtStatus::into_ntstatus(wrapper.inner.$method_name($($arg_name),*))
                }
            ],
            ;
            $($rest)*
        );
    };

    (@parse
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        trait_docs [$($trait_docs:tt)*],
        trait_safety [$($trait_safety:tt)*],
        trait_methods [$($trait_methods:tt)*],
        vtable_fields [$($vtable_fields:tt)*],
        vtable_inits [$($vtable_inits:tt)*],
        vtable_inits_secondary [$($vtable_inits_secondary:tt)*],
        shim_funcs [$($shim_funcs:tt)*],
        ;
        $(#[$method_attr:meta])* fn $method_name:ident(&self $(, $arg_name:ident : $arg_ty:ty)*) -> ::std::result::Result<$ok:ty, $err:ty>; $($rest:tt)*
    ) => {
        $crate::__kcom_define_interface!(
            @parse
            attrs [$($attrs)*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind ($parent_kind),
            parent_vtable ($($parent_vtable)+),
            iid ($guid),
            trait_docs [$($trait_docs)*],
            trait_safety [$($trait_safety)*],
            trait_methods [
                $($trait_methods)*
                $(#[$method_attr])* fn $method_name(&self $(, $arg_name : $arg_ty)*) -> ::std::result::Result<$ok, $err>;
            ],
            vtable_fields [
                $($vtable_fields)*
                pub $method_name: unsafe extern "system" fn(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS,
            ],
            vtable_inits [
                $($vtable_inits)*
                $method_name: [<shim_ $trait_name _ $method_name>]::<T>,
            ],
            vtable_inits_secondary [
                $($vtable_inits_secondary)*
                $method_name: [<shim_ $trait_name _ $method_name _secondary>]::<T, P, S, A, INDEX>,
            ],
            shim_funcs [
                $($shim_funcs)*
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name>]<T: $trait_name>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS
                where
                    T: $crate::ComImpl<[<$trait_name Vtbl>]>,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::from_ptr(this)
                    };
                    $crate::iunknown::IntoNtStatus::into_ntstatus(wrapper.inner.$method_name($($arg_name),*))
                }
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name _secondary>]<T, P, S, A, const INDEX: usize>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $crate::NTSTATUS
                where
                    T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
                    P: $crate::vtable::InterfaceVtable,
                    S: $crate::wrapper::SecondaryVtables,
                    S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, [<$trait_name Vtbl>]>,
                    A: $crate::allocator::Allocator + Send + Sync,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObjectN::<T, P, S, A>::from_secondary_ptr::<[<$trait_name Vtbl>], INDEX>(this)
                    };
                    $crate::iunknown::IntoNtStatus::into_ntstatus(wrapper.inner.$method_name($($arg_name),*))
                }
            ],
            ;
            $($rest)*
        );
    };

    (@parse
        attrs [$($attrs:tt)*],
        vis ($vis:vis),
        trait_name $trait_name:ident,
        parent_trait ($parent_trait:path),
        parent_kind ($parent_kind:ident),
        parent_vtable ($($parent_vtable:tt)+),
        iid ($guid:expr),
        trait_docs [$($trait_docs:tt)*],
        trait_safety [$($trait_safety:tt)*],
        trait_methods [$($trait_methods:tt)*],
        vtable_fields [$($vtable_fields:tt)*],
        vtable_inits [$($vtable_inits:tt)*],
        vtable_inits_secondary [$($vtable_inits_secondary:tt)*],
        shim_funcs [$($shim_funcs:tt)*],
        ;
        $(#[$method_attr:meta])* fn $method_name:ident(&self $(, $arg_name:ident : $arg_ty:ty)*) -> $ret_ty:ty; $($rest:tt)*
    ) => {
        $crate::__kcom_define_interface!(
            @parse
            attrs [$($attrs)*],
            vis ($vis),
            trait_name $trait_name,
            parent_trait ($parent_trait),
            parent_kind ($parent_kind),
            parent_vtable ($($parent_vtable)+),
            iid ($guid),
            trait_docs [$($trait_docs)*],
            trait_safety [$($trait_safety)*],
            trait_methods [
                $($trait_methods)*
                $(#[$method_attr])* fn $method_name(&self $(, $arg_name : $arg_ty)*) -> $ret_ty;
            ],
            vtable_fields [
                $($vtable_fields)*
                pub $method_name: unsafe extern "system" fn(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $ret_ty,
            ],
            vtable_inits [
                $($vtable_inits)*
                $method_name: [<shim_ $trait_name _ $method_name>]::<T>,
            ],
            vtable_inits_secondary [
                $($vtable_inits_secondary)*
                $method_name: [<shim_ $trait_name _ $method_name _secondary>]::<T, P, S, A, INDEX>,
            ],
            shim_funcs [
                $($shim_funcs)*
                #[allow(non_snake_case)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name>]<T: $trait_name>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $ret_ty
                where
                    T: $crate::ComImpl<[<$trait_name Vtbl>]>,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObject::<T, [<$trait_name Vtbl>]>::from_ptr(this)
                    };
                    $crate::__kcom_map_return!($ret_ty, wrapper.inner.$method_name($($arg_name),*))
                }
                #[allow(non_snake_case)]
                #[allow(dead_code)]
                pub unsafe extern "system" fn [<shim_ $trait_name _ $method_name _secondary>]<T, P, S, A, const INDEX: usize>(
                    this: *mut core::ffi::c_void
                    $(, $arg_name: $arg_ty)*
                ) -> $ret_ty
                where
                    T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
                    P: $crate::vtable::InterfaceVtable,
                    S: $crate::wrapper::SecondaryVtables,
                    S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, [<$trait_name Vtbl>]>,
                    A: $crate::allocator::Allocator + Send + Sync,
                {
                    let wrapper = unsafe {
                        $crate::wrapper::ComObjectN::<T, P, S, A>::from_secondary_ptr::<[<$trait_name Vtbl>], INDEX>(this)
                    };
                    $crate::__kcom_map_return!($ret_ty, wrapper.inner.$method_name($($arg_name),*))
                }
            ],
            ;
            $($rest)*
        );
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_parent_vtbl {
    (IUnknown, $parent_vtbl:ty, $ty:ty, $vtbl:ty) => {
        $crate::IUnknownVtbl::new::<$ty, $vtbl>()
    };
    (Other, $parent_vtbl:ty, $ty:ty, $vtbl:ty) => {
        <$parent_vtbl>::new::<$ty>()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_parent_vtbl_primary {
    (
        IUnknown,
        $parent_vtbl:ty,
        $ty:ty,
        $vtbl:ty,
        $primary:ty,
        $secondaries:ty,
        $alloc:ty
    ) => {
        $crate::IUnknownVtbl::new_primary::<$ty, $primary, $secondaries, $alloc>()
    };
    (
        Other,
        $parent_vtbl:ty,
        $ty:ty,
        $vtbl:ty,
        $primary:ty,
        $secondaries:ty,
        $alloc:ty
    ) => {
        <$parent_vtbl>::new_primary::<$ty, $primary, $secondaries, $alloc>()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_parent_vtbl_secondary {
    (
        IUnknown,
        $parent_vtbl:ty,
        $ty:ty,
        $vtbl:ty,
        $primary:ty,
        $secondaries:ty,
        $alloc:ty,
        $index:expr
    ) => {{
        $crate::IUnknownVtbl {
            QueryInterface: $crate::wrapper::ComObjectN::<$ty, $primary, $secondaries, $alloc>::shim_query_interface_secondary::<$vtbl, { $index }>,
            AddRef: $crate::wrapper::ComObjectN::<$ty, $primary, $secondaries, $alloc>::shim_add_ref_secondary::<$vtbl, { $index }>,
            Release: $crate::wrapper::ComObjectN::<$ty, $primary, $secondaries, $alloc>::shim_release_secondary::<$vtbl, { $index }>,
        }
    }};
    (
        Other,
        $parent_vtbl:ty,
        $ty:ty,
        $vtbl:ty,
        $primary:ty,
        $secondaries:ty,
        $alloc:ty,
        $index:expr
    ) => {{
        <$parent_vtbl>::new_secondary::<$ty, $primary, $secondaries, $alloc, { $index }>()
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_vtbl_impl_secondary {
    (
        IUnknown,
        $trait_name:ident,
        $vtbl_name:ty,
        $parent_vtbl:ty,
        { $($vtable_inits_secondary:tt)* }
    ) => {
        #[allow(dead_code)]
        pub const fn new_secondary<T, P, S, A, const INDEX: usize>() -> Self
        where
            T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
            T: $crate::ComImpl<$parent_vtbl>,
            P: $crate::vtable::InterfaceVtable,
            S: $crate::wrapper::SecondaryVtables,
            S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, $vtbl_name>,
            A: $crate::allocator::Allocator + Send + Sync,
        {
            Self {
                parent: $crate::__kcom_parent_vtbl_secondary!(
                    IUnknown,
                    $parent_vtbl,
                    T,
                    $vtbl_name,
                    P,
                    S,
                    A,
                    INDEX
                ),
                $($vtable_inits_secondary)*
            }
        }
    };
    (
        Other,
        $trait_name:ident,
        $vtbl_name:ty,
        $parent_vtbl:ty,
        { $($vtable_inits_secondary:tt)* }
    ) => {
        #[allow(dead_code)]
        pub const fn new_secondary<T, P, S, A, const INDEX: usize>() -> Self
        where
            T: $trait_name + $crate::ComImpl<P> + $crate::wrapper::SecondaryComImpl<S>,
            T: $crate::ComImpl<$parent_vtbl>,
            P: $crate::vtable::InterfaceVtable,
            S: $crate::wrapper::SecondaryVtables,
            S::Entries: $crate::wrapper::SecondaryEntryAccess<INDEX, $vtbl_name>
                + $crate::wrapper::SecondaryEntryAccess<INDEX, $parent_vtbl>,
            A: $crate::allocator::Allocator + Send + Sync,
        {
            Self {
                parent: $crate::__kcom_parent_vtbl_secondary!(
                    Other,
                    $parent_vtbl,
                    T,
                    $vtbl_name,
                    P,
                    S,
                    A,
                    INDEX
                ),
                $($vtable_inits_secondary)*
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_vtbl_impl_primary {
    (
        IUnknown,
        $trait_name:ident,
        $vtbl_name:ty,
        $parent_vtbl:ty,
        { $($vtable_inits:tt)* }
    ) => {
        #[allow(dead_code)]
        pub const fn new_primary<T, P, S, A>() -> Self
        where
            T: $trait_name
                + $crate::ComImpl<$vtbl_name>
                + $crate::ComImpl<$parent_vtbl>
                + $crate::ComImpl<P>
                + $crate::wrapper::SecondaryComImpl<S>,
            P: $crate::vtable::InterfaceVtable,
            S: $crate::wrapper::SecondaryVtables,
            S::Entries: $crate::wrapper::SecondaryList,
            A: $crate::allocator::Allocator + Send + Sync,
        {
            Self {
                parent: $crate::__kcom_parent_vtbl_primary!(
                    IUnknown,
                    $parent_vtbl,
                    T,
                    $vtbl_name,
                    P,
                    S,
                    A
                ),
                $($vtable_inits)*
            }
        }
    };
    (
        Other,
        $trait_name:ident,
        $vtbl_name:ty,
        $parent_vtbl:ty,
        { $($vtable_inits:tt)* }
    ) => {
        #[allow(dead_code)]
        pub const fn new_primary<T, P, S, A>() -> Self
        where
            T: $trait_name
                + $crate::ComImpl<$vtbl_name>
                + $crate::ComImpl<$parent_vtbl>
                + $crate::ComImpl<P>
                + $crate::wrapper::SecondaryComImpl<S>,
            P: $crate::vtable::InterfaceVtable,
            S: $crate::wrapper::SecondaryVtables,
            S::Entries: $crate::wrapper::SecondaryList,
            A: $crate::allocator::Allocator + Send + Sync,
        {
            Self {
                parent: $crate::__kcom_parent_vtbl_primary!(
                    Other,
                    $parent_vtbl,
                    T,
                    $vtbl_name,
                    P,
                    S,
                    A
                ),
                $($vtable_inits)*
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_secondary_entry_bounds {
    (IUnknown, $vtbl:ty, $parent_vtbl:ty, $index:expr) => {
        $crate::wrapper::SecondaryEntryAccess<$index, $vtbl>
    };
    (Other, $vtbl:ty, $parent_vtbl:ty, $index:expr) => {
        $crate::wrapper::SecondaryEntryAccess<$index, $vtbl>
            + $crate::wrapper::SecondaryEntryAccess<$index, $parent_vtbl>
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_vtable_ret {
    (Result<$ok:ty, $err:ty>) => {
        $crate::NTSTATUS
    };
    (::core::result::Result<$ok:ty, $err:ty>) => {
        $crate::NTSTATUS
    };
    (::std::result::Result<$ok:ty, $err:ty>) => {
        $crate::NTSTATUS
    };
    (core::result::Result<$ok:ty, $err:ty>) => {
        $crate::NTSTATUS
    };
    (std::result::Result<$ok:ty, $err:ty>) => {
        $crate::NTSTATUS
    };
    ($ret_ty:ty) => {
        $ret_ty
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_map_return {
    (Result<$ok:ty, $err:ty>, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    (::core::result::Result<$ok:ty, $err:ty>, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    (::std::result::Result<$ok:ty, $err:ty>, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    (core::result::Result<$ok:ty, $err:ty>, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    (std::result::Result<$ok:ty, $err:ty>, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    (NTSTATUS, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    ($crate::NTSTATUS, $expr:expr) => {
        $crate::iunknown::IntoNtStatus::into_ntstatus($expr)
    };
    ($ret_ty:ty, $expr:expr) => {
        $expr
    };
}
