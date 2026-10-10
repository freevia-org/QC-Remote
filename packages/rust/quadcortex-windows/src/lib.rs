//! Windows-native USB/HID transport for the Quad Cortex.
//!
//! This crate holds the generic HID-transport logic that used to live inside
//! `services/device-broker`: device enumeration/opening by vendor/product ID,
//! the RX/TX thread pair, the write loop and its Windows-HID-driver-stack
//! quirk workaround, the handshake retry loop, and the flight-recorder
//! diagnostics ring buffer those threads drive. It depends on `quadcortex`
//! for protocol/session logic but has no knowledge of `device-broker`'s
//! reconnect-trigger policy or JSON-RPC surface -- those stay in
//! `device-broker` because they are entangled with its own request/response
//! and broker-status types.
//!
//! Matches the existing one-crate-per-platform convention
//! (`quadcortex-android`, `qc-windows-midi`); a cfg-gated multi-platform
//! crate was deliberately not attempted here.

pub mod flight;
pub mod latest_cache;
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
