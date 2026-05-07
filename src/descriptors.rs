// Copyright (c) 2026 Exveria
// SPDX-License-Identifier: MIT OR Apache-2.0
//!
//! Descriptor DSL mapping
//!
//! This module provides macros that build PortCls descriptor structs. Each
//! macro maps directly to the underlying C struct fields:
//!
//! - `pcpin_descriptor!` -> `PCPIN_DESCRIPTOR`
//!   - `max_global_instances` -> `MaxGlobalInstanceCount`
//!   - `max_filter_instances` -> `MaxFilterInstanceCount`
//!   - `min_filter_instances` -> `MinFilterInstanceCount`
//!   - `automation` -> `AutomationTable`
//!   - `kspin` -> `KsPinDescriptor`
//! - `kspin_descriptor!` -> `KSPIN_DESCRIPTOR`
//!   - `interfaces`, `mediums`, `data_ranges` -> count + pointer fields
//!   - `data_ranges` typically uses `KSDATARANGE` entries and can be built
//!     with `ksdatarange_audio!` for `KSDATARANGE_AUDIO` ranges.
//! - `ksdataformat!` -> `KSDATAFORMAT`
//! - `pcautomation_table!` -> `PCAUTOMATION_TABLE`
//!   - each list produces a count + pointer pair (null when empty)
//! - `pcnode_descriptor!` -> `PCNODE_DESCRIPTOR`
//! - `pcconnection_descriptor!` -> `PCCONNECTION_DESCRIPTOR`
//! - `define_descriptor!` -> `PCFILTER_DESCRIPTOR`
//!   - arrays are emitted as internal `static` arrays; empty lists map to
//!     null pointers with count fields set to zero.

#[doc(hidden)]
#[macro_export]
macro_rules! __kcom_count_exprs {
    ($($expr:expr),* $(,)?) => {
        <[()]>::len(&[$($crate::__kcom_count_exprs!(@unit $expr)),*])
    };
    (@unit $expr:expr) => { () };
}

/// Builds a `PCPIN_DESCRIPTOR` value.
///
/// Expects the `PCPIN_DESCRIPTOR` and `KSPIN_DESCRIPTOR` types to be in scope.
#[macro_export]
macro_rules! pcpin_descriptor {
    (
        max_global_instances: $max_global:expr,
        max_filter_instances: $max_filter:expr,
        min_filter_instances: $min_filter:expr,
        automation: $automation:expr,
        kspin: $kspin:expr $(,)?
    ) => {
        PCPIN_DESCRIPTOR {
            MaxGlobalInstanceCount: $max_global,
            MaxFilterInstanceCount: $max_filter,
            MinFilterInstanceCount: $min_filter,
            AutomationTable: $automation,
            KsPinDescriptor: $kspin,
        }
    };
}

/// Builds a `KSDATARANGE_AUDIO` value with RangeInclusive syntax.
///
/// Expects `KSDATARANGE_AUDIO` and `KSDATARANGE` to be in scope.
#[macro_export]
macro_rules! ksdatarange_audio {
    (
        data_range: $data_range:expr,
        max_channels: $max_channels:expr,
        bits_per_sample: $min_bits:tt ..= $max_bits:tt,
        sample_frequency: $min_freq:tt ..= $max_freq:tt $(,)?
    ) => {
        KSDATARANGE_AUDIO {
            DataRange: $data_range,
            MaximumChannels: $max_channels,
            MinimumBitsPerSample: ($min_bits),
            MaximumBitsPerSample: ($max_bits),
            MinimumSampleFrequency: ($min_freq),
            MaximumSampleFrequency: ($max_freq),
        }
    };
}

/// Builds a `KSIDENTIFIER` value (used by `KSPIN_INTERFACE`/`KSPIN_MEDIUM`).
///
/// Expects `KSIDENTIFIER`, `KSIDENTIFIER__bindgen_ty_1`, and
/// `KSIDENTIFIER__bindgen_ty_1__bindgen_ty_1` to be in scope.
#[macro_export]
macro_rules! ksidentifier {
    (
        set: $set:expr,
        id: $id:expr,
        flags: $flags:expr $(,)?
    ) => {
        KSIDENTIFIER {
            __bindgen_anon_1: KSIDENTIFIER__bindgen_ty_1 {
                __bindgen_anon_1: KSIDENTIFIER__bindgen_ty_1__bindgen_ty_1 {
                    Set: $set,
                    Id: $id,
                    Flags: $flags,
                },
            },
        }
    };
}

/// Builds a `KSPIN_INTERFACE` value.
///
/// Expects `KSPIN_INTERFACE` aliases `KSIDENTIFIER` and the `ksidentifier!`
/// macro to be in scope.
#[macro_export]
macro_rules! kspin_interface {
    (
        set: $set:expr,
        id: $id:expr,
        flags: $flags:expr $(,)?
    ) => {
        $crate::ksidentifier!(set: $set, id: $id, flags: $flags)
    };
}

/// Builds a `KSPIN_MEDIUM` value.
///
/// Expects `KSPIN_MEDIUM` aliases `KSIDENTIFIER` and the `ksidentifier!`
/// macro to be in scope.
#[macro_export]
macro_rules! kspin_medium {
    (
        set: $set:expr,
        id: $id:expr,
        flags: $flags:expr $(,)?
    ) => {
        $crate::ksidentifier!(set: $set, id: $id, flags: $flags)
    };
}

/// Builds a `PCPROPERTY_ITEM` value.
///
/// Expects `PCPROPERTY_ITEM` and `PCPFNPROPERTY_HANDLER` types to be in scope.
#[macro_export]
macro_rules! pcproperty_item {
    (
        set: $set:expr,
        id: $id:expr,
        flags: $flags:expr,
        handler: $handler:expr $(,)?
    ) => {
        PCPROPERTY_ITEM {
            Set: $set,
            Id: $id,
            Flags: $flags,
            Handler: $handler,
        }
    };
}

/// Builds a `PCMETHOD_ITEM` value.
///
/// Expects `PCMETHOD_ITEM` and `PCPFNMETHOD_HANDLER` types to be in scope.
#[macro_export]
macro_rules! pcmethod_item {
    (
        set: $set:expr,
        id: $id:expr,
        flags: $flags:expr,
        handler: $handler:expr $(,)?
    ) => {
        PCMETHOD_ITEM {
            Set: $set,
            Id: $id,
            Flags: $flags,
            Handler: $handler,
        }
    };
}

/// Builds a `PCEVENT_ITEM` value.
///
/// Expects `PCEVENT_ITEM` and `PCPFNEVENT_HANDLER` types to be in scope.
#[macro_export]
macro_rules! pcevent_item {
    (
        set: $set:expr,
        id: $id:expr,
        flags: $flags:expr,
        handler: $handler:expr $(,)?
    ) => {
        PCEVENT_ITEM {
            Set: $set,
            Id: $id,
            Flags: $flags,
            Handler: $handler,
        }
    };
}

/// Builds a `PCAUTOMATION_TABLE` value.
///
/// Expects the `PCAUTOMATION_TABLE`, `PCPROPERTY_ITEM`, `PCMETHOD_ITEM`, and
/// `PCEVENT_ITEM` types to be in scope.
#[macro_export]
macro_rules! pcautomation_table {
    (
        properties: $property_ty:ty => [$($properties:expr),* $(,)?],
        methods: $method_ty:ty => [$($methods:expr),* $(,)?],
        events: $event_ty:ty => [$($events:expr),* $(,)?],
        $(,)?
    ) => {{
        static PROPERTIES: [$property_ty; $crate::__kcom_count_exprs!($($properties),*)] = [
            $($properties),*
        ];
        static METHODS: [$method_ty; $crate::__kcom_count_exprs!($($methods),*)] = [$($methods),*];
        static EVENTS: [$event_ty; $crate::__kcom_count_exprs!($($events),*)] = [$($events),*];

        PCAUTOMATION_TABLE {
            PropertyItemSize: ::core::mem::size_of::<$property_ty>() as _,
            PropertyCount: PROPERTIES.len() as _,
            Properties: if PROPERTIES.len() == 0 {
                ::core::ptr::null()
            } else {
                PROPERTIES.as_ptr()
            },
            MethodItemSize: ::core::mem::size_of::<$method_ty>() as _,
            MethodCount: METHODS.len() as _,
            Methods: if METHODS.len() == 0 {
                ::core::ptr::null()
            } else {
                METHODS.as_ptr()
            },
            EventItemSize: ::core::mem::size_of::<$event_ty>() as _,
            EventCount: EVENTS.len() as _,
            Events: if EVENTS.len() == 0 {
                ::core::ptr::null()
            } else {
                EVENTS.as_ptr()
            },
            Reserved: 0,
        }
    }};
}

/// Builds a `PCNODE_DESCRIPTOR` value.
///
/// Expects the `PCNODE_DESCRIPTOR` type to be in scope.
#[macro_export]
macro_rules! pcnode_descriptor {
    (
        flags: $flags:expr,
        automation: $automation:expr,
        node_type: $node_type:expr,
        node_name: $node_name:expr $(,)?
    ) => {
        PCNODE_DESCRIPTOR {
            Flags: $flags,
            AutomationTable: $automation,
            Type: $node_type,
            Name: $node_name,
        }
    };
}

/// Builds a `KSTOPOLOGY_CONNECTION` value for `PCCONNECTION_DESCRIPTOR`.
///
/// Expects the `KSTOPOLOGY_CONNECTION` type to be in scope.
#[macro_export]
macro_rules! pcconnection_descriptor {
    (
        from_node: $from_node:expr,
        from_pin: $from_pin:expr,
        to_node: $to_node:expr,
        to_pin: $to_pin:expr $(,)?
    ) => {
        KSTOPOLOGY_CONNECTION {
            FromNode: $from_node,
            FromNodePin: $from_pin,
            ToNode: $to_node,
            ToNodePin: $to_pin,
        }
    };
}

/// Builds a `KSPIN_DESCRIPTOR` value.
///
/// Expects `KSPIN_DESCRIPTOR`, `KSPIN_DESCRIPTOR__bindgen_ty_1`, and
/// `KSPIN_DESCRIPTOR__bindgen_ty_1__bindgen_ty_1` types to be in scope.
#[macro_export]
macro_rules! kspin_descriptor {
    (
        interfaces: $iface_ty:ty => [$($interfaces:expr),* $(,)?],
        mediums: $medium_ty:ty => [$($mediums:expr),* $(,)?],
        data_ranges: $range_ty:ty => [$($ranges:expr),* $(,)?],
        data_flow: $data_flow:expr,
        communication: $communication:expr,
        category: $category:expr,
        name: $name:expr,
        constrained_data_ranges: $constrained_ty:ty => [$($constrained:expr),* $(,)?],
        $(,)?
    ) => {{
        static IFACES: [$iface_ty; $crate::__kcom_count_exprs!($($interfaces),*)] = [
            $($interfaces),*
        ];
        static MEDIUMS: [$medium_ty; $crate::__kcom_count_exprs!($($mediums),*)] = [
            $($mediums),*
        ];
        static RANGES: [$range_ty; $crate::__kcom_count_exprs!($($ranges),*)] = [$($ranges),*];
        static CONSTRAINED: [$constrained_ty; $crate::__kcom_count_exprs!($($constrained),*)] = [
            $($constrained),*
        ];

        KSPIN_DESCRIPTOR {
            InterfacesCount: IFACES.len() as _,
            Interfaces: if IFACES.len() == 0 {
                ::core::ptr::null()
            } else {
                IFACES.as_ptr()
            },
            MediumsCount: MEDIUMS.len() as _,
            Mediums: if MEDIUMS.len() == 0 {
                ::core::ptr::null()
            } else {
                MEDIUMS.as_ptr()
            },
            DataRangesCount: RANGES.len() as _,
            DataRanges: if RANGES.len() == 0 {
                ::core::ptr::null()
            } else {
                RANGES.as_ptr()
            },
            DataFlow: $data_flow,
            Communication: $communication,
            Category: $category,
            Name: $name,
            __bindgen_anon_1: KSPIN_DESCRIPTOR__bindgen_ty_1 {
                __bindgen_anon_1: KSPIN_DESCRIPTOR__bindgen_ty_1__bindgen_ty_1 {
                    ConstrainedDataRangesCount: CONSTRAINED.len() as _,
                    ConstrainedDataRanges: if CONSTRAINED.len() == 0 {
                        ::core::ptr::null_mut()
                    } else {
                        CONSTRAINED.as_ptr() as *mut _
                    },
                },
            },
        }
    }};
    (
        interfaces: $iface_ty:ty => [$($interfaces:expr),* $(,)?],
        mediums: $medium_ty:ty => [$($mediums:expr),* $(,)?],
        data_ranges: $range_ty:ty => [$($ranges:expr),* $(,)?],
        data_flow: $data_flow:expr,
        communication: $communication:expr,
        category: $category:expr,
        name: $name:expr
        $(,)?
    ) => {{
        static IFACES: [$iface_ty; $crate::__kcom_count_exprs!($($interfaces),*)] = [
            $($interfaces),*
        ];
        static MEDIUMS: [$medium_ty; $crate::__kcom_count_exprs!($($mediums),*)] = [
            $($mediums),*
        ];
        static RANGES: [$range_ty; $crate::__kcom_count_exprs!($($ranges),*)] = [$($ranges),*];

        KSPIN_DESCRIPTOR {
            InterfacesCount: IFACES.len() as _,
            Interfaces: if IFACES.len() == 0 {
                ::core::ptr::null()
            } else {
                IFACES.as_ptr()
            },
            MediumsCount: MEDIUMS.len() as _,
            Mediums: if MEDIUMS.len() == 0 {
                ::core::ptr::null()
            } else {
                MEDIUMS.as_ptr()
            },
            DataRangesCount: RANGES.len() as _,
            DataRanges: if RANGES.len() == 0 {
                ::core::ptr::null()
            } else {
                RANGES.as_ptr()
            },
            DataFlow: $data_flow,
            Communication: $communication,
            Category: $category,
            Name: $name,
            __bindgen_anon_1: KSPIN_DESCRIPTOR__bindgen_ty_1 { Reserved: 0 },
        }
    }};
}

/// Builds a `KSDATAFORMAT` value.
///
/// Expects `KSDATAFORMAT` and `KSDATAFORMAT__bindgen_ty_1` to be in scope.
#[macro_export]
macro_rules! ksdataformat {
    (
        format_size: $format_size:expr,
        flags: $flags:expr,
        sample_size: $sample_size:expr,
        reserved: $reserved:expr,
        major_format: $major_format:expr,
        sub_format: $sub_format:expr,
        specifier: $specifier:expr $(,)?
    ) => {
        KSDATAFORMAT {
            __bindgen_anon_1: KSDATAFORMAT__bindgen_ty_1 {
                FormatSize: $format_size,
                Flags: $flags,
                SampleSize: $sample_size,
                Reserved: $reserved,
                MajorFormat: $major_format,
                SubFormat: $sub_format,
                Specifier: $specifier,
            },
        }
    };
}
/// Defines a static filter descriptor and its backing arrays.
///
/// This macro removes manual count/size bookkeeping for filter, pin, node,
/// connection, and category arrays.
///
/// ## Example
/// ```ignore
/// kcom::define_descriptor! {
///     pub static FILTER: PCFILTER_DESCRIPTOR = {
///         version: 0,
///         automation: core::ptr::null_mut(),
///         pins: PCPIN_DESCRIPTOR => [],
///         nodes: PCNODE_DESCRIPTOR => [],
///         connections: PCCONNECTION_DESCRIPTOR => [],
///         categories: GUID => [],
///     };
/// }
/// ```
#[macro_export]
macro_rules! define_descriptor {
    (
        $(#[$attr:meta])*
        $vis:vis static $name:ident : $filter_ty:ty = {
            version: $version:expr,
            automation: $automation:expr,
            pins: $pin_ty:ty => [$($pins:expr),* $(,)?],
            nodes: $node_ty:ty => [$($nodes:expr),* $(,)?],
            connections: $connection_ty:ty => [$($connections:expr),* $(,)?],
            categories: $category_ty:ty => [$($categories:expr),* $(,)?],
            $(,)?
        };
    ) => {
        $crate::paste::paste! {
            static [<__KCOM_ $name _PINS>]: [$pin_ty; $crate::__kcom_count_exprs!($($pins),*)] = [
                $($pins),*
            ];
            static [<__KCOM_ $name _NODES>]: [$node_ty; $crate::__kcom_count_exprs!($($nodes),*)] = [
                $($nodes),*
            ];
            static [<__KCOM_ $name _CONNECTIONS>]: [$connection_ty; $crate::__kcom_count_exprs!($($connections),*)] = [
                $($connections),*
            ];
            static [<__KCOM_ $name _CATEGORIES>]: [$category_ty; $crate::__kcom_count_exprs!($($categories),*)] = [
                $($categories),*
            ];

            $(#[$attr])*
            $vis static $name: $filter_ty = $filter_ty {
                Version: $version,
                AutomationTable: $automation,
                PinSize: ::core::mem::size_of::<$pin_ty>() as _,
                PinCount: [<__KCOM_ $name _PINS>].len() as _,
                Pins: if [<__KCOM_ $name _PINS>].len() == 0 {
                    ::core::ptr::null()
                } else {
                    [<__KCOM_ $name _PINS>].as_ptr()
                },
                NodeSize: ::core::mem::size_of::<$node_ty>() as _,
                NodeCount: [<__KCOM_ $name _NODES>].len() as _,
                Nodes: if [<__KCOM_ $name _NODES>].len() == 0 {
                    ::core::ptr::null()
                } else {
                    [<__KCOM_ $name _NODES>].as_ptr()
                },
                ConnectionCount: [<__KCOM_ $name _CONNECTIONS>].len() as _,
                Connections: if [<__KCOM_ $name _CONNECTIONS>].len() == 0 {
                    ::core::ptr::null()
                } else {
                    [<__KCOM_ $name _CONNECTIONS>].as_ptr()
                },
                CategoryCount: [<__KCOM_ $name _CATEGORIES>].len() as _,
                Categories: if [<__KCOM_ $name _CATEGORIES>].len() == 0 {
                    ::core::ptr::null()
                } else {
                    [<__KCOM_ $name _CATEGORIES>].as_ptr()
                },
            };
        }
    };
}
