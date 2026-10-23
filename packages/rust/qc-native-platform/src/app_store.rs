//! Services available only in the separately signed Mac App Store build.
use std::{ffi::CStr, os::unix::ffi::OsStrExt, path::PathBuf};
extern "C" {
    fn qc_store_user_home() -> *mut libc::c_char;
    fn qc_store_has_folder_access(path: *const libc::c_char) -> bool;
    fn qc_store_authorize_folder(path: *const libc::c_char) -> *mut libc::c_char;
    fn qc_store_set_login_enabled(enabled: bool) -> *mut libc::c_char;
    fn qc_store_shared_directory() -> *mut libc::c_char;
}

/// Fail closed if the signed app lacks its shared-container entitlement. Falling
/// back to an app-private directory would silently split GUI and broker state.
pub fn shared_directory() -> Result<PathBuf, String> {
    // SAFETY: the shim returns a fresh malloc allocation or null. Copy its
    // nul-terminated filesystem bytes before releasing it exactly once.
    unsafe {
        let value = qc_store_shared_directory();
        if value.is_null() {
            return Err("QC Remote cannot access its signed App Group container".into());
        }
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(
            CStr::from_ptr(value).to_bytes(),
        ));
        libc::free(value.cast());
        if !path.is_absolute() {
            return Err("The App Group container path is not absolute".into());
        }
        Ok(path)
    }
}

/// Register the bundled launcher using ServiceManagement, without writing into
/// the user's LaunchAgents directory. The native error allocation is copied
/// before freeing it, and no password or credential is handled here.
pub fn set_login_enabled(enabled: bool) -> Result<(), String> {
    // SAFETY: the shim accepts a boolean and returns null or a malloc string.
    unsafe {
        let error = qc_store_set_login_enabled(enabled);
        if error.is_null() {
            return Ok(());
        }
        let message = CStr::from_ptr(error).to_string_lossy().into_owned();
        libc::free(error.cast());
        Err(message)
    }
}

/// The user's real home (rather than the sandbox's rewritten HOME), used only
/// to suggest host configuration locations. It does not grant filesystem access.
pub fn user_home() -> Option<PathBuf> {
    // SAFETY: returns null or a fresh malloc filesystem string.
    unsafe {
        let value = qc_store_user_home();
        if value.is_null() {
            return None;
        }
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(
            CStr::from_ptr(value).to_bytes(),
        ));
        libc::free(value.cast());
        path.is_absolute().then_some(path)
    }
}

/// Ask the owner to select exactly one client's configuration folder. Call only
/// in response to Pair/Unpair, never from periodic status or startup refresh.
pub fn authorize_folder(path: &std::path::Path) -> Result<(), String> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "The client folder contains a nul byte".to_string())?;
    // SAFETY: the path stays alive for the synchronous call. The native shim
    // dispatches the panel to AppKit's main thread and returns a malloc error.
    unsafe {
        let error = qc_store_authorize_folder(path.as_ptr());
        if error.is_null() {
            return Ok(());
        }
        let message = CStr::from_ptr(error).to_string_lossy().into_owned();
        libc::free(error.cast());
        Err(message)
    }
}

/// Query a restored user-selected scope without ever displaying a dialog.
pub fn has_folder_access(path: &std::path::Path) -> bool {
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: the nul-terminated path remains alive throughout the native call.
    unsafe { qc_store_has_folder_access(path.as_ptr()) }
}
