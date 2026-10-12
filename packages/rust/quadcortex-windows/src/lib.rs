//! Compatibility facade for the Windows HID host API.
//!
//! The single desktop HID engine lives in `quadcortex-hid`; its Windows
//! backend preserves the native handle, threading and status-stall policy.
//! Existing Windows clients keep the same public module and type paths.

pub mod flight;
pub mod latest_cache;
pub mod usb;

pub use quadcortex_hid::{
    insert_latest_message, preset_name, scene_value, ConnectedQc, FlightRecorder, IncomingMessage,
    LatestCacheRejection, QcUsb, UsbError, UsbTelemetry, WriteDeliveryCounts,
    LATEST_MESSAGE_CACHE_MAX_BYTES, LATEST_MESSAGE_CACHE_MAX_ENTRIES,
};
