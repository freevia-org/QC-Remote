//! GUI observations of the authenticated tray-owned host. Never opens USB.
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
static STOPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static OBSERVERS: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(1);
static ORIGINAL: Mutex<Option<qc_native_platform::thread_owner::Owned>> = Mutex::new(None);
pub(crate) fn stopping() -> bool {
    STOPPING.load(std::sync::atomic::Ordering::Acquire)
}
pub(crate) fn stop() {
    STOPPING.store(true, std::sync::atomic::Ordering::Release);
    match ORIGINAL.try_lock() {
        Ok(mut original) => {
            original.take();
        }
        Err(std::sync::TryLockError::Poisoned(error)) => {
            error.into_inner().take();
        }
        Err(std::sync::TryLockError::WouldBlock) => {}
    }
    // Nonblocking retirement request, not an assertion that the thread ended.
    // Its shared original owner retains AppHandle and admission until join.
    qc_native_platform::thread_owner::reap();
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
fn observation_loop(stopping: impl Fn() -> bool, mut observe: impl FnMut()) {
    loop {
        if stopping() {
            return;
        }
        observe();
        qc_native_platform::thread_owner::reap();
        std::thread::sleep(Duration::from_secs(1));
    }
}
pub(crate) fn start(app: tauri::AppHandle, directory: PathBuf) {
    use tauri::Manager;
    if stopping() {
        return;
    }
    let mut slot = match ORIGINAL.try_lock() {
        Ok(slot) => slot,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if slot.as_mut().is_some_and(|original| !original.finished()) {
        return;
    }
    *slot = None;
    let resource = app.clone();
    let started = qc_native_platform::thread_owner::Owned::spawn(
        "qc-tray-status",
        &OBSERVERS,
        resource,
        move || {
            observation_loop(stopping, || {
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
            });
        },
    );
    if let Ok(original) = started {
        *slot = Some(original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    #[test]
    fn stopping_an_observation_loop_joins_its_original_before_releasing_its_resource() {
        static BUDGET: qc_native_platform::thread_owner::Budget =
            qc_native_platform::thread_owner::Budget::new(1);
        struct Resource(Arc<AtomicUsize>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::AcqRel);
            }
        }
        struct Stop(Arc<AtomicBool>);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_guard = Stop(stop.clone());
        let released = Arc::new(AtomicUsize::new(0));
        let (observed, observation) = mpsc::channel();
        let mut original = qc_native_platform::thread_owner::Owned::spawn(
            "gui-observation-fixture",
            &BUDGET,
            Resource(released.clone()),
            move || {
                observation_loop(
                    || stop.load(Ordering::Acquire),
                    || {
                        let _ = observed.send(());
                    },
                );
            },
        )
        .unwrap();
        observation.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(released.load(Ordering::Acquire), 0);
        drop(stop_guard);
        assert!(original.wait_within(Duration::from_secs(2)));
        assert_eq!(released.load(Ordering::Acquire), 1);
        assert_eq!(BUDGET.count(), 0);
    }
}
