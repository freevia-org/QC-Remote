use super::WriteStallPolicy;
use crate::usb::UsbError;
use hidapi::{HidApi, HidDevice, HidError};
use std::ffi::CStr;
use std::sync::{mpsc, Mutex, OnceLock};

type HidJob = Box<dyn FnOnce() + Send + 'static>;
static HID_THREAD: OnceLock<Result<mpsc::Sender<HidJob>, String>> = OnceLock::new();

/// Run every hidapi context operation on one process-lifetime thread. hidapi's
/// macOS backend schedules its process-global IOHIDManager on the current
/// CFRunLoop, so moving later discovery calls between short-lived broker
/// workers can crash inside CoreFoundation on Intel Macs.
pub(crate) fn with_hid_thread<T>(
    operation: impl FnOnce() -> Result<T, UsbError> + Send + 'static,
) -> Result<T, UsbError>
where
    T: Send + 'static,
{
    let sender = HID_THREAD.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<HidJob>();
        std::thread::Builder::new()
            .name("qc-hidapi-macos".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            })
            .map_err(|error| format!("Could not start the macOS HID thread: {error}"))?;
        Ok(sender)
    });
    let sender = sender
        .as_ref()
        .map_err(|error| UsbError::Hid(error.clone()))?;
    let (response, result) = mpsc::sync_channel(1);
    sender
        .send(Box::new(move || {
            let _ = response.send(operation());
        }))
        .map_err(|_| UsbError::Hid("The macOS HID thread stopped".into()))?;
    result
        .recv()
        .map_err(|_| UsbError::Hid("The macOS HID thread stopped before replying".into()))?
}

pub(crate) fn device_id(device: &hidapi::DeviceInfo) -> String {
    let serial = device.serial_number().filter(|serial| !serial.is_empty());
    crate::usb::macos_device_identity(device.path(), serial)
}

/// macOS hidapi calls are serialized through a mutex. In particular, this
/// avoids relying on undocumented concurrent access to the CoreHID handle.
/// `read_timeout` bounds the time spent waiting for input once it owns the
/// mutex; it does not bound time waiting to acquire the mutex. A write holds
/// the same mutex during synchronous `IOHIDDeviceSetReport`, which has no
/// timeout, so reads or writes queued behind a stalled native write may wait
/// longer than one read interval.
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
