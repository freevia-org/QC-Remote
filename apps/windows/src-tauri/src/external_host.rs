//! GUI observations of the authenticated tray-owned host. Never opens USB.
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn stopping() -> bool {
    STOPPING.load(std::sync::atomic::Ordering::Acquire)
}
pub(crate) fn stop() {
    STOPPING.store(true, std::sync::atomic::Ordering::Release);
}

fn cache() -> &'static Mutex<Option<(Instant, Value)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}
pub(crate) fn status() -> Option<Value> {
    cache()
        .lock()
        .ok()?
        .as_ref()
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(3))
        .map(|(_, value)| value.clone())
}
pub(crate) fn delegated(directory: &std::path::Path) -> bool {
    crate::broker_tray_settings::broker_tray_settings_in(directory).tray_enabled
}
pub(crate) fn start(app: tauri::AppHandle, directory: PathBuf) {
    use tauri::Manager;
    std::thread::Builder::new()
        .name("qc-tray-status".into())
        .spawn(move || {
            loop {
                if stopping() {
                    break;
                }
                // Ownership changes are applied even while the Settings page is
                // closed. Disabling the tray restores the GUI's original host.
                let delegated = delegated(&directory);
                let observed = delegated
                    .then(|| crate::host_runtime_status::read(&directory))
                    .flatten();
                *cache().lock().unwrap_or_else(|p| p.into_inner()) =
                    observed.map(|value| (Instant::now(), value));
                if let Some(local) = app.try_state::<crate::local_access::LocalAccess>() {
                    local.reconcile_ownership();
                }
                if app.try_state::<crate::relay::RelayBridge>().is_some() {
                    crate::relay::RelayBridge::schedule_reconcile(app.clone(), delegated);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .ok();
}
