//! Directory and clock shared by UI hosts and the standalone tray.
use std::path::PathBuf;

pub const APP_DIRECTORY: &str = "org.freevia.qcremote";

pub fn app_local_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local"))
        })
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIRECTORY)
}

pub fn unix_time_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}
