//! Whether this session's resident host is running, for the app's Settings row
//! and its "start it now" decision. Since 0.1.178 the resident host is
//! `qc-device-broker.exe` itself (broker, icon and background MCP host in one
//! process); it holds this named mutex exactly while it is resident. The name
//! is the one `qc-remote-tray.exe` used before, so the app reads an old
//! installation's still-running tray the same way during an upgrade, and that
//! old tray cannot start a second background host beside a new broker. The
//! broker pins the same literal (services/device-broker/src/resident.rs).
use std::ffi::c_void;

type Handle = *mut c_void;
pub(crate) const SESSION_MARKER: &str = "Local\\QCRemote.DeviceBrokerTray.v1";
// SAFETY: these declarations match the kernel32 signatures `HANDLE OpenMutexW(DWORD, BOOL,
// LPCWSTR)`, `HANDLE CreateMutexW(LPSECURITY_ATTRIBUTES, BOOL, LPCWSTR)` (tests only) and
// `BOOL CloseHandle(HANDLE)`. `isize` stands in for HANDLE in CloseHandle: both are
// pointer-sized and passed the same way on the x86 and x64 Windows ABIs.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> Handle;
    #[cfg(test)]
    fn CreateMutexW(attributes: Handle, owner: i32, name: *const u16) -> Handle;
    fn CloseHandle(handle: isize) -> i32;
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn running_named(name: &str) -> bool {
    // SAFETY: OpenMutex reads the terminated name only for the call's duration.
    let handle = unsafe { OpenMutexW(0x0010_0000, 0, wide(name).as_ptr()) };
    if handle.is_null() {
        false
    } else {
        // SAFETY: closing the handle just opened.
        unsafe { CloseHandle(handle as isize) };
        true
    }
}
pub fn is_running() -> bool {
    running_named(SESSION_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_marker_reads_running_exactly_while_a_holder_keeps_it() {
        let name = format!(
            "Local\\QCRemote.ResidentTest.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert!(!running_named(&name));
        // SAFETY: default security, a terminated name read during the call.
        let held = unsafe { CreateMutexW(std::ptr::null_mut(), 0, wide(&name).as_ptr()) };
        assert!(!held.is_null());
        assert!(running_named(&name));
        unsafe { CloseHandle(held as isize) };
        assert!(!running_named(&name));
    }

    #[test]
    fn the_marker_name_is_the_one_the_broker_holds() {
        // services/device-broker/src/resident.rs SESSION_MARKER.
        assert_eq!(SESSION_MARKER, "Local\\QCRemote.DeviceBrokerTray.v1");
    }
}
