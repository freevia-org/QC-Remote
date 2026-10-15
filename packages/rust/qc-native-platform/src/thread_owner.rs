//! Original-thread ownership for long-lived application actors. Admission and
//! attached resources outlive the thread body, including native TLS teardown.
//! Stop requests remain the caller's responsibility; this does not force-kill
//! threads or give synchronous OS I/O a hard deadline.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct Budget {
    count: AtomicUsize,
    limit: usize,
}
impl Budget {
    pub const fn new(limit: usize) -> Self {
        Self {
            count: AtomicUsize::new(0),
            limit,
        }
    }
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    pub fn reserve(&'static self) -> Result<Reservation, String> {
        reap();
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.limit).then_some(n + 1)
            })
            .map(|_| Reservation(self))
            .map_err(|_| {
                "Previous native actors are still retiring; admission headroom exhausted".into()
            })
    }
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}
pub struct Reservation(&'static Budget);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Owned {
    original: Option<JoinHandle<()>>,
    resource: Option<Box<dyn Send>>,
    permit: Option<Reservation>,
}
static RETAINED: Mutex<Vec<Owned>> = Mutex::new(Vec::new());
impl Owned {
    /// Whether the exact original native thread is signalled (or was joined).
    /// Unlike JoinHandle::is_finished, this includes Windows TLS retirement.
    pub fn native_finished(&self) -> bool {
        self.original
            .as_ref()
            .is_none_or(crate::process::thread_signalled)
    }
    /// Explicit blocking join for an already-owned background actor. GUI or
    /// deadline-bound callers must use wait_within instead. Attached resources
    /// and admission remain owned until the actual original join completes.
    pub fn join_original(mut self) -> std::thread::Result<()> {
        let result = match self.original.take() {
            Some(original) => original.join(),
            None => Ok(()),
        };
        let resource = self.resource.take();
        let permit = self.permit.take();
        drop((resource, permit));
        result
    }
    pub fn spawn(
        name: &str,
        budget: &'static Budget,
        resource: impl Send + 'static,
        run: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        Self::spawn_reserved(name, budget.reserve()?, resource, run)
    }
    pub fn spawn_reserved(
        name: &str,
        permit: Reservation,
        resource: impl Send + 'static,
        run: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let original = std::thread::Builder::new()
            .name(name.into())
            .spawn(run)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            original: Some(original),
            resource: Some(Box::new(resource)),
            permit: Some(permit),
        })
    }
    pub fn finished(&mut self) -> bool {
        if self.original.is_none() {
            return true;
        }
        if !crate::process::thread_signalled(self.original.as_ref().unwrap()) {
            return false;
        }
        // The shared Windows helper checks the exact held OS thread object,
        // not Rust's earlier body-completion flag. Never join a pending one.
        let outcome = self.original.take().unwrap().join();
        let resource = self.resource.take();
        let permit = self.permit.take();
        // Disarm before any resource or diagnostic destructor can panic.
        drop((resource, permit));
        if outcome.is_err() {
            let _ =
                std::panic::catch_unwind(|| eprintln!("An owned native actor retired after panic"));
        }
        true
    }
    pub fn wait_within(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if self.finished() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(5)));
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if self.original.is_none() {
            return;
        }
        let original = Self {
            original: self.original.take(),
            resource: self.resource.take(),
            permit: self.permit.take(),
        };
        RETAINED
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(original);
    }
}
pub fn reap() {
    let originals =
        std::mem::take(&mut *RETAINED.lock().unwrap_or_else(|error| error.into_inner()));
    let mut pending = Vec::new();
    for mut original in originals {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| original.finished()));
        if original.original.is_some() {
            pending.push(original);
        }
    }
    RETAINED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .extend(pending);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{mpsc, Arc};
    static BUDGET: Budget = Budget::new(1);
    struct Receipt(Arc<AtomicUsize>);
    impl Drop for Receipt {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }
    struct Release(Option<mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    struct Tls {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl Drop for Tls {
        fn drop(&mut self) {
            let _ = self.started.send(());
            let _ = self.release.recv_timeout(Duration::from_secs(30));
        }
    }
    thread_local! { static TLS: RefCell<Option<Tls>> = const { RefCell::new(None) }; }
    #[test]
    fn an_actor_body_return_does_not_release_lease_or_admission_before_native_tls_exit() {
        let released = Arc::new(AtomicUsize::new(0));
        let (body, body_rx) = mpsc::channel();
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release));
        let mut owned = Owned::spawn(
            "owned-actor-tls",
            &BUDGET,
            Receipt(released.clone()),
            move || {
                TLS.with(|slot| {
                    *slot.borrow_mut() = Some(Tls {
                        started,
                        release: release_rx,
                    })
                });
                let _ = body.send(());
            },
        )
        .unwrap();
        body_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let before = Instant::now();
        assert!(!owned.finished());
        assert!(!owned.wait_within(Duration::from_millis(10)));
        assert!(before.elapsed() < Duration::from_millis(500));
        drop(owned);
        reap();
        assert_eq!(BUDGET.count(), 1);
        assert_eq!(released.load(Ordering::Acquire), 0);
        assert!(
            Owned::spawn("refused-actor", &BUDGET, (), || panic!("must not dispatch")).is_err()
        );
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while BUDGET.count() != 0 {
            reap();
            assert!(
                Instant::now() < deadline,
                "original native actor did not retire"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(released.load(Ordering::Acquire), 1);
    }
}
