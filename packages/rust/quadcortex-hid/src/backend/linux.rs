use crate::usb::UsbError;
use hidapi::{HidApi, HidDevice, HidError};
use std::cell::UnsafeCell;
use std::ffi::{CStr, OsStr};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(crate) fn device_id(device: &hidapi::DeviceInfo) -> String {
    let serial = device.serial_number().filter(|serial| !serial.is_empty());
    let generation = if serial.is_none() {
        std::fs::metadata(OsStr::from_bytes(device.path().to_bytes()))
            .ok()
            .map(|metadata| {
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            })
    } else {
        None
    };
    identity(
        device.vendor_id(),
        device.product_id(),
        device.path(),
        serial,
        generation,
    )
}

fn identity(
    vendor: u16,
    product: u16,
    path: &CStr,
    serial: Option<&str>,
    generation: Option<(u64, u64, i64, i64)>,
) -> String {
    let prefix = format!("linux:{vendor:04x}:{product:04x}");
    if let Some(serial) = serial {
        let serial = serial
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        return format!("{prefix}:serial:{serial}");
    }
    // hidraw minor numbers are reusable. A unit with no advertised serial
    // receives a conservative node-generation identity, so a replacement at
    // the same path cannot silently inherit an explicit selection. Such a unit
    // must be selected again after unplug/replug; do not guess its identity.
    let path = crate::usb::device_path_id(path);
    match generation {
        Some((device, inode, seconds, nanos)) => {
            format!("{prefix}:node:{path}:{device}:{inode}:{seconds}:{nanos}")
        }
        None => format!("{prefix}:unavailable:{path}"),
    }
}

/// One hidraw handle, with an advisory inode lock retained through both lanes.
///
/// hidraw allows multiple opens. The broker singleton protects this user's
/// process; this lock also excludes cooperating transports across users and
/// keeps a reconnect from replacing a handle whose writer has not exited.
/// It cannot exclude applications that ignore flock, so other QC control
/// applications must be closed before connecting.
pub(crate) struct SharedLinuxHid {
    // Close HID before releasing ownership. Both fields outlive every lane
    // because the engine holds this object in an Arc.
    device: UnsafeCell<HidDevice>,
    _ownership: File,
}

// SAFETY: audited against hidapi 2.6.7's linux-native backend at upstream
// 8729463f8a96b6d4671b7ef114b5ead65ccd7469/src/linux_native.rs. Its write and
// read_timeout use only the immutable OwnedFd and call-local buffers/pollfds.
// Neither accesses blocking: Cell<bool> or info: RefCell<Option<DeviceInfo>>.
// This wrapper exposes ONLY those two calls: the engine has one RX caller and
// one TX caller, never concurrent calls within a lane, and an Arc retains both
// the HID handle and its ownership lock until those calls return. Do not add
// accessors or enable another Linux hidapi backend without re-auditing this.
unsafe impl Sync for SharedLinuxHid {}

impl SharedLinuxHid {
    pub(crate) fn open(
        api: &HidApi,
        path: &CStr,
        selected_identity: &str,
    ) -> Result<Self, UsbError> {
        let path_name = OsStr::from_bytes(path.to_bytes());
        let ownership = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(path_name)
            .map_err(|error| {
                UsbError::Hid(format!(
                    "cannot open the Quad Cortex hidraw interface: {error}; check the Linux udev permissions"
                ))
            })?;
        if !ownership
            .metadata()
            .map_err(|error| UsbError::Hid(error.to_string()))?
            .file_type()
            .is_char_device()
        {
            return Err(UsbError::Hid(
                "the selected interface is not a hidraw character device".into(),
            ));
        }
        claim(&ownership).map_err(|error| {
            UsbError::Hid(format!(
                "the Quad Cortex hidraw interface is already owned or cannot be locked: {error}"
            ))
        })?;
        let device = api.open_path(path).map_err(|error| {
            UsbError::Hid(format!(
                "cannot open the Quad Cortex HID interface: {error}"
            ))
        })?;
        // Enumeration and opening are separate syscalls. Re-check the native
        // handle before any session writes; a reused hidraw minor must not
        // replace an explicitly selected serial or node generation.
        let opened_identity = device
            .get_device_info()
            .map(|opened| device_id(&opened))
            .map_err(|error| {
                UsbError::Hid(format!("cannot verify the opened HID identity: {error}"))
            })?;
        if opened_identity != selected_identity {
            return Err(UsbError::NotAvailable);
        }
        // The lock and hidapi handles open separately. Even a reconnect with
        // the same advertised serial must not pair a new node's HID handle
        // with a flock held on the previous (now unlinked) node's inode.
        ensure_current_node(&ownership, Path::new(path_name)).map_err(|error| {
            UsbError::Hid(format!("HID ownership changed during open: {error}"))
        })?;
        Ok(Self {
            device: UnsafeCell::new(device),
            _ownership: ownership,
        })
    }

    pub(crate) fn read_timeout(&self, report: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        // SAFETY: the audited RX method only reads the fd; see Sync above.
        unsafe { (&*self.device.get()).read_timeout(report, timeout_ms) }
            .map_err(|error| error.to_string())
    }

    pub(crate) fn write(&self, report: &[u8]) -> Result<usize, HidError> {
        // SAFETY: the audited TX method only reads the fd; see Sync above.
        unsafe { (&*self.device.get()).write(report) }
    }
}

fn ensure_current_node(ownership: &File, path: &Path) -> std::io::Result<()> {
    let held = ownership.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    if current.file_type().is_symlink()
        || (held.dev(), held.ino()) != (current.dev(), current.ino())
    {
        return Err(std::io::Error::other("the locked HID node was replaced"));
    }
    Ok(())
}

fn claim(file: &File) -> std::io::Result<()> {
    // SAFETY: file owns a live descriptor for the duration of this syscall.
    // flock changes kernel ownership only and does not access Rust memory.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Linux errno values are not Windows status codes. Until hardware evidence
/// establishes a Linux-specific exception, only a complete write is delivered.
pub(crate) fn write_report_delivered(
    result: &Result<usize, HidError>,
    expected_bytes: usize,
) -> bool {
    matches!(result, Ok(written) if *written == expected_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("qc-hid-lock-{}", uuid::Uuid::new_v4())))
        }
        fn open(&self) -> File {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&self.0)
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn ownership_refuses_a_second_open_and_is_released_on_failure_cleanup() {
        let fixture = Fixture::new();
        let competitor = fixture.open();
        let attempt = (|| -> std::io::Result<()> {
            let owner = fixture.open();
            claim(&owner)?;
            assert!(
                claim(&competitor).is_err(),
                "two independent opens cannot both own HID"
            );
            Err(std::io::Error::other("injected HID open failure"))
        })();
        assert!(attempt.is_err());
        claim(&competitor).expect("failure dropped the ownership file");
    }

    #[test]
    fn ownership_survives_until_the_last_io_lane_releases_it() {
        let fixture = Fixture::new();
        let owner = std::sync::Arc::new(fixture.open());
        claim(&owner).unwrap();
        let writer = std::sync::Arc::clone(&owner);
        let competitor = fixture.open();
        drop(owner);
        assert!(
            claim(&competitor).is_err(),
            "a retained writer still owns HID"
        );
        drop(writer);
        claim(&competitor).unwrap();
    }

    #[test]
    fn replacing_a_node_between_lock_and_native_open_invalidates_ownership() {
        let fixture = Fixture::new();
        let ownership = fixture.open();
        claim(&ownership).unwrap();
        ensure_current_node(&ownership, &fixture.0).unwrap();
        std::fs::remove_file(&fixture.0).unwrap();
        let successor = fixture.open();
        claim(&successor).expect("a new inode is not protected by the old flock");
        assert!(ensure_current_node(&ownership, &fixture.0).is_err());
        ensure_current_node(&successor, &fixture.0).unwrap();
    }

    #[test]
    fn linux_requires_full_write_and_never_treats_errno_as_windows_status() {
        assert!(write_report_delivered(&Ok(129), 129));
        assert!(!write_report_delivered(&Ok(128), 129));
        for code in [libc::EPIPE, libc::EIO, libc::ENODEV, libc::EACCES, 31] {
            assert!(!write_report_delivered(
                &Err(HidError::IoError {
                    error: std::io::Error::from_raw_os_error(code),
                }),
                129
            ));
        }
    }

    #[test]
    fn serial_identity_survives_node_renumbering_and_rejects_another_unit() {
        let first = std::ffi::CString::new("/dev/hidraw1").unwrap();
        let next = std::ffi::CString::new("/dev/hidraw7").unwrap();
        let id = identity(0x2f21, 0x880a, &first, Some("unit-A"), None);
        assert_eq!(id, identity(0x2f21, 0x880a, &next, Some("unit-A"), None));
        assert_ne!(id, identity(0x2f21, 0x880a, &first, Some("unit-B"), None));
        assert_ne!(id, identity(0x2f21, 0x892f, &first, Some("unit-A"), None));
    }

    #[test]
    fn serialless_identity_cannot_follow_a_reused_hidraw_path() {
        let path = std::ffi::CString::new("/dev/hidraw1").unwrap();
        let id = identity(0x2f21, 0x880a, &path, None, Some((1, 2, 3, 4)));
        assert_ne!(
            id,
            identity(0x2f21, 0x880a, &path, None, Some((1, 5, 3, 4)))
        );
        assert_ne!(
            id,
            identity(0x2f21, 0x880a, &path, None, Some((1, 2, 8, 4)))
        );
        assert_ne!(id, identity(0x2f21, 0x880a, &path, None, None));
    }
}
