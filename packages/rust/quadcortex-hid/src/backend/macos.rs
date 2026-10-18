use super::WriteStallPolicy;
use crate::usb::UsbError;
use hidapi::{HidApi, HidDevice, HidError};
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
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
pub(crate) struct SharedMacosHid {
    device: Mutex<HidDevice>,
    writer_waiting: AtomicBool,
    write_stall_policy: WriteStallPolicy,
}

const QC_880A_INTERFACE_5_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x00, 0xa1, 0x01, 0x15, 0x00, 0x26, 0xff, 0x00, 0x85, 0x01, 0x75, 0x08, 0x95,
    0x80, 0x09, 0x00, 0x81, 0x82, 0x85, 0x02, 0x75, 0x08, 0x95, 0x80, 0x09, 0x00, 0x91, 0x82, 0xc0,
];

fn qc_control_stall_profile(vendor: u16, product: u16, interface: i32, descriptor: &[u8]) -> bool {
    vendor == 0x152a
        && product == 0x880a
        && interface == 5
        && descriptor == QC_880A_INTERFACE_5_REPORT_DESCRIPTOR
}

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
        let mut descriptor = [0u8; 33];
        let measured_profile = device
            .get_report_descriptor(&mut descriptor)
            .ok()
            .is_some_and(|n| {
                n <= descriptor.len()
                    && qc_control_stall_profile(
                        opened_info.vendor_id(),
                        opened_info.product_id(),
                        opened_info.interface_number(),
                        &descriptor[..n],
                    )
            });
        Ok(Self {
            device: Mutex::new(device),
            writer_waiting: AtomicBool::new(false),
            write_stall_policy: if measured_profile {
                WriteStallPolicy::MacosQc880aControlStatusStall
            } else {
                WriteStallPolicy::MacosSerialized
            },
        })
    }

    pub(crate) fn read_timeout(&self, report: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        // std::sync::Mutex is not fair. A polling reader can otherwise retake
        // the lock indefinitely while a queued handshake write expires.
        if self.writer_waiting.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(1));
            return Ok(0);
        }
        self.device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .read_timeout(report, timeout_ms)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn write(&self, report: &[u8]) -> Result<usize, HidError> {
        self.writer_waiting.store(true, Ordering::Release);
        let result = self
            .device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write(report);
        self.writer_waiting.store(false, Ordering::Release);
        result
    }

    pub(crate) fn write_stall_policy(&self) -> WriteStallPolicy {
        self.write_stall_policy
    }
}

pub(crate) fn write_report_delivered(
    result: &Result<usize, HidError>,
    expected_bytes: usize,
    policy: WriteStallPolicy,
) -> bool {
    match result {
        Ok(written) => *written == expected_bytes,
        // Measured on macOS with the exact profile above: a complete 129-byte
        // ResetCommsBuffers write returns kUSBHostReturnPipeStalled, followed
        // by a reply echoing the fresh session UUID. This is a native status
        // exception, not semantic command verification. Other errors fail closed.
        Err(HidError::HidApiError { message }) => {
            policy == WriteStallPolicy::MacosQc880aControlStatusStall
                && expected_bytes == quadcortex::framing::REPORT_SIZE
                && message.starts_with("IOHIDDeviceSetReport failed: (0xE0005000) ")
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_stall_is_limited_to_the_measured_device_and_descriptor() {
        let d = QC_880A_INTERFACE_5_REPORT_DESCRIPTOR;
        assert!(qc_control_stall_profile(0x152a, 0x880a, 5, d));
        assert!(!qc_control_stall_profile(0x152a, 0x892f, 5, d));
        assert!(!qc_control_stall_profile(0x152b, 0x880a, 5, d));
        assert!(!qc_control_stall_profile(0x152a, 0x880a, 4, d));
        assert!(!qc_control_stall_profile(0x152a, 0x880a, 5, &d[..31]));
    }

    #[test]
    fn only_the_exact_native_stall_on_a_complete_report_is_expected() {
        let stall = Err(HidError::HidApiError {
            message: "IOHIDDeviceSetReport failed: (0xE0005000) unknown error code".into(),
        });
        let policy = WriteStallPolicy::MacosQc880aControlStatusStall;
        assert!(write_report_delivered(&stall, 129, policy));
        assert!(!write_report_delivered(&stall, 128, policy));
        assert!(!write_report_delivered(
            &stall,
            129,
            WriteStallPolicy::MacosSerialized
        ));
        assert!(!write_report_delivered(&Ok(128), 129, policy));
        assert!(write_report_delivered(&Ok(129), 129, policy));
        let other = Err(HidError::HidApiError {
            message: "IOHIDDeviceSetReport failed: (0xE00002C0) error".into(),
        });
        assert!(!write_report_delivered(&other, 129, policy));
    }
}
