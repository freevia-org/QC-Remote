#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteStallPolicy {
    #[cfg(any(target_os = "linux", test))]
    Strict,
    #[cfg(windows)]
    WindowsQcError31,
    #[cfg(target_os = "macos")]
    MacosSerialized,
    #[cfg(target_os = "linux")]
    LinuxQc880aControlStatusEpipe,
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{device_id, write_report_delivered, SharedLinuxHid as NativeHid};
#[cfg(target_os = "macos")]
pub(crate) use macos::{
    device_id, with_hid_thread, write_report_delivered, SharedMacosHid as NativeHid,
};
#[cfg(windows)]
pub(crate) use windows::{device_id, write_report_delivered, SharedWindowsHid as NativeHid};
