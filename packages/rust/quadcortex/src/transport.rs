//! Platform-neutral transport framing and lifecycle state.
//!
//! Re-exports [`crate::device::transport`], the HID/USB report framing and
//! handshake state machine that hosts drive with a monotonic clock and a
//! byte-oriented write action. The crate does not open a platform USB or
//! MIDI handle itself; hosts supply that thin adapter.

pub use crate::device::transport::*;
