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
    #[cfg(any(not(windows), test))]
    joiner: Mutex<Option<JoinReaper>>,
}
impl Budget {
    pub const fn new(limit: usize) -> Self {
        Self {
            count: AtomicUsize::new(0),
            limit,
            #[cfg(any(not(windows), test))]
            joiner: Mutex::new(None),
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
    original: Mutex<Option<JoinHandle<()>>>,
    #[cfg(not(windows))]
    joining: Mutex<Option<JoinReceipt>>,
    resource: Option<Box<dyn Send>>,
    permit: Option<Reservation>,
}
static RETAINED: Mutex<Vec<Owned>> = Mutex::new(Vec::new());

// One persistent join reaper per Budget bounds helper threads independently
// of actor admission. It owns original handles only while joining; actor
// resources and reservations stay in Owned until the completion receipt.
#[cfg(any(not(windows), test))]
type OriginalSlot = std::sync::Arc<Mutex<Option<JoinHandle<()>>>>;
#[cfg(any(not(windows), test))]
type CompletionReceipt = std::sync::Arc<(Mutex<Option<bool>>, std::sync::Condvar)>;
#[cfg(any(not(windows), test))]
type RetainedFailure = std::sync::Arc<Mutex<Option<Box<dyn std::any::Any + Send>>>>;
#[cfg(any(not(windows), test))]
struct JoinTask {
    original: OriginalSlot,
    result: CompletionReceipt,
    failure: RetainedFailure,
}
#[cfg(any(not(windows), test))]
struct JoinReaper {
    queue: std::sync::mpsc::SyncSender<JoinTask>,
    // Intentionally process-lifetime, like the static admission Budget.
    // Never detach a new helper for each completed actor.
    _original: JoinHandle<()>,
}
#[cfg(any(not(windows), test))]
impl JoinReaper {
    fn start(capacity: usize) -> std::io::Result<Self> {
        Self::start_with(capacity, |run| {
            std::thread::Builder::new()
                .name("qc-original-reaper".into())
                .spawn(run)
        })
    }
    fn start_with(
        capacity: usize,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>,
    ) -> std::io::Result<Self> {
        let (queue, tasks) = std::sync::mpsc::sync_channel::<JoinTask>(capacity);
        let original = spawn(Box::new(move || {
            while let Ok(task) = tasks.recv() {
                let original = task
                    .original
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take();
                if let Some(original) = original {
                    let panicked = match original.join() {
                        Ok(()) => false,
                        Err(payload) => {
                            // Arbitrary payload destruction must also stay off
                            // deadline-bound callers. A stuck/panicking Drop
                            // leaves no receipt and retains actor admission.
                            if let Err(failure) =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    drop(payload)
                                }))
                            {
                                // Keep an arbitrary secondary payload owned
                                // and charged to this actor, never drop it
                                // on a bounded caller or kill the reaper.
                                *task.failure.lock().unwrap_or_else(|p| p.into_inner()) =
                                    Some(failure);
                                continue;
                            }
                            true
                        }
                    };
                    *task.result.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(panicked);
                    task.result.1.notify_all();
                }
            }
        }))?;
        Ok(Self {
            queue,
            _original: original,
        })
    }
}
#[cfg(any(not(windows), test))]
struct JoinReceipt {
    result: CompletionReceipt,
    // A failed reaper must not drop queued original handles with its receiver.
    _original: OriginalSlot,
    _failure: RetainedFailure,
}
#[cfg(any(not(windows), test))]
impl JoinReceipt {
    fn start(original: JoinHandle<()>, budget: &'static Budget) -> Result<Self, JoinHandle<()>> {
        Self::start_with(original, |task| {
            let mut reaper = budget.joiner.lock().unwrap_or_else(|p| p.into_inner());
            if reaper.is_none() {
                match JoinReaper::start(budget.limit) {
                    Ok(started) => *reaper = Some(started),
                    Err(_) => return Err(task),
                }
            }
            reaper
                .as_ref()
                .unwrap()
                .queue
                .try_send(task)
                .map_err(|error| match error {
                    std::sync::mpsc::TrySendError::Full(task)
                    | std::sync::mpsc::TrySendError::Disconnected(task) => task,
                })
        })
    }
    fn start_with(
        original: JoinHandle<()>,
        submit: impl FnOnce(JoinTask) -> Result<(), JoinTask>,
    ) -> Result<Self, JoinHandle<()>> {
        let slot = std::sync::Arc::new(Mutex::new(Some(original)));
        let result = std::sync::Arc::new((Mutex::new(None), std::sync::Condvar::new()));
        let failure = std::sync::Arc::new(Mutex::new(None));
        let task = JoinTask {
            original: std::sync::Arc::clone(&slot),
            result: std::sync::Arc::clone(&result),
            failure: std::sync::Arc::clone(&failure),
        };
        match submit(task) {
            Ok(()) => Ok(Self {
                result,
                _original: slot,
                _failure: failure,
            }),
            Err(_) => Err(slot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
                .unwrap()),
        }
    }
    fn ready(&self) -> bool {
        self.result
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }
    fn take_result(&self) -> bool {
        self.result
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap()
    }
    #[cfg(not(windows))]
    fn join(self) -> std::thread::Result<()> {
        let result = self.result.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut result = self
            .result
            .1
            .wait_while(result, |value| value.is_none())
            .unwrap_or_else(|p| p.into_inner());
        if result.take().unwrap() {
            Err(Box::new("An owned native actor panicked"))
        } else {
            Ok(())
        }
    }
}
impl Owned {
    /// Whether the exact original native thread is signalled (or was joined).
    /// Unlike JoinHandle::is_finished, this includes Windows TLS retirement.
    pub fn native_finished(&self) -> bool {
        #[cfg(not(windows))]
        {
            let mut joining = self.joining.lock().unwrap_or_else(|p| p.into_inner());
            if joining.is_none() {
                let mut slot = self.original.lock().unwrap_or_else(|p| p.into_inner());
                let Some(original) = slot.as_ref() else {
                    return true;
                };
                if !original.is_finished() {
                    return false;
                }
                match JoinReceipt::start(slot.take().unwrap(), self.permit.as_ref().unwrap().0) {
                    Ok(receipt) => *joining = Some(receipt),
                    Err(original) => {
                        *slot = Some(original);
                        return false;
                    }
                }
            }
            joining.as_ref().unwrap().ready()
        }
        #[cfg(windows)]
        self.original
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_none_or(crate::process::thread_signalled)
    }
    /// Explicit blocking join for an already-owned background actor. GUI or
    /// deadline-bound callers must use wait_within instead. Attached resources
    /// and admission remain owned until the actual original join completes.
    pub fn join_original(mut self) -> std::thread::Result<()> {
        let result = match self
            .original
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            Some(original) => original.join(),
            None => {
                #[cfg(not(windows))]
                if let Some(joining) = self
                    .joining
                    .get_mut()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    return joining.join();
                }
                Ok(())
            }
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
            original: Mutex::new(Some(original)),
            #[cfg(not(windows))]
            joining: Mutex::new(None),
            resource: Some(Box::new(resource)),
            permit: Some(permit),
        })
    }
    pub fn finished(&mut self) -> bool {
        #[cfg(not(windows))]
        {
            if !self.native_finished() {
                return false;
            }
            let Some(joining) = self
                .joining
                .get_mut()
                .unwrap_or_else(|p| p.into_inner())
                .take()
            else {
                return true;
            };
            let outcome = joining.take_result();
            drop(joining);
            drop((self.resource.take(), self.permit.take()));
            if outcome {
                eprintln!("An owned native actor retired after panic");
            }
            true
        }
        #[cfg(windows)]
        {
            let original = self.original.get_mut().unwrap_or_else(|p| p.into_inner());
            if original.is_none() {
                return true;
            }
            if !crate::process::thread_signalled(original.as_ref().unwrap()) {
                return false;
            }
            // The shared Windows helper checks the exact held OS thread object,
            // not Rust's earlier body-completion flag. Never join a pending one.
            let outcome = original.take().unwrap().join();
            let resource = self.resource.take();
            let permit = self.permit.take();
            // Disarm before any resource or diagnostic destructor can panic.
            drop((resource, permit));
            if outcome.is_err() {
                let _ = std::panic::catch_unwind(|| {
                    eprintln!("An owned native actor retired after panic")
                });
            }
            true
        }
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
        let complete = self
            .original
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .is_none();
        #[cfg(not(windows))]
        let complete = complete
            && self
                .joining
                .get_mut()
                .unwrap_or_else(|p| p.into_inner())
                .is_none();
        if complete {
            return;
        }
        let original = Self {
            original: Mutex::new(
                self.original
                    .get_mut()
                    .unwrap_or_else(|p| p.into_inner())
                    .take(),
            ),
            #[cfg(not(windows))]
            joining: Mutex::new(
                self.joining
                    .get_mut()
                    .unwrap_or_else(|p| p.into_inner())
                    .take(),
            ),
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
        let unfinished = original
            .original
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .is_some();
        #[cfg(not(windows))]
        let unfinished = unfinished
            || original
                .joining
                .get_mut()
                .unwrap_or_else(|p| p.into_inner())
                .is_some();
        if unfinished {
            pending.push(original);
        }
    }
    RETAINED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .extend(pending);
}

#[cfg(test)]
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
    fn join_receipt_waits_for_original_tls_teardown() {
        static JOIN_BUDGET: Budget = Budget::new(1);
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release));
        let original = std::thread::spawn(move || {
            TLS.with(|slot| {
                *slot.borrow_mut() = Some(Tls {
                    started,
                    release: release_rx,
                })
            });
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let receipt = JoinReceipt::start(original, &JOIN_BUDGET)
            .unwrap_or_else(|_| panic!("join reaper starts"));
        assert!(
            !receipt.ready(),
            "original join must not acknowledge blocked TLS"
        );
        drop(release);
        let until = Instant::now() + Duration::from_secs(2);
        while !receipt.ready() {
            assert!(
                Instant::now() < until,
                "original join did not acknowledge release"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!receipt.take_result());
    }
    #[test]
    fn join_observer_spawn_failure_returns_the_exact_original_handle() {
        let (release, released) = mpsc::channel();
        let release = Release(Some(release));
        let original = std::thread::spawn(move || {
            let _ = released.recv_timeout(Duration::from_secs(30));
        });
        let id = original.thread().id();
        let original = JoinReceipt::start_with(original, |task| {
            let reaper = JoinReaper::start_with(1, |_| {
                Err(std::io::Error::other("injected reaper spawn failure"))
            });
            assert!(reaper.is_err());
            Err(task)
        })
        .err()
        .expect("exact original is returned");
        assert_eq!(original.thread().id(), id);
        assert!(!original.is_finished());
        drop(release);
        original.join().unwrap();
    }
    #[test]
    fn completed_actors_reuse_one_budget_reaper_instead_of_spawning_more_helpers() {
        static JOIN_BUDGET: Budget = Budget::new(1);
        let mut reaper_id = None;
        for _ in 0..8 {
            let original = std::thread::spawn(|| {});
            let receipt = JoinReceipt::start(original, &JOIN_BUDGET)
                .unwrap_or_else(|_| panic!("join reaper admits next actor"));
            let id = JOIN_BUDGET
                .joiner
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                ._original
                .thread()
                .id();
            assert_eq!(*reaper_id.get_or_insert(id), id);
            let until = Instant::now() + Duration::from_secs(2);
            while !receipt.ready() {
                assert!(
                    Instant::now() < until,
                    "original join receipt did not arrive"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(!receipt.take_result());
        }
    }
    #[test]
    fn a_blocking_panic_payload_is_destroyed_before_join_receipt_publication() {
        static JOIN_BUDGET: Budget = Budget::new(1);
        struct Payload(mpsc::Sender<()>, mpsc::Receiver<()>);
        impl Drop for Payload {
            fn drop(&mut self) {
                let _ = self.0.send(());
                let _ = self.1.recv_timeout(Duration::from_secs(30));
            }
        }
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release));
        let original =
            std::thread::spawn(move || std::panic::panic_any(Payload(entered, release_rx)));
        let receipt = JoinReceipt::start(original, &JOIN_BUDGET)
            .unwrap_or_else(|_| panic!("join reaper starts"));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            !receipt.ready(),
            "payload destruction cannot block the receipt reader"
        );
        drop(release);
        let until = Instant::now() + Duration::from_secs(2);
        while !receipt.ready() {
            assert!(Instant::now() < until, "panic payload did not retire");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            receipt.take_result(),
            "original panic is retained as plain status"
        );
    }
    #[test]
    fn a_panicking_payload_is_retained_without_killing_the_budget_reaper() {
        static JOIN_BUDGET: Budget = Budget::new(1);
        struct BadDrop;
        impl Drop for BadDrop {
            fn drop(&mut self) {
                panic!("injected payload destructor panic");
            }
        }
        let original = std::thread::spawn(|| std::panic::panic_any(BadDrop));
        let receipt = JoinReceipt::start(original, &JOIN_BUDGET)
            .unwrap_or_else(|_| panic!("join reaper starts"));
        let until = Instant::now() + Duration::from_secs(2);
        while receipt._failure.lock().unwrap().is_none() {
            assert!(Instant::now() < until, "secondary payload was not retained");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !receipt.ready(),
            "uncertain payload cleanup cannot release actor admission"
        );
        assert!(
            receipt._original.lock().unwrap().is_none(),
            "the exact original was joined"
        );
        let next = JoinReceipt::start(std::thread::spawn(|| {}), &JOIN_BUDGET)
            .unwrap_or_else(|_| panic!("same reaper still accepts bounded queued work"));
        let until = Instant::now() + Duration::from_secs(2);
        while !next.ready() {
            assert!(Instant::now() < until, "reaper died after payload panic");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!next.take_result());
        assert_eq!(
            receipt
                ._failure
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .downcast_ref::<&str>(),
            Some(&"injected payload destructor panic")
        );
    }
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
