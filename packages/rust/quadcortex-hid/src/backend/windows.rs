use super::WriteStallPolicy;
use crate::usb::UsbError;
use hidapi::{HidApi, HidDevice, HidError};
use std::cell::UnsafeCell;
use std::ffi::CStr;

pub(crate) fn device_id(device: &hidapi::DeviceInfo) -> String {
    crate::usb::device_path_id(device.path())
}

/// One native handle with independent full-duplex read and write lanes.
///
/// hidapi's pure-Rust Windows backend keeps distinct buffers, events and
/// OVERLAPPED records for those operations. This crate relies on that split:
/// one permanent RX thread is already waiting when the command thread writes
/// a report.
pub(crate) struct SharedWindowsHid(UnsafeCell<HidDevice>);

// SAFETY: exactly one RX thread calls read_timeout and exactly one TX thread
// calls write. The windows-native backend touches disjoint read_state and
// write_state RefCells, read_pending is RX-only and the native handle is
// immutable. Both lanes hold an `Arc` clone, so the handle outlives any lane
// still running. HidIo bounds observation of both original RX/TX threads and
// retains unacknowledged join owners rather than detaching them. The closure's
// NativeLifetime drops this native handle before its host lease capability;
// the original thread owner separately retains local admission through native
// thread signal and join. Native-handle destruction stays on the I/O thread,
// since kernel cancellation may block. Audited against hidapi 2.6.7 only: the crate Cargo.toml
// pins `hidapi = "=2.6.7"`, and this impl is unsound against a backend that
// shares state between the lanes. Re-audit it before bumping the pin.
unsafe impl Sync for SharedWindowsHid {}

impl SharedWindowsHid {
    pub(crate) fn open(
        api: &HidApi,
        path: &CStr,
        _selected_identity: &str,
    ) -> Result<Self, UsbError> {
        api.open_path(path)
            .map(|device| Self(UnsafeCell::new(device)))
            .map_err(|_| UsbError::NotAvailable)
    }

    pub(crate) fn read_timeout(&self, report: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        // SAFETY: see the `unsafe impl Sync` above. This is the RX lane's only call into the device
        // and only the single qc-native-hid-rx thread makes it, so it never overlaps itself; it
        // touches the read-side state of the hidapi 2.6.7 backend, which no TX call touches. The
        // shared reference ends with the call, and the `Arc` holding `self` keeps the device alive.
        unsafe { (&*self.0.get()).read_timeout(report, timeout_ms) }
            .map_err(|error| error.to_string())
    }

    pub(crate) fn write(&self, report: &[u8]) -> Result<usize, HidError> {
        // SAFETY: see the `unsafe impl Sync` above. This is the TX lane's only call into the device
        // and only the single qc-native-hid-tx thread makes it, so it never overlaps itself; it
        // touches the write-side state of the hidapi 2.6.7 backend, which no RX call touches. The
        // shared reference ends with the call, and the `Arc` holding `self` keeps the device alive.
        unsafe { (&*self.0.get()).write(report) }
    }

    pub(crate) fn write_stall_policy(&self) -> WriteStallPolicy {
        WriteStallPolicy::WindowsQcError31
    }
}

/// The QC accepts an output report and then deliberately STALLs its Windows
/// status stage. The windows-native hidapi backend surfaces that completed
/// delivery as ERROR_GEN_FAILURE (31). Continue the logical frame for that
/// one device-specific result; every other write failure remains terminal.
pub(crate) fn write_report_delivered(
    result: &Result<usize, HidError>,
    expected_bytes: usize,
    policy: WriteStallPolicy,
) -> bool {
    match result {
        Ok(written) => *written == expected_bytes,
        Err(HidError::IoError { error }) => {
            policy == WriteStallPolicy::WindowsQcError31 && error.raw_os_error() == Some(31)
        }
        Err(_) => false,
    }
}
