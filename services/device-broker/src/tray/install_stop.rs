//! An installer's request that this installation's broker step aside.
//!
//! The broker may hold the device for minutes after its last client, and since
//! 0.1.178 the same process is also the session's background MCP host, so it
//! would otherwise outlive every upgrade. The installer runs
//! `qc-device-broker.exe --stop-for-install` (see [`request_stop`]), which
//! signals a named event scoped to the broker's path. The broker stops its
//! in-process MCP host first, then honours the request only with no other
//! participant attached; with one it keeps serving, and the installer's unlock
//! check reports the broker in use.
//!
//! The event name is unchanged from 0.1.176, so an old installation's
//! `qc-remote-tray.exe --stop-for-install` stops a new broker at the same path,
//! and this helper stops an old one.
//!
//! The installer must run this helper only from a binary known to understand
//! it: a broker from before 0.1.178 treats an unknown argument as its default
//! live probe and would open the USB device. The NSIS hooks therefore use an
//! old installation's `qc-remote-tray.exe` whenever one is present.
use std::path::Path;

const PREFIX: &str = "Local\\QCRemote.DeviceBroker.InstallStop.";

/// Stable FNV-1a over the case-folded UTF-16 path, independent of Rust's
/// randomized hashing. `apps/windows/src-tauri/src/tray_native.rs` computes
/// the same name for its sibling broker; both pin the same test vector.
pub fn event_name(broker_executable: &Path) -> String {
    let hash = broker_executable
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!("{PREFIX}{hash:016x}")
}

/// Waits on this installation's stop event for the life of the process and
/// calls `on_request` each time it is signalled. Returns once the event
/// exists, so a signal sent after this returns cannot be missed.
#[cfg(windows)]
pub fn watch(on_request: impl Fn() + Send + 'static) -> Result<(), String> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::CreateEventW;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let name: Vec<u16> = event_name(&executable)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // Auto-reset: a refused request is consumed, never left armed to fire
    // later when the last participant happens to leave.
    // SAFETY: default security, a terminated name read only during the call.
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if event.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // Acquire ownership before constructing/spawning the watcher. A rejected
    // spawn drops its closure; callback unwind drops the same owned handle.
    let event = unsafe { OwnedHandle::from_raw_handle(event.cast()) };
    watch_event_with(event, on_request, |task| {
        std::thread::Builder::new()
            .name("qc-broker-install-stop".into())
            .spawn(task)
            .map(|_| ())
    })
}

#[cfg(windows)]
fn watch_event_with(
    event: std::os::windows::io::OwnedHandle,
    on_request: impl Fn() + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    spawn(Box::new(move || loop {
        // SAFETY: the handle is owned by this thread for the process life.
        let waited = unsafe { WaitForSingleObject(event.as_raw_handle().cast(), INFINITE) };
        if waited != WAIT_OBJECT_0 {
            return;
        }
        on_request();
    }))
    .map_err(|error| error.to_string())
}

/// The helper side: signal the broker at `broker_executable` and wait, at most
/// `timeout`, for it to exit (its event disappears with its last handle).
/// Repeats the request every `resignal_every`: the event is auto-reset, and
/// the first request can land while the broker's own host connection is still
/// closing. `Ok(true)` means it exited, `Ok(false)` that none was running or
/// it kept running; neither is the installer's verdict, which belongs to the
/// unlock check that follows.
#[cfg(windows)]
pub fn request_stop(
    broker_executable: &Path,
    timeout: std::time::Duration,
    resignal_every: std::time::Duration,
) -> Result<bool, String> {
    use std::time::Instant;
    let name = event_name(broker_executable);
    if !signal(&name)? {
        return Ok(false);
    }
    let deadline = Instant::now() + timeout;
    let mut resignal_at = Instant::now() + resignal_every;
    loop {
        if !exists(&name)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        if Instant::now() >= resignal_at {
            if !signal(&name)? {
                return Ok(true);
            }
            resignal_at = Instant::now() + resignal_every;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn open(name: &str, access: u32) -> Result<Option<isize>, String> {
    use windows_sys::Win32::System::Threading::OpenEventW;
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a terminated name read only during the call.
    let handle = unsafe { OpenEventW(access, 0, wide.as_ptr()) };
    if !handle.is_null() {
        return Ok(Some(handle as isize));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(2) {
        Ok(None)
    } else {
        Err(error.to_string())
    }
}

#[cfg(windows)]
fn signal(name: &str) -> Result<bool, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{SetEvent, EVENT_MODIFY_STATE};
    let Some(handle) = open(name, EVENT_MODIFY_STATE)? else {
        return Ok(false);
    };
    // SAFETY: a handle this function opened and closes before returning.
    let set = unsafe { SetEvent(handle as _) };
    let error = std::io::Error::last_os_error();
    // SAFETY: as above. Never keep the named object alive while waiting.
    unsafe { CloseHandle(handle as _) };
    if set == 0 {
        return Err(error.to_string());
    }
    Ok(true)
}

#[cfg(windows)]
fn exists(name: &str) -> Result<bool, String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::SYNCHRONIZATION_SYNCHRONIZE;
    let Some(handle) = open(name, SYNCHRONIZATION_SYNCHRONIZE)? else {
        return Ok(false);
    };
    // SAFETY: closing the handle just opened.
    unsafe { CloseHandle(handle as _) };
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn watcher_rejected_spawn_and_callback_unwind_release_the_owned_event() {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::CreateEventW;
        for callback_unwind in [false, true] {
            let path = std::env::temp_dir().join(format!(
                "qc-install-stop-owner-{:032x}/qc-device-broker.exe",
                rand::random::<u128>()
            ));
            let name = event_name(&path);
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            assert!(!exists(&name).unwrap(), "fresh fixture namespace");
            let raw = unsafe {
                CreateEventW(
                    std::ptr::null(),
                    0,
                    i32::from(callback_unwind),
                    wide.as_ptr(),
                )
            };
            assert!(!raw.is_null());
            let event = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
            assert!(exists(&name).unwrap());
            if callback_unwind {
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    watch_event_with(
                        event,
                        || panic!("injected callback unwind"),
                        |task| {
                            task(); // The already-signalled event enters the actual watcher callback.
                            Ok(())
                        },
                    )
                }))
                .expect_err("callback panic is preserved");
                assert_eq!(
                    panic.downcast_ref::<&str>(),
                    Some(&"injected callback unwind")
                );
            } else {
                let error = watch_event_with(
                    event,
                    || panic!("refused task cannot run"),
                    |_task| Err(std::io::Error::other("injected thread admission failure")),
                )
                .unwrap_err();
                assert_eq!(error, "injected thread admission failure");
            }
            assert!(
                !exists(&name).unwrap(),
                "last owned event handle closed on failure"
            );
        }
    }

    #[test]
    fn the_name_is_path_scoped_case_insensitive_and_pinned_across_crates() {
        // The same literal is pinned in tray_native.rs; change both or neither.
        assert_eq!(
            event_name(Path::new(r"C:\Apps\QC Remote\qc-device-broker.exe")),
            "Local\\QCRemote.DeviceBroker.InstallStop.80c0dd0940482133"
        );
        assert_eq!(
            event_name(Path::new(r"C:\Apps\QC Remote\qc-device-broker.exe")),
            event_name(Path::new("c:/apps/qc remote/QC-DEVICE-BROKER.EXE"))
        );
        assert_ne!(
            event_name(Path::new(r"C:\Apps\QC Remote\qc-device-broker.exe")),
            event_name(Path::new(r"D:\Other\QC Remote\qc-device-broker.exe"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_helper_repeats_a_refused_request_and_reports_the_exit() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
        let broker = std::env::temp_dir().join(format!(
            "qc-install-stop-{}-{}/qc-device-broker.exe",
            std::process::id(),
            line!()
        ));
        assert_eq!(
            request_stop(
                &broker,
                Duration::from_millis(50),
                Duration::from_millis(10)
            ),
            Ok(false),
            "no broker running is not an error"
        );
        let name: Vec<u16> = event_name(&broker).encode_utf16().chain(Some(0)).collect();
        // Auto-reset, exactly as `watch` creates it.
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; no security
        // attributes are passed. A null result is caught by the assertion below.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        assert!(!event.is_null());
        let event = unsafe { OwnedHandle::from_raw_handle(event.cast()) };
        let waiter = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut seen = 0;
            while seen < 2 {
                assert!(Instant::now() < deadline, "the request was not repeated");
                // SAFETY: `event` is the non-zero handle created above. Only this closure
                // uses it, and it is not closed until after the loop.
                if unsafe { WaitForSingleObject(event.as_raw_handle().cast(), 10) } == WAIT_OBJECT_0
                {
                    seen += 1; // the first is refused (host still closing)
                }
            }
            // OwnedHandle closes on success and on an assertion unwind alike.
        });
        assert_eq!(
            request_stop(&broker, Duration::from_secs(3), Duration::from_millis(100)),
            Ok(true)
        );
        waiter.join().unwrap();
    }
}
