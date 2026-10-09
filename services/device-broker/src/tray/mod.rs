//! The broker's own notification icon. It lives in this process so that the
//! broker and its icon share one lifecycle: the icon appears once the broker
//! has won the descriptor election and disappears when it exits, and the
//! device is held past the shipped grace only while the icon is registered.
//!
//! Three threads, none of which can wedge the device:
//! - the UI thread owns a hidden window, the icon and the popup menu, and
//!   touches nothing of the broker's;
//! - the state thread builds a snapshot from the broker's cached status and
//!   runs menu commands (each on its own short-lived thread);
//! - the idle timer is the broker's own backstop thread, not either of these.
pub mod host_status;
pub mod install_stop;
pub mod model;
#[cfg_attr(not(windows), allow(dead_code))]
mod state;
#[cfg(windows)]
mod ui;

use crate::resident::ResidentHost;
use crate::rpc::BrokerHandle;
use crate::tray_settings::{IdlePolicy, TraySettings};
use std::ffi::OsString;
use std::sync::Arc;

/// Environment escape hatch for test harnesses that spawn a real broker
/// through production code. Equivalent to a non-interactive session.
pub const NO_TRAY_ENV: &str = "QC_DEVICE_BROKER_NO_TRAY";
pub const NO_TRAY_FLAG: &str = "--no-tray";

/// Why a `--broker-serve` broker draws no icon, if it does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suppressed {
    Flag,
    Environment,
    NotInteractive,
}

/// Whether this broker may draw an icon at all. `trayEnabled` is not part of
/// this: it is read live, so the setting can change under a running broker.
pub fn allowed(
    no_tray_flag: bool,
    no_tray_env: Option<OsString>,
    interactive: bool,
) -> Result<(), Suppressed> {
    if no_tray_flag {
        return Err(Suppressed::Flag);
    }
    if no_tray_env.is_some_and(|value| !value.is_empty() && value != "0") {
        return Err(Suppressed::Environment);
    }
    if !interactive {
        return Err(Suppressed::NotInteractive);
    }
    Ok(())
}

/// The hold the broker may apply: only while the icon is actually registered
/// with the shell, and only while the user's switch says the icon is wanted.
pub fn hold_for(icon_registered: bool, settings: TraySettings) -> Option<IdlePolicy> {
    (icon_registered && settings.tray_enabled).then_some(settings.idle)
}

/// A session with a desktop the user can see: not session 0, and a visible
/// window station. A service, a CI runner or a non-interactive logon is not.
#[cfg(windows)]
pub fn interactive_session() -> bool {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::WSF_VISIBLE;
    let mut session = 0_u32;
    // SAFETY: both calls write only into the locals passed by pointer.
    unsafe {
        if ProcessIdToSessionId(GetCurrentProcessId(), &mut session) == 0 || session == 0 {
            return false;
        }
        let station = GetProcessWindowStation();
        if station.is_null() {
            return false;
        }
        let mut flags: USEROBJECTFLAGS = std::mem::zeroed();
        let mut needed = 0_u32;
        if GetUserObjectInformationW(
            station,
            UOI_FLAGS,
            (&mut flags as *mut USEROBJECTFLAGS).cast(),
            std::mem::size_of::<USEROBJECTFLAGS>() as u32,
            &mut needed,
        ) == 0
        {
            return false;
        }
        flags.dwFlags & WSF_VISIBLE as u32 != 0
    }
}

#[cfg(not(windows))]
pub fn interactive_session() -> bool {
    false
}

/// Start the icon for a published broker. Failure to start is reported and
/// leaves the broker exactly as it shipped: no icon, no hold. `host` is the
/// background MCP host this binary links, if any: the icon's Unlink on a local
/// client reaches it in process.
pub fn start(handle: BrokerHandle, host: Option<Arc<dyn ResidentHost>>) {
    #[cfg(windows)]
    {
        if let Err(error) = state::start(handle, host, ui::start) {
            eprintln!("The broker icon could not start; the broker runs without it: {error}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (handle, host);
    }
}

/// Remove the icon immediately from any thread. Called just before the process
/// exits so no ghost icon is left behind. Never waits on the UI thread.
pub fn remove_icon_now() {
    #[cfg(windows)]
    ui::remove_icon_now();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_interactive_unsuppressed_broker_may_draw() {
        assert_eq!(allowed(false, None, true), Ok(()));
        assert_eq!(allowed(true, None, true), Err(Suppressed::Flag));
        assert_eq!(
            allowed(false, Some("1".into()), true),
            Err(Suppressed::Environment)
        );
        assert_eq!(allowed(false, Some("0".into()), true), Ok(()));
        assert_eq!(allowed(false, Some("".into()), true), Ok(()));
        assert_eq!(allowed(false, None, false), Err(Suppressed::NotInteractive));
    }

    #[test]
    fn the_hold_requires_both_a_registered_icon_and_the_switch() {
        let on = TraySettings {
            tray_enabled: true,
            idle: IdlePolicy::Never,
        };
        assert_eq!(hold_for(true, on), Some(IdlePolicy::Never));
        assert_eq!(hold_for(false, on), None, "no icon, no hold");
        let off = TraySettings {
            tray_enabled: false,
            ..on
        };
        assert_eq!(hold_for(true, off), None, "switched off, no hold");
    }
}
