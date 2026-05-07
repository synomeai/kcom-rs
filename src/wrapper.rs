// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

use core::alloc::Layout;
use core::ffi::c_void;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::allocator::{Allocator, GlobalAllocator};
use crate::iunknown::{
    GUID, IUnknownVtbl, IID_IUNKNOWN, NTSTATUS, STATUS_INSUFFICIENT_RESOURCES, STATUS_NOINTERFACE,
    STATUS_SUCCESS,
};
use crate::smart_ptr::{ComInterface, ComRc};
use crate::traits::ComImpl;
use crate::vtable::{ComInterfaceInfo, InterfaceVtable};
use crate::refcount;

#[cold]
#[inline(never)]
fn resurrection_violation() -> ! {
    #[cfg(debug_assertions)]
    crate::trace::report_error(file!(), line!(), crate::iunknown::STATUS_UNSUCCESSFUL);

    #[cfg(all(
        feature = "driver",
        any(feature = "async-com-kernel", feature = "kernel-unicode"),
        not(miri)
    ))]
    unsafe {
        crate::ntddk::KeBugCheckEx(0x4B43_4F4D, 0x52455355, 0, 0, 0);
    }

    #[cfg(all(not(feature = "driver"), test))]
    {
        std::process::abort();
    }

    #[cfg(all(not(feature = "driver"), not(test)))]
    {
        loop {
            core::hint::spin_loop();
        }
    }

    #[cfg(all(feature = "driver", not(any(feature = "async-com-kernel", feature = "kernel-unicode"))))]
    {
        loop {
            core::hint::spin_loop();
        }
    }

    #[cfg(all(
        feature = "driver",
        any(feature = "async-com-kernel", feature = "kernel-unicode"),
        miri
    ))]
    {
        loop {
            core::hint::spin_loop();
        }
    }
}

#[cold]
#[inline(never)]
fn fail_fast() -> ! {
    #[cfg(all(
        feature = "driver",
        any(feature = "async-com-kernel", feature = "kernel-unicode"),
        not(miri)
    ))]
    unsafe {
        crate::ntddk::KeBugCheckEx(0xDEAD_DEAD, 0, 0, 0, 0);
    }

    #[cfg(all(not(feature = "driver"), test))]
    {
        std::process::abort();
    }

    #[cfg(all(not(feature = "driver"), not(test)))]
    {
        panic!("kcom: panic crossed FFI boundary");
    }

    #[cfg(all(
        feature = "driver",
        not(any(feature = "async-com-kernel", feature = "kernel-unicode"))
    ))]
    {
        loop {
            core::hint::spin_loop();
        }
    }

    #[cfg(all(
        feature = "driver",
        any(feature = "async-com-kernel", feature = "kernel-unicode"),
        miri
    ))]
    {
        loop {
            core::hint::spin_loop();
        }
    }
}

#[must_use]
pub(crate) struct PanicGuard;

impl PanicGuard {
    #[inline(always)]
    pub fn new() -> Self {
        Self
    }
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        fail_fast();
    }
}

#[inline]
unsafe fn delegating_add_ref(
    outer_unknown: Option<*mut c_void>,
    ref_count: &AtomicU32,
) -> u32 {
    if let Some(outer) = outer_unknown {
        if !outer.is_null() {
            let vtbl = unsafe { *(outer as *mut *mut IUnknownVtbl) };
            return unsafe { ((*vtbl).AddRef)(outer) };
        }
    }
    refcount::add(ref_count)
}

#[inline]
unsafe fn delegating_release<F>(
    outer_unknown: Option<*mut c_void>,
    ref_count: &AtomicU32,
    release_inner: F,
) -> u32
where
    F: FnOnce(),
{
    if let Some(outer) = outer_unknown {
        if !outer.is_null() {
            let vtbl = unsafe { *(outer as *mut *mut IUnknownVtbl) };
            return unsafe { ((*vtbl).Release)(outer) };
        }
    }

    let count = refcount::sub(ref_count);
    if count == 0 {
        core::sync::atomic::fence(Ordering::Acquire);
        release_inner();
    }

    count
}

#[repr(C)]
struct NonDelegatingIUnknown<T, I, A>
where
    T: ComImpl<I>,
    I: InterfaceVtable,
    A: Allocator + Send + Sync,
{
    vtable: &'static IUnknownVtbl,
    parent: *mut ComObject<T, I, A>,
}

impl<T, P, S> ComObjectN<T, P, S, GlobalAllocator>
where
    T: ComImpl<P> + SecondaryComImpl<S>,
    P: InterfaceVtable,
    S: SecondaryVtables,
    S::Entries: SecondaryList,
{
    #[inline]
    pub fn new(inner: T) -> Result<*mut c_void, NTSTATUS> {
        Self::new_in(inner, GlobalAllocator)
    }

    #[inline]
    pub fn try_new(inner: T) -> Option<*mut c_void> {
        Self::try_new_in(inner, GlobalAllocator)
    }

    /// Creates a COM object and returns a smart pointer that owns the initial reference.
    #[inline]
    pub fn new_rc<R>(inner: T) -> Result<ComRc<R>, NTSTATUS>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = P>,
    {
        Self::new_rc_in(inner, GlobalAllocator)
    }

    /// Creates a COM object and returns a smart pointer that owns the initial reference.
    #[inline]
    pub fn try_new_rc<R>(inner: T) -> Option<ComRc<R>>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = P>,
    {
        Self::try_new_rc_in(inner, GlobalAllocator)
    }
}

#[repr(C)]
pub struct InterfaceEntryN<I>
where
    I: InterfaceVtable,
{
    vtable: &'static I,
    parent: *mut c_void,
}

pub trait SecondaryList {
    fn init(&mut self, parent: *mut c_void);
}

pub trait SecondaryEntryAccess<const INDEX: usize, I>
where
    I: InterfaceVtable,
{
    fn entry(&mut self) -> *mut InterfaceEntryN<I>;

    #[inline]
    unsafe fn parent_from_ptr(ptr: *mut c_void) -> *mut c_void {
        unsafe { (*(ptr as *mut InterfaceEntryN<I>)).parent }
    }
}

pub trait SecondaryComImpl<S>
where
    S: SecondaryVtables,
{
    fn secondary_entries() -> S::Entries;
}

pub trait SecondaryVtables
where
    Self: Sized,
{
    type Entries: SecondaryList;

    fn entries<T>() -> Self::Entries
    where
        T: SecondaryComImpl<Self>,
    {
        T::secondary_entries()
    }
}

impl SecondaryList for () {
    #[inline]
    fn init(&mut self, _parent: *mut c_void) {}
}

macro_rules! impl_secondary_entry_access {
    ($name:ident, $index:tt, $($all:ident),+) => {
        impl<$($all),+> SecondaryEntryAccess<$index, $name> for ($(InterfaceEntryN<$all>,)+)
        where
            $($all: InterfaceVtable,)+
        {
            #[inline]
            fn entry(&mut self) -> *mut InterfaceEntryN<$name> {
                &mut self.$index
            }
        }
    };
}

macro_rules! impl_secondary_entry_access_all {
    (($name:ident : $index:tt, $($rest:ident : $rest_index:tt),+); ($($all:ident),+)) => {
        impl_secondary_entry_access!($name, $index, $($all),+);
        impl_secondary_entry_access_all!(($($rest : $rest_index),+); ($($all),+));
    };
    (($name:ident : $index:tt); ($($all:ident),+)) => {
        impl_secondary_entry_access!($name, $index, $($all),+);
    };
}

macro_rules! impl_secondary_tuple {
    ($(($len:tt, $($name:ident : $index:tt),+)),+ $(,)?) => {
        $(
            impl<$($name),+> SecondaryVtables for ($($name,)+)
            where
                $($name: InterfaceVtable,)+
            {
                type Entries = ($(InterfaceEntryN<$name>,)+);
            }

            impl<$($name),+> SecondaryList for ($(InterfaceEntryN<$name>,)+)
            where
                $($name: InterfaceVtable,)+
            {
                #[inline]
                fn init(&mut self, parent: *mut c_void) {
                    $(self.$index.parent = parent;)+
                }
            }

            impl<T, $($name),+> SecondaryComImpl<($($name,)+)> for T
            where
                $($name: InterfaceVtable,)+
                $(T: ComImpl<$name>,)+
            {
                #[inline]
                fn secondary_entries() -> <($($name,)+) as SecondaryVtables>::Entries {
                    ($(
                        InterfaceEntryN {
                            vtable: <T as ComImpl<$name>>::VTABLE,
                            parent: core::ptr::null_mut(),
                        },
                    )+)
                }
            }

            impl_secondary_entry_access_all!(($($name : $index),+); ($($name),+));
        )+
    };
}

impl_secondary_tuple!((1, S1: 0));
impl_secondary_tuple!((2, S1: 0, S2: 1));
impl_secondary_tuple!((3, S1: 0, S2: 1, S3: 2));
impl_secondary_tuple!((4, S1: 0, S2: 1, S3: 2, S4: 3));
impl_secondary_tuple!((5, S1: 0, S2: 1, S3: 2, S4: 3, S5: 4));
impl_secondary_tuple!((6, S1: 0, S2: 1, S3: 2, S4: 3, S5: 4, S6: 5));
impl_secondary_tuple!((7, S1: 0, S2: 1, S3: 2, S4: 3, S5: 4, S6: 5, S7: 6));
impl_secondary_tuple!((8, S1: 0, S2: 1, S3: 2, S4: 3, S5: 4, S6: 5, S7: 6, S8: 7));

#[repr(C)]
struct NonDelegatingIUnknownN<T, P, S, A>
where
    T: ComImpl<P> + SecondaryComImpl<S>,
    P: InterfaceVtable,
    S: SecondaryVtables,
    A: Allocator + Send + Sync,
{
    vtable: &'static IUnknownVtbl,
    parent: *mut ComObjectN<T, P, S, A>,
}

#[repr(C)]
pub struct ComObjectN<T, P, S, A = GlobalAllocator>
where
    T: ComImpl<P> + SecondaryComImpl<S>,
    P: InterfaceVtable,
    S: SecondaryVtables,
    A: Allocator + Send + Sync,
{
    vtable: &'static P,
    secondaries: S::Entries,
    non_delegating_unknown: NonDelegatingIUnknownN<T, P, S, A>,
    ref_count: AtomicU32,
    outer_unknown: Option<*mut c_void>,
    pub inner: T,
    alloc: ManuallyDrop<A>,
}

impl<T, P, S, A> ComObjectN<T, P, S, A>
where
    T: ComImpl<P> + SecondaryComImpl<S>,
    P: InterfaceVtable,
    S: SecondaryVtables,
    S::Entries: SecondaryList,
    A: Allocator + Send + Sync,
{
    const LAYOUT: Layout = Layout::new::<Self>();
    const NON_DELEGATING_VTABLE: IUnknownVtbl = IUnknownVtbl {
        QueryInterface: Self::shim_non_delegating_query_interface,
        AddRef: Self::shim_non_delegating_add_ref,
        Release: Self::shim_non_delegating_release,
    };

    #[inline]
    fn init_non_delegating_ptr(ptr: *mut Self) {
        unsafe {
            (*ptr).non_delegating_unknown.parent = ptr;
        }
    }

    #[inline]
    fn init_secondary_ptr(ptr: *mut Self) {
        unsafe {
            (*ptr).secondaries.init(ptr as *mut c_void);
        }
    }

    #[inline]
    pub unsafe fn secondary_ptr<I, const INDEX: usize>(ptr: *mut Self) -> *mut c_void
    where
        I: InterfaceVtable,
        S::Entries: SecondaryEntryAccess<INDEX, I>,
    {
        unsafe {
            <S::Entries as SecondaryEntryAccess<INDEX, I>>::entry(&mut (*ptr).secondaries)
                as *mut _
                as *mut c_void
        }
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to a `ComObjectN<T, P, S>` allocated by this crate.
    /// The returned reference is only valid while `ptr` remains a valid allocation and must
    /// not be stored beyond that lifetime.
    pub unsafe fn from_ptr<'a>(ptr: *mut c_void) -> &'a Self {
        unsafe { &*(ptr as *const Self) }
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to the secondary interface entry for this object.
    /// The returned reference is only valid while the underlying COM object allocation
    /// remains alive.
    pub unsafe fn from_secondary_ptr<'a, I, const INDEX: usize>(ptr: *mut c_void) -> &'a Self
    where
        I: InterfaceVtable,
        S::Entries: SecondaryEntryAccess<INDEX, I>,
    {
        let parent = unsafe { <S::Entries as SecondaryEntryAccess<INDEX, I>>::parent_from_ptr(ptr) };
        unsafe { &*(parent as *const Self) }
    }

    #[inline]
    pub fn new_rc_in<R>(inner: T, alloc: A) -> Result<ComRc<R>, NTSTATUS>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = P>,
    {
        Self::try_new_rc_in(inner, alloc).ok_or(STATUS_INSUFFICIENT_RESOURCES)
    }

    #[inline]
    pub fn try_new_rc_in<R>(inner: T, alloc: A) -> Option<ComRc<R>>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = P>,
    {
        let ptr = Self::try_new_in(inner, alloc)?;
        Some(unsafe { ComRc::from_raw_unchecked(ptr as *mut R) })
    }

    #[inline]
    pub fn new_in(inner: T, alloc: A) -> Result<*mut c_void, NTSTATUS> {
        Self::try_new_in(inner, alloc).ok_or(STATUS_INSUFFICIENT_RESOURCES)
    }

    #[inline]
    pub fn try_new_in(inner: T, alloc: A) -> Option<*mut c_void> {
        let ptr = unsafe { alloc.alloc(Self::LAYOUT) } as *mut Self;
        if ptr.is_null() {
            return None;
        }
        unsafe {
            ptr.write(Self {
                vtable: <T as ComImpl<P>>::VTABLE,
                secondaries: S::entries::<T>(),
                non_delegating_unknown: NonDelegatingIUnknownN {
                    vtable: &Self::NON_DELEGATING_VTABLE,
                    parent: core::ptr::null_mut(),
                },
                ref_count: AtomicU32::new(1),
                outer_unknown: None,
                inner,
                alloc: ManuallyDrop::new(alloc),
            });
            Self::init_non_delegating_ptr(ptr);
            Self::init_secondary_ptr(ptr);
            Some(ptr as *mut c_void)
        }
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_add_ref(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_ptr(this) };
        let result = delegating_add_ref(wrapper.outer_unknown, &wrapper.ref_count);
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_release(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let wrapper = unsafe { &*(this as *const Self) };
        let result = delegating_release(wrapper.outer_unknown, &wrapper.ref_count, || {
            let ptr = this as *mut Self;
            let alloc = core::ptr::read(&(*ptr).alloc);
            let alloc = ManuallyDrop::into_inner(alloc);
            core::ptr::drop_in_place(&mut (*ptr).inner);
            let resurrected = (*ptr).ref_count.load(Ordering::Acquire);
            if resurrected != 0 {
                resurrection_violation();
            }
            alloc.dealloc(ptr as *mut u8, Self::LAYOUT);
            drop(alloc);
        });
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObjectN` for `T`.
    /// `riid` and `ppv` must be valid, non-null pointers.
    pub unsafe extern "system" fn shim_query_interface(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> NTSTATUS {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_ptr(this) };
        if let Some(outer) = wrapper.outer_unknown {
            if !outer.is_null() {
                let vtbl = unsafe { *(outer as *mut *mut IUnknownVtbl) };
                let result = unsafe { ((*vtbl).QueryInterface)(outer, riid, ppv) };
                core::mem::forget(guard);
                return result;
            }
        }

        if ppv.is_null() || riid.is_null() {
            core::mem::forget(guard);
            return STATUS_NOINTERFACE;
        }

        let riid = unsafe { &*riid };

        if *riid == IID_IUNKNOWN {
            unsafe { Self::shim_add_ref(this) };
            unsafe { *ppv = this };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        if let Some(ptr) = <T as ComImpl<P>>::query_interface(&wrapper.inner, this, riid) {
            let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
            unsafe { ((*vtbl).AddRef)(ptr) };
            unsafe { *ppv = ptr };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        unsafe { *ppv = core::ptr::null_mut() };
        core::mem::forget(guard);
        STATUS_NOINTERFACE
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_add_ref_secondary<I, const INDEX: usize>(
        this: *mut c_void,
    ) -> u32
    where
        I: InterfaceVtable,
        S::Entries: SecondaryEntryAccess<INDEX, I>,
    {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_secondary_ptr::<I, INDEX>(this) };
        let result = delegating_add_ref(wrapper.outer_unknown, &wrapper.ref_count);
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_release_secondary<I, const INDEX: usize>(
        this: *mut c_void,
    ) -> u32
    where
        I: InterfaceVtable,
        S::Entries: SecondaryEntryAccess<INDEX, I>,
    {
        let guard = PanicGuard::new();
        let primary =
            unsafe { <S::Entries as SecondaryEntryAccess<INDEX, I>>::parent_from_ptr(this) };
        let result = unsafe { Self::shim_release(primary) };
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_query_interface_secondary<I, const INDEX: usize>(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> NTSTATUS
    where
        I: InterfaceVtable,
        S::Entries: SecondaryEntryAccess<INDEX, I>,
    {
        let guard = PanicGuard::new();
        let primary =
            unsafe { <S::Entries as SecondaryEntryAccess<INDEX, I>>::parent_from_ptr(this) };
        let wrapper = unsafe { Self::from_ptr(primary) };
        if let Some(outer) = wrapper.outer_unknown {
            if !outer.is_null() {
                let vtbl = unsafe { *(outer as *mut *mut IUnknownVtbl) };
                let result = unsafe { ((*vtbl).QueryInterface)(outer, riid, ppv) };
                core::mem::forget(guard);
                return result;
            }
        }

        if ppv.is_null() || riid.is_null() {
            core::mem::forget(guard);
            return STATUS_NOINTERFACE;
        }

        let riid = unsafe { &*riid };

        if *riid == IID_IUNKNOWN {
            unsafe { Self::shim_add_ref(primary) };
            unsafe { *ppv = primary };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        if let Some(ptr) = <T as ComImpl<P>>::query_interface(&wrapper.inner, primary, riid) {
            let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
            unsafe { ((*vtbl).AddRef)(ptr) };
            unsafe { *ppv = ptr };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        unsafe { *ppv = core::ptr::null_mut() };
        core::mem::forget(guard);
        STATUS_NOINTERFACE
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid non-delegating IUnknown pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_non_delegating_add_ref(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_non_delegating(this) };
        let result = refcount::add(&wrapper.ref_count);
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid non-delegating IUnknown pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_non_delegating_release(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let ptr = unsafe { Self::non_delegating_parent_ptr(this) };
        let count = refcount::sub(unsafe { &(*ptr).ref_count });

        if count == 0 {
            core::sync::atomic::fence(Ordering::Acquire);
            let alloc = unsafe { core::ptr::read(core::ptr::addr_of!((*ptr).alloc)) };
            let alloc = ManuallyDrop::into_inner(alloc);
            unsafe {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*ptr).inner));
                let resurrected = (*ptr).ref_count.load(Ordering::Acquire);
                if resurrected != 0 {
                    resurrection_violation();
                }
                alloc.dealloc(ptr as *mut u8, Self::LAYOUT);
            }
            drop(alloc);
        }

        core::mem::forget(guard);
        count
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid non-delegating IUnknown pointer created by `ComObjectN` for `T`.
    pub unsafe extern "system" fn shim_non_delegating_query_interface(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> NTSTATUS {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_non_delegating(this) };
        if ppv.is_null() || riid.is_null() {
            core::mem::forget(guard);
            return STATUS_NOINTERFACE;
        }

        let riid = unsafe { &*riid };

        if *riid == IID_IUNKNOWN {
            unsafe { Self::shim_non_delegating_add_ref(this) };
            unsafe { *ppv = this };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        let primary = unsafe { Self::non_delegating_parent_ptr(this) as *mut c_void };
        if let Some(ptr) = <T as ComImpl<P>>::query_interface(&wrapper.inner, primary, riid) {
            let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
            unsafe { ((*vtbl).AddRef)(ptr) };
            unsafe { *ppv = ptr };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        unsafe { *ppv = core::ptr::null_mut() };
        core::mem::forget(guard);
        STATUS_NOINTERFACE
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to a non-delegating IUnknown created by this crate.
    unsafe fn from_non_delegating<'a>(ptr: *mut c_void) -> &'a Self {
        let unknown = unsafe { &*(ptr as *const NonDelegatingIUnknownN<T, P, S, A>) };
        unsafe { &*unknown.parent }
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to a non-delegating IUnknown created by this crate.
    unsafe fn non_delegating_parent_ptr(ptr: *mut c_void) -> *mut Self {
        let unknown = ptr as *mut NonDelegatingIUnknownN<T, P, S, A>;
        unsafe { (*unknown).parent }
    }
}

#[repr(C)]
pub struct ComObject<T, I, A = GlobalAllocator>
where
    T: ComImpl<I>,
    I: InterfaceVtable,
    A: Allocator + Send + Sync,
{
    vtable: &'static I,
    non_delegating_unknown: NonDelegatingIUnknown<T, I, A>,
    ref_count: AtomicU32,
    outer_unknown: Option<*mut c_void>,
    pub inner: T,
    alloc: ManuallyDrop<A>,
}

impl<T, I, A> ComObject<T, I, A>
where
    T: ComImpl<I>,
    I: InterfaceVtable,
    A: Allocator + Send + Sync,
{
    const LAYOUT: Layout = Layout::new::<Self>();
    const NON_DELEGATING_VTABLE: IUnknownVtbl = IUnknownVtbl {
        QueryInterface: Self::shim_non_delegating_query_interface,
        AddRef: Self::shim_non_delegating_add_ref,
        Release: Self::shim_non_delegating_release,
    };

    #[inline]
    fn init_non_delegating_ptr(ptr: *mut Self) {
        unsafe {
            (*ptr).non_delegating_unknown.parent = ptr;
        }
    }

    /// Creates a COM object and returns a smart pointer that owns the initial reference.
    #[inline]
    pub fn new_rc_in<R>(inner: T, alloc: A) -> Result<ComRc<R>, NTSTATUS>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = I>,
    {
        Self::try_new_rc_in(inner, alloc).ok_or(STATUS_INSUFFICIENT_RESOURCES)
    }

    /// Creates a COM object and returns a smart pointer that owns the initial reference.
    #[inline]
    pub fn try_new_rc_in<R>(inner: T, alloc: A) -> Option<ComRc<R>>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = I>,
    {
        let ptr = Self::try_new_in(inner, alloc)?;
        // SAFETY: `ptr` is a freshly created COM pointer with refcount 1.
        Some(unsafe { ComRc::from_raw_unchecked(ptr as *mut R) })
    }

    #[inline]
    fn non_delegating_ptr(ptr: *mut Self) -> *mut c_void {
        unsafe { &mut (*ptr).non_delegating_unknown as *mut _ as *mut c_void }
    }

    #[inline]
    pub fn new_in(inner: T, alloc: A) -> Result<*mut c_void, NTSTATUS> {
        Self::try_new_in(inner, alloc).ok_or(STATUS_INSUFFICIENT_RESOURCES)
    }

    #[inline]
    pub fn try_new_in(inner: T, alloc: A) -> Option<*mut c_void> {
        let ptr = unsafe { alloc.alloc(Self::LAYOUT) } as *mut Self;
        if ptr.is_null() {
            return None;
        }
        unsafe {
            ptr.write(Self {
                vtable: T::VTABLE,
                non_delegating_unknown: NonDelegatingIUnknown {
                    vtable: &Self::NON_DELEGATING_VTABLE,
                    parent: core::ptr::null_mut(),
                },
                ref_count: AtomicU32::new(1),
                outer_unknown: None,
                inner,
                alloc: ManuallyDrop::new(alloc),
            });
            Self::init_non_delegating_ptr(ptr);
            Some(ptr as *mut c_void)
        }
    }

    #[inline]
    pub fn try_new_in_with_layout(inner: T, alloc: A, layout: Layout) -> Option<*mut c_void> {
        if layout != Self::LAYOUT {
            return None;
        }
        Self::try_new_in(inner, alloc)
    }

    /// Creates an aggregated COM object and returns the **non-delegating IUnknown** pointer.
    ///
    /// The outer object should hold this pointer to manage the inner object's lifetime.
    /// Interfaces returned via `QueryInterface` will delegate IUnknown calls to the outer
    /// unknown, while this non-delegating pointer updates the inner refcount directly.
    ///
    /// # Safety
    /// `outer_unknown` must point to a valid outer IUnknown interface pointer.
    #[inline]
    pub unsafe fn new_aggregated_in(
        inner: T,
        outer_unknown: *mut c_void,
        alloc: A,
    ) -> Result<*mut c_void, NTSTATUS> {
        unsafe { Self::try_new_aggregated_in(inner, outer_unknown, alloc) }
            .ok_or(STATUS_INSUFFICIENT_RESOURCES)
    }

    #[inline]
    pub unsafe fn try_new_aggregated_in(
        inner: T,
        outer_unknown: *mut c_void,
        alloc: A,
    ) -> Option<*mut c_void> {
        let ptr = unsafe { alloc.alloc(Self::LAYOUT) } as *mut Self;
        if ptr.is_null() {
            return None;
        }
        unsafe {
            ptr.write(Self {
                vtable: T::VTABLE,
                non_delegating_unknown: NonDelegatingIUnknown {
                    vtable: &Self::NON_DELEGATING_VTABLE,
                    parent: core::ptr::null_mut(),
                },
                ref_count: AtomicU32::new(1),
                outer_unknown: Some(outer_unknown),
                inner,
                alloc: ManuallyDrop::new(alloc),
            });
            Self::init_non_delegating_ptr(ptr);
            Some(Self::non_delegating_ptr(ptr))
        }
    }

    #[inline]
    pub fn is_aggregated(&self) -> bool {
        self.outer_unknown.is_some()
    }

    #[inline]
    pub fn inner_ref(&self) -> &T {
        &self.inner
    }

    #[inline]
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to a `ComObject<T, I>` allocated by this crate.
    /// The pointer must be properly aligned and remain valid for the returned lifetime.
    /// The returned reference must not outlive the underlying COM object allocation.
    pub unsafe fn from_ptr<'a>(ptr: *mut c_void) -> &'a Self {
        unsafe { &*(ptr as *const Self) }
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to a non-delegating IUnknown created by this crate.
    /// The returned reference must not outlive the underlying COM object allocation.
    pub unsafe fn from_non_delegating<'a>(ptr: *mut c_void) -> &'a Self {
        let unknown = unsafe { &*(ptr as *const NonDelegatingIUnknown<T, I, A>) };
        unsafe { &*unknown.parent }
    }

    #[inline(always)]
    /// # Safety
    /// `ptr` must be a valid pointer to a non-delegating IUnknown created by this crate.
    unsafe fn non_delegating_parent_ptr(ptr: *mut c_void) -> *mut Self {
        let unknown = ptr as *mut NonDelegatingIUnknown<T, I, A>;
        unsafe { (*unknown).parent }
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObject` for `T`.
    pub unsafe extern "system" fn shim_add_ref(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_ptr(this) };
        let result = delegating_add_ref(wrapper.outer_unknown, &wrapper.ref_count);
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObject` for `T`.
    pub unsafe extern "system" fn shim_release(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let wrapper = unsafe { &*(this as *const Self) };
        let result = delegating_release(wrapper.outer_unknown, &wrapper.ref_count, || {
            let ptr = this as *mut Self;
            let alloc = core::ptr::read(&(*ptr).alloc);
            let alloc = ManuallyDrop::into_inner(alloc);
            core::ptr::drop_in_place(&mut (*ptr).inner);
            let resurrected = (*ptr).ref_count.load(Ordering::Acquire);
            if resurrected != 0 {
                resurrection_violation();
            }
            alloc.dealloc(ptr as *mut u8, Self::LAYOUT);
            drop(alloc);
        });
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid COM pointer created by `ComObject` for `T`.
    /// `riid` and `ppv` must be valid, non-null pointers.
    pub unsafe extern "system" fn shim_query_interface(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> NTSTATUS {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_ptr(this) };
        if let Some(outer) = wrapper.outer_unknown {
            if !outer.is_null() {
                let vtbl = unsafe { *(outer as *mut *mut IUnknownVtbl) };
                let result = unsafe { ((*vtbl).QueryInterface)(outer, riid, ppv) };
                core::mem::forget(guard);
                return result;
            }
        }

        if ppv.is_null() || riid.is_null() {
            core::mem::forget(guard);
            return STATUS_NOINTERFACE;
        }

        let riid = unsafe { &*riid };

        if *riid == IID_IUNKNOWN {
            unsafe { Self::shim_add_ref(this) };
            unsafe { *ppv = this };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        if let Some(ptr) = wrapper.inner.query_interface(this, riid) {
            let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
            unsafe { ((*vtbl).AddRef)(ptr) };
            unsafe { *ppv = ptr };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        unsafe { *ppv = core::ptr::null_mut() };
        core::mem::forget(guard);
        STATUS_NOINTERFACE
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid non-delegating IUnknown pointer created by `ComObject` for `T`.
    pub unsafe extern "system" fn shim_non_delegating_add_ref(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_non_delegating(this) };
        let result = refcount::add(&wrapper.ref_count);
        core::mem::forget(guard);
        result
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid non-delegating IUnknown pointer created by `ComObject` for `T`.
    pub unsafe extern "system" fn shim_non_delegating_release(this: *mut c_void) -> u32 {
        let guard = PanicGuard::new();
        let ptr = unsafe { Self::non_delegating_parent_ptr(this) };
        let count = refcount::sub(unsafe { &(*ptr).ref_count });

        if count == 0 {
            core::sync::atomic::fence(Ordering::Acquire);
            let alloc = unsafe { core::ptr::read(core::ptr::addr_of!((*ptr).alloc)) };
            let alloc = ManuallyDrop::into_inner(alloc);
            unsafe {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*ptr).inner));
                let resurrected = (*ptr).ref_count.load(Ordering::Acquire);
                if resurrected != 0 {
                    resurrection_violation();
                }
                alloc.dealloc(ptr as *mut u8, Self::LAYOUT);
            }
            drop(alloc);
        }

        core::mem::forget(guard);
        count
    }

    #[allow(non_snake_case)]
    /// # Safety
    /// `this` must be a valid non-delegating IUnknown pointer created by `ComObject` for `T`.
    /// `riid` and `ppv` must be valid, non-null pointers.
    pub unsafe extern "system" fn shim_non_delegating_query_interface(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> NTSTATUS {
        let guard = PanicGuard::new();
        let wrapper = unsafe { Self::from_non_delegating(this) };

        if ppv.is_null() || riid.is_null() {
            core::mem::forget(guard);
            return STATUS_NOINTERFACE;
        }

        let riid = unsafe { &*riid };

        if *riid == IID_IUNKNOWN {
            unsafe { Self::shim_non_delegating_add_ref(this) };
            unsafe { *ppv = this };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        let delegating_ptr = wrapper as *const Self as *mut c_void;

        if let Some(ptr) = wrapper.inner.query_interface(delegating_ptr, riid) {
            let vtbl = unsafe { *(ptr as *mut *mut IUnknownVtbl) };
            unsafe { ((*vtbl).AddRef)(ptr) };
            unsafe { *ppv = ptr };
            core::mem::forget(guard);
            return STATUS_SUCCESS;
        }

        unsafe { *ppv = core::ptr::null_mut() };
        core::mem::forget(guard);
        STATUS_NOINTERFACE
    }
}

impl<T, I> ComObject<T, I, GlobalAllocator>
where
    T: ComImpl<I>,
    I: InterfaceVtable,
{
    #[inline]
    pub fn new(inner: T) -> Result<*mut c_void, NTSTATUS> {
        Self::new_in(inner, GlobalAllocator)
    }

    #[inline]
    pub fn try_new(inner: T) -> Option<*mut c_void> {
        Self::try_new_in(inner, GlobalAllocator)
    }

    /// Creates a COM object and returns a smart pointer that owns the initial reference.
    #[inline]
    pub fn new_rc<R>(inner: T) -> Result<ComRc<R>, NTSTATUS>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = I>,
    {
        Self::new_rc_in(inner, GlobalAllocator)
    }

    /// Creates a COM object and returns a smart pointer that owns the initial reference.
    #[inline]
    pub fn try_new_rc<R>(inner: T) -> Option<ComRc<R>>
    where
        R: ComInterface + ComInterfaceInfo<Vtable = I>,
    {
        Self::try_new_rc_in(inner, GlobalAllocator)
    }

    /// # Safety
    /// `outer_unknown` must point to a valid outer IUnknown interface pointer.
    #[inline]
    pub unsafe fn new_aggregated(
        inner: T,
        outer_unknown: *mut c_void,
    ) -> Result<*mut c_void, NTSTATUS> {
        unsafe { Self::new_aggregated_in(inner, outer_unknown, GlobalAllocator) }
    }

    /// # Safety
    /// `outer_unknown` must point to a valid outer IUnknown interface pointer.
    #[inline]
    pub unsafe fn try_new_aggregated(
        inner: T,
        outer_unknown: *mut c_void,
    ) -> Option<*mut c_void> {
        unsafe { Self::try_new_aggregated_in(inner, outer_unknown, GlobalAllocator) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use core::sync::atomic::{AtomicU32, Ordering};

    static DROP_COUNT: AtomicU32 = AtomicU32::new(0);
    static OUTER_ADDREF_COUNT: AtomicU32 = AtomicU32::new(0);
    static OUTER_RELEASE_COUNT: AtomicU32 = AtomicU32::new(0);
    static OUTER_QUERY_COUNT: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn outer_add_ref(this: *mut core::ffi::c_void) -> u32 {
        assert!(!this.is_null());
        OUTER_ADDREF_COUNT.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn outer_release(this: *mut core::ffi::c_void) -> u32 {
        assert!(!this.is_null());
        OUTER_RELEASE_COUNT.fetch_add(1, Ordering::Relaxed) + 1
    }

    unsafe extern "system" fn outer_query_interface(
        this: *mut core::ffi::c_void,
        _riid: *const GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> NTSTATUS {
        assert!(!this.is_null());
        OUTER_QUERY_COUNT.fetch_add(1, Ordering::Relaxed);
        if !ppv.is_null() {
            unsafe { *ppv = core::ptr::null_mut() };
        }
        STATUS_NOINTERFACE
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct OuterUnknown {
        lpVtbl: *const IUnknownVtbl,
    }

    static OUTER_VTBL: IUnknownVtbl = IUnknownVtbl {
        QueryInterface: outer_query_interface,
        AddRef: outer_add_ref,
        Release: outer_release,
    };

    struct Dummy;

    impl Drop for Dummy {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn add_ref_release_drops() {
        DROP_COUNT.store(0, Ordering::Relaxed);

        let ptr = ComObject::<Dummy, IUnknownVtbl>::new(Dummy).unwrap();

        unsafe {
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_add_ref(ptr), 2);
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(ptr), 1);
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(ptr), 0);
        }

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn query_interface_iunknown_returns_self() {
        DROP_COUNT.store(0, Ordering::Relaxed);

        let ptr = ComObject::<Dummy, IUnknownVtbl>::new(Dummy).unwrap();
        let mut out = core::ptr::null_mut();

        let status = unsafe {
            ComObject::<Dummy, IUnknownVtbl>::shim_query_interface(
                ptr,
                &IID_IUNKNOWN,
                &mut out,
            )
        };

        assert_eq!(status, STATUS_SUCCESS);
        assert_eq!(out, ptr);

        unsafe {
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(ptr), 1);
            assert_eq!(ComObject::<Dummy, IUnknownVtbl>::shim_release(ptr), 0);
        }

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn aggregated_non_delegating_and_delegating_paths() {
        DROP_COUNT.store(0, Ordering::Relaxed);
        OUTER_ADDREF_COUNT.store(0, Ordering::Relaxed);
        OUTER_RELEASE_COUNT.store(0, Ordering::Relaxed);
        OUTER_QUERY_COUNT.store(0, Ordering::Relaxed);

        let outer = OuterUnknown {
            lpVtbl: &OUTER_VTBL as *const _,
        };
        let ptr = unsafe {
            ComObject::<Dummy, IUnknownVtbl>::new_aggregated(
                Dummy,
                &outer as *const _ as *mut core::ffi::c_void,
            )
        }
        .unwrap();

        unsafe {
            let vtbl = *(ptr as *mut *mut IUnknownVtbl);
            assert_eq!(((*vtbl).AddRef)(ptr), 2);
            assert_eq!(((*vtbl).Release)(ptr), 1);
        }

        assert_eq!(OUTER_ADDREF_COUNT.load(Ordering::Relaxed), 0);
        assert_eq!(OUTER_RELEASE_COUNT.load(Ordering::Relaxed), 0);

        let delegating_ptr = unsafe {
            ComObject::<Dummy, IUnknownVtbl>::from_non_delegating(ptr) as *const _
                as *mut core::ffi::c_void
        };

        unsafe {
            assert_eq!(
                ComObject::<Dummy, IUnknownVtbl>::shim_add_ref(delegating_ptr),
                1
            );
            assert_eq!(
                ComObject::<Dummy, IUnknownVtbl>::shim_release(delegating_ptr),
                1
            );
        }

        assert_eq!(OUTER_ADDREF_COUNT.load(Ordering::Relaxed), 1);
        assert_eq!(OUTER_RELEASE_COUNT.load(Ordering::Relaxed), 1);

        unsafe {
            let vtbl = *(ptr as *mut *mut IUnknownVtbl);
            assert_eq!(((*vtbl).Release)(ptr), 0);
        }

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), 1);
    }
}
