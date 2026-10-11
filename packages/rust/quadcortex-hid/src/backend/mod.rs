#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::{device_id, write_report_delivered, SharedLinuxHid as NativeHid};
#[cfg(windows)]
pub(crate) use windows::{device_id, write_report_delivered, SharedWindowsHid as NativeHid};
