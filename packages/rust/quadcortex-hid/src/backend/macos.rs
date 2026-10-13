use super::WriteStallPolicy;
use crate::usb::UsbError;
use hidapi::{HidApi, HidDevice, HidError};
use std::ffi::CStr;
use std::sync::Mutex;

pub(crate) fn device_id(device: &hidapi::DeviceInfo) -> String {
    let serial = device.serial_number().filter(|serial| !serial.is_empty());
    crate::usb::macos_device_identity(device.path(), serial)
}

/// macOS hidapi calls are serialized through a mutex. In particular, this
/// avoids relying on undocumented concurrent access to the CoreHID handle.
/// Reads use the same bounded timeout as other platforms, so commands can
/// wait at most one read interval to acquire the handle.
pub(crate) struct SharedMacosHid(Mutex<HidDevice>);

impl SharedMacosHid {
    pub(crate) fn open(
        api: &HidApi,
        path: &CStr,
        selected_identity: &str,
    ) -> Result<Self, UsbError> {
        let device = api.open_path(path).map_err(|_| UsbError::NotAvailable)?;
        let opened_info = device.get_device_info().map_err(|error| {
            UsbError::Hid(format!("cannot verify the opened HID identity: {error}"))
        })?;
        if device_id(&opened_info) != selected_identity {
            return Err(UsbError::NotAvailable);
        }
        Ok(Self(Mutex::new(device)))
    }

    pub(crate) fn read_timeout(&self, report: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .read_timeout(report, timeout_ms)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn write(&self, report: &[u8]) -> Result<usize, HidError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write(report)
    }

    pub(crate) fn write_stall_policy(&self) -> WriteStallPolicy {
        WriteStallPolicy::MacosSerialized
    }
}

pub(crate) fn write_report_delivered(
    result: &Result<usize, HidError>,
    expected_bytes: usize,
    _policy: WriteStallPolicy,
) -> bool {
    matches!(result, Ok(written) if *written == expected_bytes)
}
