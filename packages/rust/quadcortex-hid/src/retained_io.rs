//! Exact retained HID I/O thread ownership. No OS cancellation promise.
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn thread_signalled<T>(handle: &JoinHandle<T>) -> bool {
    #[cfg(windows)]
    {
        qc_windows_process::thread_signalled(handle)
    }
    #[cfg(not(windows))]
    {
        handle.is_finished()
    }
}

const MAX_IO_LIFETIMES: usize = 4;
#[derive(Default)]
pub(crate) struct Admissions(Mutex<HashSet<String>>);
pub(crate) struct IoReservation {
    admissions: Arc<Admissions>,
    identity: String,
}
impl Admissions {
    pub(crate) fn reserve(self: &Arc<Self>, identity: &str) -> Result<Arc<IoReservation>, String> {
        let mut live = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if live.contains(identity) {
            return Err("the previous native HID lifetime still owns this device".into());
        }
        if live.len() >= MAX_IO_LIFETIMES {
            return Err(
                "native HID lifetime capacity exhausted; retained I/O must retire first".into(),
            );
        }
        live.insert(identity.to_owned());
        Ok(Arc::new(IoReservation {
            admissions: Arc::clone(self),
            identity: identity.into(),
        }))
    }
}
impl Drop for IoReservation {
    fn drop(&mut self) {
        self.admissions
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.identity);
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterFinish {
    Joined,
    Retained,
    Panicked,
}
pub(crate) struct OwnedWriter {
    handle: JoinHandle<()>,
    // Not released by worker return: retained until the original handle is joined.
    _reservation: Arc<IoReservation>,
}
impl OwnedWriter {
    pub(crate) fn new(handle: JoinHandle<()>, reservation: Arc<IoReservation>) -> Self {
        Self {
            handle,
            _reservation: reservation,
        }
    }
    pub(crate) fn finish(self, budget: Duration) -> WriterFinish {
        self.finish_in(retained(), budget)
    }
    fn finish_in(self, registry: &Mutex<Vec<Self>>, budget: Duration) -> WriterFinish {
        let deadline = Instant::now() + budget;
        while !thread_signalled(&self.handle) && Instant::now() < deadline {
            std::thread::sleep(
                Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        if thread_signalled(&self.handle) {
            return if self.handle.join().is_ok() {
                WriterFinish::Joined
            } else {
                WriterFinish::Panicked
            };
        }
        reap(registry);
        // RX and TX can each retain one original per admitted native lifetime.
        // Both share that lifetime's admission, so at most eight originals exist.
        // The registry never owns Admissions, so there is no retention cycle.
        registry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(self);
        WriterFinish::Retained
    }
}
fn retained() -> &'static Mutex<Vec<OwnedWriter>> {
    static RETAINED: OnceLock<Mutex<Vec<OwnedWriter>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::with_capacity(MAX_IO_LIFETIMES * 2)))
}
fn reap(registry: &Mutex<Vec<OwnedWriter>>) {
    let finished = {
        let mut records = registry.lock().unwrap_or_else(|p| p.into_inner());
        let mut finished = Vec::new();
        let mut index = 0;
        while index < records.len() {
            if thread_signalled(&records[index].handle) {
                finished.push(records.swap_remove(index));
            } else {
                index += 1;
            }
        }
        finished
    };
    for record in finished {
        let _ = record.handle.join();
    }
}
#[cfg(test)]
pub(crate) fn reap_retained_for_test() {
    reap(retained());
}

pub(crate) fn reserve(identity: &str) -> Result<Arc<IoReservation>, String> {
    reap(retained());
    static ADMISSIONS: OnceLock<Arc<Admissions>> = OnceLock::new();
    ADMISSIONS
        .get_or_init(|| Arc::new(Admissions::default()))
        .reserve(identity)
}
/// Fields drop in order: the real native handle before the host's lease owner.
pub(crate) struct NativeLifetime<T> {
    pub(crate) device: T,
    pub(crate) _io_owner: Option<Arc<dyn Send + Sync>>,
    pub(crate) _reservation: Arc<IoReservation>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    struct Probe(Arc<Mutex<Vec<&'static str>>>, &'static str);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.lock().unwrap().push(self.1);
        }
    }
    struct FixtureCleanup<'a> {
        release: Option<mpsc::Sender<()>>,
        registry: &'a Mutex<Vec<OwnedWriter>>,
        pending: Option<OwnedWriter>,
    }
    impl Drop for FixtureCleanup<'_> {
        fn drop(&mut self) {
            if let Some(sender) = self.release.take() {
                let _ = sender.send(());
            }
            if let Some(writer) = self.pending.take() {
                writer.finish_in(self.registry, Duration::ZERO);
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                reap(self.registry);
                if self
                    .registry
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty()
                {
                    return;
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            // Never discard original handles if failure cleanup cannot acknowledge.
            let outstanding =
                std::mem::take(&mut *self.registry.lock().unwrap_or_else(|p| p.into_inner()));
            retained()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend(outstanding);
            eprintln!("held-writer fixture cleanup retained its exact unacknowledged task");
        }
    }
    #[cfg(windows)]
    #[test]
    fn native_tls_writer_retains_reopen_admission_until_original_signal() {
        struct ExitGate {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl Drop for ExitGate {
            fn drop(&mut self) {
                let _ = self.entered.send(());
                let _ = self.release.recv_timeout(Duration::from_secs(5));
            }
        }
        thread_local! {
            static EXIT_GATE: std::cell::RefCell<Option<ExitGate>> =
                const { std::cell::RefCell::new(None) };
        }
        let admissions = Arc::new(Admissions::default());
        let registry = Mutex::new(Vec::new());
        let reservation = admissions.reserve("tls").unwrap();
        let (release, held) = mpsc::channel();
        let (entered, observed) = mpsc::channel();
        let mut cleanup = FixtureCleanup {
            release: Some(release),
            registry: &registry,
            pending: None,
        };
        let handle = std::thread::spawn(move || {
            EXIT_GATE.with(|gate| {
                *gate.borrow_mut() = Some(ExitGate {
                    entered,
                    release: held,
                })
            });
        });
        cleanup.pending = Some(OwnedWriter::new(handle, reservation));
        observed.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(cleanup.pending.as_ref().unwrap().handle.is_finished());
        assert!(!thread_signalled(&cleanup.pending.as_ref().unwrap().handle));
        let began = Instant::now();
        assert_eq!(
            cleanup
                .pending
                .take()
                .unwrap()
                .finish_in(&registry, Duration::from_millis(25)),
            WriterFinish::Retained
        );
        reap(&registry);
        assert!(began.elapsed() < Duration::from_secs(1));
        assert!(admissions.reserve("tls").is_err());
        cleanup.release.take().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            reap(&registry);
            if registry.lock().unwrap().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "native TLS writer failed to retire"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(admissions.reserve("tls").is_ok());
    }

    #[test]
    fn writer_finish_reports_actual_join_and_panic_instead_of_retirement_success() {
        let admissions = Arc::new(Admissions::default());
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_completed = Arc::clone(&completed);
        let reservation = admissions.reserve("joined").unwrap();
        let handle = std::thread::spawn(move || {
            worker_completed.store(true, std::sync::atomic::Ordering::Release);
        });
        let owner = OwnedWriter::new(handle, reservation);
        assert_eq!(owner.finish(Duration::from_secs(2)), WriterFinish::Joined);
        assert!(completed.load(std::sync::atomic::Ordering::Acquire));
        let reservation = admissions.reserve("panicked").unwrap();
        let handle = std::thread::spawn(|| panic!("controlled writer panic"));
        let owner = OwnedWriter::new(handle, reservation);
        assert_eq!(owner.finish(Duration::from_secs(2)), WriterFinish::Panicked);
    }

    #[test]
    fn held_writer_retains_original_handle_native_owner_and_reopen_exclusion() {
        let admissions = Arc::new(Admissions::default());
        let registry = Mutex::new(Vec::new());
        let reservation = admissions.reserve("device").unwrap();
        let drops = Arc::new(Mutex::new(Vec::new()));
        let capability: Arc<dyn Send + Sync> = Arc::new(Probe(Arc::clone(&drops), "lease"));
        let native = NativeLifetime {
            device: Probe(Arc::clone(&drops), "native"),
            _io_owner: Some(capability),
            _reservation: Arc::clone(&reservation),
        };
        let (release, wait) = mpsc::channel();
        let mut cleanup = FixtureCleanup {
            release: Some(release),
            registry: &registry,
            pending: None,
        };
        let (entered, entry) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _native = native;
            entered.send(()).unwrap();
            // Failure cleanup cannot create a permanently blocked test task.
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        // Arm the original handle before any fallible observation.
        cleanup.pending = Some(OwnedWriter::new(handle, Arc::clone(&reservation)));
        entry.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            cleanup
                .pending
                .take()
                .unwrap()
                .finish_in(&registry, Duration::from_millis(20)),
            WriterFinish::Retained,
        );
        drop(reservation);
        assert_eq!(registry.lock().unwrap().len(), 1);
        assert!(drops.lock().unwrap().is_empty());
        assert!(admissions.reserve("device").is_err());
        cleanup.release.take().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            reap(&registry);
            if registry.lock().unwrap().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "original retained writer did not retire"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(*drops.lock().unwrap(), ["native", "lease"]);
        let replacement = admissions.reserve("device").unwrap();
        drop(replacement);
        assert!(admissions.0.lock().unwrap().is_empty());
    }
    #[test]
    fn held_writer_fixture_unwind_releases_and_acknowledges_its_original_task() {
        let admissions = Arc::new(Admissions::default());
        let registry = Mutex::new(Vec::new());
        let (release, wait) = mpsc::channel();
        let (done, acknowledgement) = mpsc::channel();
        let reservation = admissions.reserve("unwind").unwrap();
        let handle = std::thread::spawn(move || {
            let _ = wait.recv_timeout(Duration::from_secs(5));
            let _ = done.send(());
        });
        let cleanup = FixtureCleanup {
            release: Some(release),
            registry: &registry,
            pending: Some(OwnedWriter::new(handle, reservation)),
        };
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _cleanup = cleanup;
            panic!("fixture observation failed before entry acknowledgement");
        }));
        assert!(panic.is_err());
        acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(
            registry.lock().unwrap().is_empty(),
            "original handle was not joined"
        );
        assert!(admissions.0.lock().unwrap().is_empty());
    }
    #[test]
    fn reader_native_owner_keeps_admission_after_original_writer_join() {
        let admissions = Arc::new(Admissions::default());
        let registry = Mutex::new(Vec::new());
        let reservation = admissions.reserve("reader").unwrap();
        let drops = Arc::new(Mutex::new(Vec::new()));
        let lease: Arc<dyn Send + Sync> = Arc::new(Probe(Arc::clone(&drops), "lease"));
        let native = Arc::new(NativeLifetime {
            device: Probe(Arc::clone(&drops), "native"),
            _io_owner: Some(lease),
            _reservation: Arc::clone(&reservation),
        });
        let writer_native = Arc::clone(&native);
        let writer = std::thread::spawn(move || drop(writer_native));
        let mut cleanup = FixtureCleanup {
            release: None,
            registry: &registry,
            pending: Some(OwnedWriter::new(writer, reservation)),
        };
        cleanup
            .pending
            .take()
            .unwrap()
            .finish_in(&registry, Duration::from_secs(1));
        assert!(
            registry.lock().unwrap().is_empty(),
            "original writer join acknowledged"
        );
        assert!(
            admissions.reserve("reader").is_err(),
            "reader still owns native lifetime"
        );
        assert!(drops.lock().unwrap().is_empty());
        drop(native);
        assert_eq!(*drops.lock().unwrap(), ["native", "lease"]);
        let replacement = admissions.reserve("reader").unwrap();
        drop(replacement);
        assert!(admissions.0.lock().unwrap().is_empty());
    }
    #[test]
    fn native_lifetime_capacity_refuses_before_open_and_releases_on_acknowledged_drop() {
        let admissions = Arc::new(Admissions::default());
        let mut permits = (0..MAX_IO_LIFETIMES)
            .map(|n| admissions.reserve(&format!("device-{n}")).unwrap())
            .collect::<Vec<_>>();
        assert!(admissions.reserve("overflow").is_err());
        drop(permits.pop());
        let replacement = admissions.reserve("overflow").unwrap();
        assert_eq!(admissions.0.lock().unwrap().len(), MAX_IO_LIFETIMES);
        drop(replacement);
        drop(permits);
        assert!(admissions.0.lock().unwrap().is_empty());
    }
}
