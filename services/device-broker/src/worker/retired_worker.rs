use super::{WorkerExit, WorkerThreadState};
use quadcortex::sync::RecoverPoison;
use std::sync::{Arc, Mutex, Weak};
use std::thread;

pub(super) const MAX_WORKER_LIFETIMES: usize = 4;

/// Includes healthy, searching and retiring workers. Reserving before spawn
/// prevents concurrent healthy workers from overflowing retired ownership later.
pub(super) struct WorkerRetirement {
    limit: usize,
    admitted: Mutex<usize>,
    retired: Mutex<Vec<RetiredWorker>>,
}
struct RetiredWorker {
    handle: thread::JoinHandle<()>,
    slot: Weak<WorkerThreadState>,
    lifetime: Arc<WorkerLifetime>,
}
pub(super) struct WorkerLifetime(pub(super) Arc<WorkerRetirement>);
impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        *self.0.admitted.lock_recover() -= 1;
    }
}

impl WorkerRetirement {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            admitted: Mutex::new(0),
            retired: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn production() -> Arc<Self> {
        #[cfg(not(test))]
        {
            static OWNER: std::sync::OnceLock<Arc<WorkerRetirement>> = std::sync::OnceLock::new();
            Arc::clone(OWNER.get_or_init(|| Self::new(MAX_WORKER_LIFETIMES)))
        }
        #[cfg(test)]
        {
            // Independent unit-test cohorts still exercise the same admission
            // and retirement implementation without contending across tests.
            thread_local! {
                static OWNER: Arc<WorkerRetirement> =
                    WorkerRetirement::new(MAX_WORKER_LIFETIMES);
            }
            OWNER.with(Arc::clone)
        }
    }

    pub(super) fn reserve(self: &Arc<Self>) -> Result<Arc<WorkerLifetime>, String> {
        self.reap();
        let mut admitted = self.admitted.lock_recover();
        if *admitted >= self.limit {
            return Err(format!(
                "Native QC worker capacity exhausted ({} worker lifetimes, including disconnected/searching or retiring workers); wait for an existing worker to retire",
                self.limit
            ));
        }
        *admitted += 1;
        Ok(Arc::new(WorkerLifetime(Arc::clone(self))))
    }

    pub(super) fn retain(
        &self,
        handle: thread::JoinHandle<()>,
        slot: Weak<WorkerThreadState>,
        lifetime: Arc<WorkerLifetime>,
    ) {
        let mut complete = Vec::new();
        {
            let mut retired = self.retired.lock_recover();
            let mut index = 0;
            while index < retired.len() {
                if super::thread_signalled(&retired[index].handle) {
                    complete.push(retired.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            let worker = RetiredWorker {
                handle,
                slot,
                lifetime,
            };
            if super::thread_signalled(&worker.handle) {
                complete.push(worker);
            } else {
                // Each original retains its permit until actual join. One
                // lifetime owns USB, state decoder and metadata decoder originals.
                debug_assert!(retired.len() < self.limit.saturating_mul(3));
                retired.push(worker);
            }
        }
        Self::join_completed(complete);
    }

    pub(super) fn reap(&self) {
        let complete = {
            let mut retired = self.retired.lock_recover();
            let mut complete = Vec::new();
            let mut index = 0;
            while index < retired.len() {
                if super::thread_signalled(&retired[index].handle) {
                    complete.push(retired.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            complete
        };
        Self::join_completed(complete);
    }

    fn join_completed(complete: Vec<RetiredWorker>) {
        for worker in complete {
            let exit = if worker.handle.join().is_ok() {
                WorkerExit::Joined
            } else {
                WorkerExit::Panicked
            };
            if let Some(slot) = worker.slot.upgrade() {
                let exit = if exit == WorkerExit::Joined {
                    slot.joined_exit()
                } else {
                    exit
                };
                *slot.lock_recover() = super::WorkerSlot::Stopped(exit);
            }
            drop(worker.lifetime);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Owns the original before the first assertion or readiness wait.
    struct HeldWorker {
        handle: Option<thread::JoinHandle<()>>,
        release: Option<mpsc::Sender<()>>,
        registry: Arc<WorkerRetirement>,
        lifetime: Option<Arc<WorkerLifetime>>,
    }
    impl Drop for HeldWorker {
        fn drop(&mut self) {
            self.release.take();
            if let Some(handle) = self.handle.take() {
                self.registry
                    .retain(handle, Weak::new(), self.lifetime.take().unwrap());
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                self.registry.reap();
                if self.registry.retired.lock_recover().is_empty() {
                    break;
                }
                if Instant::now() >= deadline {
                    eprintln!("controlled worker did not retire; original handle remains owned");
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_tls_finalization_keeps_the_original_worker_and_admission_retained() {
        struct ExitGate {
            entered: Option<mpsc::Sender<()>>,
            release: mpsc::Receiver<()>,
        }
        impl Drop for ExitGate {
            fn drop(&mut self) {
                if let Some(entered) = self.entered.take() {
                    let _ = entered.send(());
                }
                let _ = self.release.recv_timeout(Duration::from_secs(5));
            }
        }
        thread_local! {
            static EXIT_GATE: std::cell::RefCell<Option<ExitGate>> =
                const { std::cell::RefCell::new(None) };
        }
        let registry = WorkerRetirement::new(1);
        let lifetime = registry.reserve().unwrap();
        let original_lifetime = Arc::clone(&lifetime);
        let (entered, ready) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _lifetime = lifetime;
            EXIT_GATE.with(|gate| {
                *gate.borrow_mut() = Some(ExitGate {
                    entered: Some(entered),
                    release: held,
                });
            });
        });
        // Original+release ownership precedes readiness or any assertion.
        let mut owner = HeldWorker {
            handle: Some(handle),
            release: Some(release),
            registry: Arc::clone(&registry),
            lifetime: Some(original_lifetime),
        };
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            owner.handle.as_ref().unwrap().is_finished(),
            "controlled body returned while native TLS remains held"
        );
        assert!(!super::super::thread_signalled(
            owner.handle.as_ref().unwrap()
        ));
        let (exit, retained) = super::super::wait_for_worker(
            owner.handle.take().unwrap(),
            Instant::now() + Duration::from_millis(25),
        );
        owner.handle = retained;
        assert_eq!(exit, WorkerExit::TimedOut);
        registry.retain(
            owner.handle.take().unwrap(),
            Weak::new(),
            owner.lifetime.take().unwrap(),
        );
        let before = Instant::now();
        registry.reap();
        assert!(
            before.elapsed() < Duration::from_secs(1),
            "reaper must not wait on the held native TLS destructor"
        );
        assert_eq!(registry.retired.lock_recover().len(), 1);
        assert!(
            registry.reserve().is_err(),
            "unknown original still owns admission"
        );
        owner.release.take().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !registry.retired.lock_recover().is_empty() {
            registry.reap();
            assert!(
                Instant::now() < deadline,
                "released native original did not retire"
            );
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(*registry.admitted.lock_recover(), 0);
        drop(registry.reserve().unwrap());
    }

    #[test]
    fn retired_original_join_preserves_unverified_native_teardown_evidence() {
        for verified in [true, false] {
            let registry = WorkerRetirement::new(1);
            let lifetime = registry.reserve().unwrap();
            let slot = Arc::new(WorkerThreadState::default());
            slot.teardown_verified
                .store(verified, std::sync::atomic::Ordering::Release);
            let handle = thread::spawn(|| {});
            let owned = RetiredWorker {
                handle,
                slot: Arc::downgrade(&slot),
                lifetime,
            };
            WorkerRetirement::join_completed(vec![owned]);
            let expected = if verified {
                WorkerExit::Joined
            } else {
                WorkerExit::TeardownUnverified
            };
            assert!(
                matches!(*slot.lock_recover(), super::super::WorkerSlot::Stopped(exit) if exit == expected)
            );
            assert_eq!(*registry.admitted.lock_recover(), 0);
        }
    }

    #[test]
    fn original_retired_worker_is_acknowledged_and_admission_waits_for_decoder_lifetime() {
        let registry = WorkerRetirement::new(1);
        let lifetime = registry.reserve().unwrap();
        let decoder_lifetime = Arc::clone(&lifetime);
        let metadata_lifetime = Arc::clone(&lifetime);
        let worker_lifetime = Arc::clone(&lifetime);
        let (entered, ready) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let thread = thread::spawn(move || {
            let _lifetime = lifetime;
            entered.send(()).unwrap();
            let _ = held.recv_timeout(Duration::from_secs(5));
        });
        let mut worker = HeldWorker {
            handle: Some(thread),
            release: Some(release),
            registry: Arc::clone(&registry),
            lifetime: Some(worker_lifetime),
        };
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(registry
            .reserve()
            .err()
            .unwrap()
            .contains("capacity exhausted"));
        registry.retain(
            worker.handle.take().unwrap(),
            Weak::new(),
            worker.lifetime.take().unwrap(),
        );
        assert_eq!(registry.retired.lock_recover().len(), 1);
        worker.release.take().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !registry.retired.lock_recover().is_empty() {
            registry.reap();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(*registry.admitted.lock_recover(), 1);
        assert!(registry
            .reserve()
            .err()
            .unwrap()
            .contains("capacity exhausted"));
        drop(decoder_lifetime);
        assert_eq!(*registry.admitted.lock_recover(), 1);
        assert!(registry
            .reserve()
            .err()
            .unwrap()
            .contains("capacity exhausted"));
        drop(metadata_lifetime);
        assert_eq!(*registry.admitted.lock_recover(), 0);
        let fresh = registry.reserve().unwrap();
        drop(fresh);
    }

    #[test]
    fn owned_retired_worker_fixture_unwind_acknowledges_the_exact_original() {
        let registry = WorkerRetirement::new(1);
        let controlled = Arc::clone(&registry);
        let panic = std::panic::catch_unwind(move || {
            let lifetime = controlled.reserve().unwrap();
            let worker_lifetime = Arc::clone(&lifetime);
            let (entered, ready) = mpsc::channel();
            let (release, held) = mpsc::channel();
            let handle = thread::spawn(move || {
                let _lifetime = lifetime;
                entered.send(()).unwrap();
                let _ = held.recv_timeout(Duration::from_secs(5));
            });
            let _owner = HeldWorker {
                handle: Some(handle),
                release: Some(release),
                registry: controlled,
                lifetime: Some(worker_lifetime),
            };
            ready.recv_timeout(Duration::from_secs(1)).unwrap();
            panic!("controlled assertion unwind");
        });
        assert!(panic.is_err());
        assert!(registry.retired.lock_recover().is_empty());
        assert_eq!(*registry.admitted.lock_recover(), 0);
    }
}
