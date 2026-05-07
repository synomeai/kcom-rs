// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0

#![allow(non_camel_case_types)]

#[cfg(all(feature = "driver", not(miri)))]
pub use wdk_sys::{
    APC_LEVEL, DISPATCH_LEVEL, EVENT_TYPE, KWAIT_REASON, KEVENT, UNICODE_STRING, _EVENT_TYPE,
    _KWAIT_REASON, _MODE,
};
#[cfg(all(feature = "driver", not(miri)))]
pub use wdk_sys::{KDPC, KTIMER, LARGE_INTEGER, PKDPC, PKTIMER};
#[cfg(all(feature = "driver", not(miri)))]
pub use wdk_sys::{KIRQL, KSPIN_LOCK};
#[cfg(all(feature = "driver", not(miri)))]
pub use wdk_sys::ntddk::{
    KeAcquireSpinLockRaiseToDpc, KeBugCheckEx, KeCancelTimer, KeGetCurrentIrql, KeInitializeDpc,
    KeInitializeEvent, KeInitializeSpinLock, KeInitializeTimer, KeInsertQueueDpc,
    KeQueryPerformanceCounter, KeReleaseSpinLock, KeRemoveQueueDpc, KeSetEvent, KeSetTimer,
    KeWaitForSingleObject, MmGetSystemRoutineAddress,
};
#[cfg(all(feature = "driver", not(miri)))]
pub use wdk_sys::_EVENT_TYPE::SynchronizationEvent;

#[cfg(any(not(feature = "driver"), miri))]
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
pub struct UNICODE_STRING {
    pub Length: u16,
    pub MaximumLength: u16,
    pub Buffer: *mut u16,
}

// UNICODE_STRING is a plain data carrier; sharing is safe as long as the buffer
// lifetime is managed by the caller (mirrors kernel ABI expectations).
#[cfg(any(not(feature = "driver"), miri))]
unsafe impl Send for UNICODE_STRING {}
#[cfg(any(not(feature = "driver"), miri))]
unsafe impl Sync for UNICODE_STRING {}


#[cfg(all(feature = "async-com-kernel", driver_model__driver_type = "WDM", not(miri)))]
pub use wdk_sys::ntddk::{
    DEVICE_OBJECT, IoAllocateWorkItem, IoFreeWorkItem, IoQueueWorkItem, ObDereferenceObject,
    ObReferenceObject, PIO_WORKITEM, WORK_QUEUE_TYPE,
};

#[cfg(all(feature = "async-com-kernel", driver_model__driver_type = "KMDF", not(miri)))]
pub use wdk_sys::wdf::{
    WDFOBJECT, WDFDEVICE, WDFWORKITEM, PFN_WDF_WORKITEM, WDF_WORKITEM_CONFIG,
    WDF_OBJECT_ATTRIBUTES, WDF_OBJECT_CONTEXT_TYPE_INFO, WdfWorkItemCreate,
    WdfWorkItemEnqueue, WdfObjectDelete, WdfObjectGetTypedContextWorker,
};

#[cfg(all(feature = "async-com-kernel", driver_model__driver_type = "KMDF", not(miri)))]
#[inline]
pub fn wdf_workitem_config_init(evt: PFN_WDF_WORKITEM) -> WDF_WORKITEM_CONFIG {
    WDF_WORKITEM_CONFIG {
        Size: core::mem::size_of::<WDF_WORKITEM_CONFIG>() as u32,
        EvtWorkItem: evt,
        AutomaticSerialization: 0,
    }
}

#[cfg(all(feature = "async-com-kernel", driver_model__driver_type = "KMDF", not(miri)))]
#[inline]
pub fn wdf_object_attributes_init(
    parent: WDFOBJECT,
    context_size: usize,
    context_type_info: *const WDF_OBJECT_CONTEXT_TYPE_INFO,
) -> WDF_OBJECT_ATTRIBUTES {
    WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<WDF_OBJECT_ATTRIBUTES>() as u32,
        EvtCleanupCallback: None,
        EvtDestroyCallback: None,
        ExecutionLevel: 0,
        SynchronizationScope: 0,
        ParentObject: parent,
        ContextSizeOverride: context_size,
        ContextTypeInfo: context_type_info,
    }
}
