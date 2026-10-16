//! One desktop HID adapter for the platform-neutral `quadcortex` library.
//!
//! This crate owns enumeration, native RX/TX workers and diagnostics. Session,
//! framing, commands, admission, delivery and state decisions remain in
//! `quadcortex`. Only the handle and native completion policy vary by platform.

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
compile_error!("quadcortex-hid supports Windows, Linux and macOS hosts");

mod backend;
pub mod flight;
pub mod latest_cache;
mod retained_io;
pub mod usb;

pub use flight::FlightRecorder;
pub use latest_cache::{
    insert_latest_message, LatestCacheRejection, LATEST_MESSAGE_CACHE_MAX_BYTES,
    LATEST_MESSAGE_CACHE_MAX_ENTRIES,
};
pub use usb::{
    preset_name, scene_value, ConnectedQc, IncomingMessage, QcUsb, UsbError, UsbTelemetry,
    WriteDeliveryCounts,
};
