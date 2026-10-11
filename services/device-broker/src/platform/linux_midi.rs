//! Performance MIDI on Linux, bound to the physical USB device selected over HID.
//!
//! The ALSA raw-MIDI sysfs device is accepted only when its physical USB
//! ancestor is the same QC USB device as the freshly resolved selected hidraw
//! identity. Open descriptors are checked against the current node generation
//! and USB ancestry before every write.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MIDI_IO_BUDGET: Duration = Duration::from_millis(100);
const MIDI_RETRY_POLL_MS: i32 = 10;

#[derive(Default)]
pub struct PerformanceMidi {
    device_id: Option<String>,
    last_sent: Option<Instant>,
    endpoint: Option<RawMidiEndpoint>,
}

pub struct PerformanceMidiReceipt {
    pub endpoint: String,
    pub dispatch_latency_ms: f64,
    pub throttle_delay_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PhysicalUsb {
    path: PathBuf,
    vendor_id: u16,
    product_id: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeGeneration {
    device: u64,
    inode: u64,
    rdev: u64,
    physical: PhysicalUsb,
}

struct Candidate {
    name: String,
    path: PathBuf,
    sysfs_device: PathBuf,
    generation: NodeGeneration,
}

struct RawMidiEndpoint {
    file: File,
    candidate: Candidate,
}

impl PerformanceMidi {
    pub fn send_for_device(
        &mut self,
        device_id: &str,
        controller: u8,
        value: u8,
    ) -> Result<PerformanceMidiReceipt, String> {
        let queued_at = Instant::now();
        let mut throttle_delay = Duration::ZERO;
        if let Some(last_sent) = self.last_sent {
            let gap = Duration::from_millis(quadcortex::profile::PERFORMANCE_MIDI_GAP_MS);
            let elapsed = last_sent.elapsed();
            if elapsed < gap {
                throttle_delay = gap - elapsed;
                std::thread::sleep(throttle_delay);
            }
        }

        self.warm_up_for_device(device_id)?;
        let endpoint = self
            .endpoint
            .as_mut()
            .ok_or_else(|| "QC performance MIDI endpoint was not opened".to_owned())?;
        if let Err(error) = validate_open_endpoint(device_id, endpoint) {
            self.endpoint.take();
            self.device_id = None;
            return Err(error);
        }

        let message = [
            quadcortex::profile::MIDI_CONTROL_CHANGE_STATUS as u8,
            controller,
            value,
        ];
        if let Err(error) = write_bounded(&endpoint.file, &message) {
            self.endpoint.take();
            self.device_id = None;
            return Err(error);
        }
        self.last_sent = Some(Instant::now());
        Ok(PerformanceMidiReceipt {
            endpoint: endpoint.candidate.name.clone(),
            dispatch_latency_ms: queued_at.elapsed().as_secs_f64() * 1000.0,
            throttle_delay_ms: throttle_delay.as_secs_f64() * 1000.0,
        })
    }

    pub fn warm_up_for_device(&mut self, device_id: &str) -> Result<String, String> {
        if self.device_id.as_deref() != Some(device_id) || self.endpoint.is_none() {
            self.endpoint = None;
            self.device_id = None;
            let hidraw = selected_hidraw_path(device_id)?;
            let target = physical_usb_for_hid(&hidraw, Path::new("/sys"))?;
            let endpoint =
                open_matching_endpoint(&target, Path::new("/sys"), Path::new("/dev/snd"))?;
            let name = endpoint.candidate.name.clone();
            self.endpoint = Some(endpoint);
            self.device_id = Some(device_id.to_owned());
            return Ok(name);
        }

        let endpoint = self
            .endpoint
            .as_mut()
            .ok_or_else(|| "QC performance MIDI endpoint was not opened".to_owned())?;
        if let Err(error) = validate_open_endpoint(device_id, endpoint) {
            self.endpoint.take();
            self.device_id = None;
            return Err(error);
        }
        Ok(endpoint.candidate.name.clone())
    }
}

fn selected_hidraw_path(device_id: &str) -> Result<PathBuf, String> {
    quadcortex_hid::usb::linux_hidraw_path_for_device(device_id)
        .map_err(|error| format!("Resolve selected Quad Cortex HID interface: {error}"))
}

fn physical_usb_for_hid(hidraw_path: &Path, sysfs_root: &Path) -> Result<PhysicalUsb, String> {
    let node = hidraw_path
        .file_name()
        .ok_or_else(|| "Selected hidraw path has no node name".to_owned())?;
    let class_device = sysfs_root.join("class/hidraw").join(node).join("device");
    physical_usb_ancestor(&class_device, sysfs_root)
}

fn physical_usb_ancestor(device_link: &Path, sysfs_root: &Path) -> Result<PhysicalUsb, String> {
    let root = fs::canonicalize(sysfs_root)
        .map_err(|error| format!("Resolve sysfs root {}: {error}", sysfs_root.display()))?;
    let start = fs::canonicalize(device_link)
        .map_err(|error| format!("Resolve USB ancestry {}: {error}", device_link.display()))?;
    if !start.starts_with(&root) {
        return Err("USB ancestry resolved outside sysfs".to_owned());
    }
    for ancestor in start.ancestors().take_while(|path| path.starts_with(&root)) {
        let vendor = read_hex_id(&ancestor.join("idVendor"))?;
        let product = read_hex_id(&ancestor.join("idProduct"))?;
        match (vendor, product) {
            (Some(vendor_id), Some(product_id)) => {
                if vendor_id != quadcortex::profile::VENDOR_ID
                    || !quadcortex::profile::supports_product_id(product_id)
                {
                    return Err(format!(
                        "Selected HID ancestry is not a supported Quad Cortex USB device ({vendor_id:04x}:{product_id:04x})"
                    ));
                }
                return Ok(PhysicalUsb {
                    path: ancestor.to_path_buf(),
                    vendor_id,
                    product_id,
                });
            }
            (None, None) => {}
            _ => return Err("Incomplete USB vendor/product ancestry identity".to_owned()),
        }
    }
    Err("Selected hidraw interface has no physical Quad Cortex USB ancestor".to_owned())
}

fn read_hex_id(path: &Path) -> Result<Option<u16>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Read USB ancestry {}: {error}", path.display())),
    };
    let text = text.trim();
    if text.len() != 4 {
        return Err(format!("Malformed USB ancestry ID in {}", path.display()));
    }
    u16::from_str_radix(text, 16)
        .map(Some)
        .map_err(|error| format!("Malformed USB ancestry ID in {}: {error}", path.display()))
}

fn is_raw_midi_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("midiC") else {
        return false;
    };
    let Some((card, device)) = rest.split_once('D') else {
        return false;
    };
    !card.is_empty()
        && !device.is_empty()
        && card.bytes().all(|byte| byte.is_ascii_digit())
        && device.bytes().all(|byte| byte.is_ascii_digit())
}

fn discover_candidates(
    target: &PhysicalUsb,
    sysfs_root: &Path,
    dev_root: &Path,
) -> Result<Vec<Candidate>, String> {
    let sound = sysfs_root.join("class/sound");
    let entries = fs::read_dir(&sound).map_err(|error| {
        format!(
            "Enumerate ALSA raw MIDI devices {}: {error}",
            sound.display()
        )
    })?;
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("Read ALSA device entry: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_raw_midi_name(&name) {
            continue;
        }
        let sysfs_device_link = entry.path().join("device");
        let physical = match physical_usb_ancestor(&sysfs_device_link, sysfs_root) {
            Ok(physical) => physical,
            Err(_) => continue,
        };
        if &physical != target {
            continue;
        }
        let path = dev_root.join(&name);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("Inspect ALSA raw MIDI node {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_char_device() {
            return Err(format!(
                "ALSA raw MIDI endpoint is not a character device: {}",
                path.display()
            ));
        }
        matches.push(Candidate {
            name,
            path,
            sysfs_device: fs::canonicalize(&sysfs_device_link).map_err(|error| {
                format!(
                    "Resolve ALSA raw MIDI ancestry {}: {error}",
                    sysfs_device_link.display()
                )
            })?,
            generation: NodeGeneration {
                device: metadata.dev(),
                inode: metadata.ino(),
                rdev: metadata.rdev(),
                physical,
            },
        });
    }
    Ok(matches)
}

fn open_matching_endpoint(
    target: &PhysicalUsb,
    sysfs_root: &Path,
    dev_root: &Path,
) -> Result<RawMidiEndpoint, String> {
    let mut candidates = discover_candidates(target, sysfs_root, dev_root)?;
    if candidates.len() != 1 {
        return Err(if candidates.is_empty() {
            "No ALSA raw MIDI endpoint maps to the selected Quad Cortex USB device".to_owned()
        } else {
            "Multiple ALSA raw MIDI endpoints map to the selected Quad Cortex USB device; refusing ambiguous output".to_owned()
        });
    }
    let candidate = candidates.remove(0);
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(&candidate.path)
        .map_err(|error| format!("Open selected ALSA raw MIDI endpoint: {error}"))?;
    verify_open_node(&file, &candidate)?;
    let current = discover_candidates(target, sysfs_root, dev_root)?;
    if current.len() != 1 || !same_candidate(&candidate, &current[0]) {
        return Err("ALSA raw MIDI endpoint changed while opening; refusing to send".to_owned());
    }
    Ok(RawMidiEndpoint { file, candidate })
}

fn same_candidate(left: &Candidate, right: &Candidate) -> bool {
    left.path == right.path
        && left.sysfs_device == right.sysfs_device
        && left.generation == right.generation
}

fn verify_open_node(file: &File, candidate: &Candidate) -> Result<(), String> {
    let held = file
        .metadata()
        .map_err(|error| format!("Inspect open ALSA MIDI endpoint: {error}"))?;
    let current = fs::symlink_metadata(&candidate.path).map_err(|error| {
        format!(
            "Recheck ALSA MIDI node {}: {error}",
            candidate.path.display()
        )
    })?;
    if !held.file_type().is_char_device()
        || current.file_type().is_symlink()
        || !current.file_type().is_char_device()
        || !same_node_generation(&held, &current, &candidate.generation)
    {
        return Err("ALSA raw MIDI character node was replaced".to_owned());
    }
    Ok(())
}

fn same_node_generation(
    held: &fs::Metadata,
    current: &fs::Metadata,
    expected: &NodeGeneration,
) -> bool {
    (held.dev(), held.ino(), held.rdev()) == (current.dev(), current.ino(), current.rdev())
        && (held.dev(), held.ino(), held.rdev()) == (expected.device, expected.inode, expected.rdev)
}

fn validate_open_endpoint(device_id: &str, endpoint: &RawMidiEndpoint) -> Result<(), String> {
    let hidraw = selected_hidraw_path(device_id)?;
    let physical = physical_usb_for_hid(&hidraw, Path::new("/sys"))?;
    if physical != endpoint.candidate.generation.physical {
        return Err("Selected Quad Cortex USB device changed; refusing stale MIDI endpoint".into());
    }
    verify_open_node(&endpoint.file, &endpoint.candidate)?;
    let current = discover_candidates(&physical, Path::new("/sys"), Path::new("/dev/snd"))?;
    if current.len() != 1 || !same_candidate(&endpoint.candidate, &current[0]) {
        return Err("Selected ALSA raw MIDI endpoint is stale or ambiguous".into());
    }
    Ok(())
}

fn write_bounded(file: &File, message: &[u8]) -> Result<(), String> {
    write_bounded_with(
        message,
        |bytes| write_once(file, bytes),
        |remaining| wait_writable(file, remaining),
    )
}

fn write_bounded_with(
    message: &[u8],
    mut write: impl FnMut(&[u8]) -> io::Result<usize>,
    mut wait: impl FnMut(Duration) -> Result<(), String>,
) -> Result<(), String> {
    let deadline = Instant::now() + MIDI_IO_BUDGET;
    let mut written = 0;
    while written < message.len() {
        if Instant::now() >= deadline {
            return Err(delivery_failure(
                written,
                "raw MIDI write exceeded its time budget",
            ));
        }
        match write(&message[written..]) {
            Ok(0) => {
                return Err(delivery_failure(
                    written,
                    "raw MIDI write returned zero bytes",
                ))
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(delivery_failure(written, "raw MIDI output remained busy"));
                }
                if let Err(error) = wait(remaining) {
                    return Err(delivery_failure(written, &error));
                }
            }
            Err(error) => return Err(delivery_failure(written, &error.to_string())),
        }
        if Instant::now() >= deadline && written < message.len() {
            return Err(delivery_failure(
                written,
                "raw MIDI write exceeded its time budget",
            ));
        }
    }
    Ok(())
}

fn write_once(file: &File, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: `bytes` is a live readable buffer, and the descriptor is owned by
    // `file` for this call. libc::write does not retain either pointer.
    let count = unsafe { libc::write(file.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    if count < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(count as usize)
    }
}

fn wait_writable(file: &File, remaining: Duration) -> Result<(), String> {
    let mut descriptor = libc::pollfd {
        fd: file.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    let timeout = remaining.as_millis().clamp(1, MIDI_RETRY_POLL_MS as u128) as i32;
    // SAFETY: descriptor points to one initialized pollfd and its count is one.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(());
        }
        return Err(format!("poll raw MIDI output: {error}"));
    }
    if result > 0 && descriptor.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        return Err("raw MIDI endpoint disconnected while waiting".to_owned());
    }
    Ok(())
}

fn delivery_failure(written: usize, reason: &str) -> String {
    if written == 0 {
        format!("Could not send QC performance MIDI command: {reason}")
    } else {
        format!(
            "QC performance MIDI delivery is unknown: wrote {written} of 3 bytes before {reason}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn midi_node_names_accept_only_raw_midi_class_entries() {
        assert!(is_raw_midi_name("midiC0D0"));
        assert!(is_raw_midi_name("midiC12D3"));
        for name in [
            "midiC0",
            "midiC0D",
            "midiX0D0",
            "midiC0D0extra",
            "controlC0",
        ] {
            assert!(!is_raw_midi_name(name), "{name}");
        }
    }

    #[test]
    fn maps_only_the_same_physical_usb_ancestor_and_rejects_ambiguity() {
        let fixture = SysfsFixture::new();
        fixture.add_usb("usb/1-1", 0x152a, 0x880a);
        fixture.add_hidraw("hidraw0", "usb/1-1/1-1:1.5/hidraw/hidraw0");
        fixture.add_midi("midiC0D0", "usb/1-1/1-1:1.2/sound/card0/midiC0D0");
        fixture.add_usb("usb/1-2", 0x152a, 0x880a);
        fixture.add_midi("midiC1D0", "usb/1-2/1-2:1.2/sound/card1/midiC1D0");
        let target = physical_usb_for_hid(&fixture.dev.join("hidraw0"), &fixture.sys).unwrap();
        let matching = discover_sysfs_midi(&target, &fixture.sys).unwrap();
        assert_eq!(matching, vec!["midiC0D0"]);
        fixture.add_midi("midiC2D0", "usb/1-1/1-1:1.2/sound/card2/midiC2D0");
        assert!(discover_sysfs_midi(&target, &fixture.sys)
            .unwrap_err()
            .contains("ambiguous"));
    }

    #[test]
    fn rejects_wrong_usb_vid_pid_and_missing_ancestry() {
        let fixture = SysfsFixture::new();
        fixture.add_usb("usb/1-1", 0x1234, 0xabcd);
        fixture.add_hidraw("hidraw0", "usb/1-1/1-1:1.5/hidraw/hidraw0");
        assert!(
            physical_usb_for_hid(&fixture.dev.join("hidraw0"), &fixture.sys)
                .unwrap_err()
                .contains("not a supported Quad Cortex")
        );
        let loose = fixture.sys.join("devices/pci/1-1:1.5/hidraw/hidraw9");
        fs::create_dir_all(&loose).unwrap();
        let class_link = fixture.sys.join("class/hidraw/hidraw9");
        fs::create_dir_all(&class_link).unwrap();
        symlink(&loose, class_link.join("device")).unwrap();
        assert!(
            physical_usb_for_hid(&fixture.dev.join("hidraw9"), &fixture.sys)
                .unwrap_err()
                .contains("no physical Quad Cortex")
        );
    }

    #[test]
    fn stale_character_node_generation_is_rejected_before_write() {
        let fixture = SysfsFixture::new();
        let node = fixture.dev.join("midiC0D0");
        fs::write(&node, b"old").unwrap();
        let candidate = Candidate {
            name: "midiC0D0".into(),
            path: node.clone(),
            sysfs_device: fixture.sys.clone(),
            generation: NodeGeneration {
                device: 0,
                inode: 0,
                rdev: 0,
                physical: PhysicalUsb {
                    path: fixture.sys.clone(),
                    vendor_id: 0x152a,
                    product_id: 0x880a,
                },
            },
        };
        let file = OpenOptions::new().write(true).open(&node).unwrap();
        assert!(verify_open_node(&file, &candidate).is_err());
    }

    #[test]
    fn replacing_the_path_does_not_change_the_generation_held_by_an_open_node() {
        let fixture = SysfsFixture::new();
        let node = fixture.dev.join("midiC0D0");
        fs::write(&node, b"old").unwrap();
        let file = OpenOptions::new().write(true).open(&node).unwrap();
        let opened = file.metadata().unwrap();
        let expected = NodeGeneration {
            device: opened.dev(),
            inode: opened.ino(),
            rdev: opened.rdev(),
            physical: PhysicalUsb {
                path: fixture.sys.clone(),
                vendor_id: 0x152a,
                product_id: 0x880a,
            },
        };
        fs::remove_file(&node).unwrap();
        fs::write(&node, b"successor").unwrap();
        let successor = fs::metadata(&node).unwrap();
        assert!(!same_node_generation(&opened, &successor, &expected));
        assert_eq!(fs::read(&node).unwrap(), b"successor");
    }

    #[test]
    fn bounded_writer_reports_partial_delivery_and_timeout() {
        let mut first = true;
        let error = write_bounded_with(
            &[0xb0, 1, 2],
            |_| {
                if first {
                    first = false;
                    Ok(1)
                } else {
                    Err(io::ErrorKind::WouldBlock.into())
                }
            },
            |_| Err("injected EAGAIN timeout".into()),
        )
        .unwrap_err();
        assert!(error.contains("delivery is unknown"));
        assert!(error.contains("wrote 1 of 3 bytes"));

        let error = write_bounded_with(
            &[0xb0, 1, 2],
            |_| Err(io::ErrorKind::BrokenPipe.into()),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("Could not send"));

        let began = Instant::now();
        let error = write_bounded_with(
            &[0xb0, 1, 2],
            |_| Err(io::ErrorKind::WouldBlock.into()),
            |remaining| {
                std::thread::sleep(remaining.min(Duration::from_millis(5)));
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("time budget"));
        assert!(began.elapsed() <= MIDI_IO_BUDGET + Duration::from_millis(30));
    }

    struct SysfsFixture {
        root: PathBuf,
        sys: PathBuf,
        dev: PathBuf,
    }

    impl SysfsFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("qc-linux-midi-{}", uuid::Uuid::new_v4()));
            let sys = root.join("sys");
            let dev = root.join("dev");
            fs::create_dir_all(sys.join("class/hidraw")).unwrap();
            fs::create_dir_all(sys.join("class/sound")).unwrap();
            fs::create_dir_all(&dev).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self { root, sys, dev }
        }

        fn add_usb(&self, name: &str, vendor: u16, product: u16) {
            let path = self.sys.join("devices").join(name);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("idVendor"), format!("{vendor:04x}\n")).unwrap();
            fs::write(path.join("idProduct"), format!("{product:04x}\n")).unwrap();
        }

        fn add_hidraw(&self, name: &str, relative_device: &str) {
            let path = self.sys.join("devices").join(relative_device);
            fs::create_dir_all(&path).unwrap();
            let class = self.sys.join("class/hidraw").join(name);
            fs::create_dir_all(&class).unwrap();
            symlink(&path, class.join("device")).unwrap();
            let node = self.dev.join(name);
            fs::write(node, b"").unwrap();
        }

        fn add_midi(&self, name: &str, relative_device: &str) {
            let path = self.sys.join("devices").join(relative_device);
            fs::create_dir_all(&path).unwrap();
            let class = self.sys.join("class/sound").join(name);
            fs::create_dir_all(&class).unwrap();
            symlink(&path, class.join("device")).unwrap();
            fs::write(self.dev.join(name), b"").unwrap();
        }
    }

    impl Drop for SysfsFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    use std::os::unix::fs::symlink;
}

#[cfg(test)]
fn discover_sysfs_midi(target: &PhysicalUsb, sysfs_root: &Path) -> Result<Vec<String>, String> {
    let sound = sysfs_root.join("class/sound");
    let mut matches = Vec::new();
    for entry in fs::read_dir(&sound).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_raw_midi_name(&name)
            && physical_usb_ancestor(&entry.path().join("device"), sysfs_root).as_ref()
                == Ok(target)
        {
            matches.push(name);
        }
    }
    if matches.len() > 1 {
        return Err(
            "Multiple ALSA raw MIDI endpoints map to the selected USB device (ambiguous)".into(),
        );
    }
    Ok(matches)
}
