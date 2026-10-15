//! Nonblocking exact relay-task retirement. Four total original tasks are a
//! software resource budget, not a cloud/device protocol maximum.
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};
const KEEP: usize = 4;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static RETAINED: Mutex<Vec<Owned>> = Mutex::new(Vec::new());
pub(crate) struct Permit;
impl Permit {
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    pub(crate) fn acquire() -> Result<Self, String> {
        reap();
        ACTIVE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < KEEP).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| "Previous relay tasks are still retiring; no new host was started".into())
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(crate) struct Owned {
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    // Release only after the original task's successful ready-poll join, not
    // merely after abort or is_finished. Tests can attach an owned root here.
    resource: Option<Box<dyn Send>>,
    permit: Option<Permit>,
}
impl Owned {
    pub(crate) fn is_finished(&self) -> bool {
        self.task
            .as_ref()
            .is_none_or(|task| task.inner().is_finished())
    }
    pub(crate) fn new(
        task: tauri::async_runtime::JoinHandle<()>,
        resource: impl Send + 'static,
        permit: Permit,
    ) -> Self {
        Self {
            task: Some(task),
            resource: Some(Box::new(resource)),
            permit: Some(permit),
        }
    }
    pub(crate) fn retire(mut self) {
        self.task.as_ref().expect("owned original").abort();
        let original = Self {
            task: self.task.take(),
            resource: self.resource.take(),
            permit: self.permit.take(),
        };
        RETAINED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(original);
        reap();
    }
    fn join_ready(&mut self) -> bool {
        self.join_ready_with(|error| eprintln!("Relay task retired: {error}"))
    }
    fn join_ready_with(&mut self, report: impl FnOnce(String)) -> bool {
        if !self
            .task
            .as_ref()
            .is_some_and(|task| task.inner().is_finished())
        {
            return false;
        }
        let mut cx = Context::from_waker(Waker::noop());
        match Pin::new(self.task.as_mut().expect("owned original")).poll(&mut cx) {
            Poll::Ready(outcome) => {
                // Cancelled and panicked tasks are also acknowledged native
                // completion. The original has been joined, never detached.
                // First disarm the completed original and release its owned
                // resources. A diagnostic fault must never repoll it.
                self.task.take();
                self.resource.take();
                self.permit.take();
                if let Err(error) = outcome {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        report(error.to_string())
                    }));
                }
                true
            }
            Poll::Pending => false,
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if self.task.is_some() {
            self.task.as_ref().unwrap().abort();
            let original = Self {
                task: self.task.take(),
                resource: self.resource.take(),
                permit: self.permit.take(),
            };
            RETAINED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(original);
        }
    }
}
pub(crate) fn reap() {
    let originals = std::mem::take(&mut *RETAINED.lock().unwrap_or_else(|e| e.into_inner()));
    let mut pending = Vec::new();
    for mut original in originals {
        if !original.join_ready() {
            pending.push(original);
        }
    }
    RETAINED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(pending);
}
#[cfg(test)]
pub(crate) fn fixture_lock() -> std::sync::MutexGuard<'static, ()> {
    tests::FIXTURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    pub(super) static FIXTURES: Mutex<()> = Mutex::new(());
    struct Root(std::path::PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
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
    struct Gated {
        polled: Option<mpsc::Sender<()>>,
        dropping: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl Future for Gated {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            if let Some(sender) = self.polled.take() {
                let _ = sender.send(());
            }
            Poll::Pending
        }
    }
    impl Drop for Gated {
        fn drop(&mut self) {
            let _ = self.dropping.send(());
            // Always bounded, including assertion unwind in the caller.
            let _ = self.release.recv_timeout(Duration::from_secs(30));
        }
    }
    #[test]
    fn relay_stop_retains_original_host_lease_until_aborted_future_retires() {
        let _fixture = FIXTURES.lock().unwrap_or_else(|e| e.into_inner());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let root = Arc::new(Root(crate::tests::unique_fixture_directory(
            "relay-owned-task",
        )));
        let lease = crate::background_host::HostLease::try_acquire(&root.0, "relay-fixture.lock")
            .unwrap()
            .unwrap();
        let permit = Permit::acquire().unwrap();
        let (polled, started) = mpsc::channel();
        let (dropping, dropping_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        // Unwind releases before runtime shutdown waits for the blocked Drop.
        let release = Release(Some(release));
        let handle = tauri::async_runtime::JoinHandle::Tokio(runtime.spawn(Gated {
            polled: Some(polled),
            dropping,
            release: release_rx,
        }));
        let original = Owned::new(handle, (lease, Arc::clone(&root)), permit);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let before = Instant::now();
        original.retire();
        assert!(
            before.elapsed() < Duration::from_millis(500),
            "GUI stop never waits for Drop"
        );
        dropping_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        reap();
        assert!(
            crate::background_host::HostLease::try_acquire(&root.0, "relay-fixture.lock")
                .unwrap()
                .is_none(),
            "replacement cannot precede original retirement"
        );
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            reap();
            if crate::background_host::HostLease::try_acquire(&root.0, "relay-fixture.lock")
                .unwrap()
                .is_some()
            {
                break;
            }
            assert!(Instant::now() < deadline, "exact original did not join");
            std::thread::sleep(Duration::from_millis(5));
        }
        runtime.shutdown_timeout(Duration::from_secs(2));
    }
    #[test]
    fn relay_failed_publication_and_panicked_tasks_keep_admission_until_actual_join() {
        let _fixture = FIXTURES.lock().unwrap_or_else(|e| e.into_inner());
        // This fixture's local runtime owns all originals; no broker/device.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap();
        struct Receipt(Arc<AtomicUsize>);
        impl Drop for Receipt {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::AcqRel);
            }
        }
        let released = Arc::new(AtomicUsize::new(0));
        let mut originals = Vec::new();
        for _ in 0..KEEP {
            let permit = Permit::acquire().unwrap();
            let handle =
                tauri::async_runtime::JoinHandle::Tokio(runtime.spawn(std::future::pending()));
            originals.push(Owned::new(handle, Receipt(released.clone()), permit));
        }
        assert!(
            Permit::acquire().is_err(),
            "total original-owner cap is enforced before spawn"
        );
        drop(originals); // Same Drop path as failure to publish into task slot.
        let deadline = Instant::now() + Duration::from_secs(2);
        while released.load(Ordering::Acquire) != KEEP {
            reap();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let (panicking, entered) = mpsc::channel();
        let permit = Permit::acquire().unwrap();
        let handle = tauri::async_runtime::JoinHandle::Tokio(runtime.spawn(async move {
            let _ = panicking.send(());
            panic!("injected relay task failure");
        }));
        let original = Owned::new(handle, Receipt(released.clone()), permit);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(original);
        while released.load(Ordering::Acquire) != KEEP + 1 {
            reap();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        runtime.shutdown_timeout(Duration::from_secs(2));
    }
    #[test]
    fn relay_reporting_panic_cannot_retain_or_repoll_a_joined_original() {
        let _fixture = FIXTURES.lock().unwrap_or_else(|e| e.into_inner());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap();
        let permit = Permit::acquire().unwrap();
        let handle = tauri::async_runtime::JoinHandle::Tokio(runtime.spawn(async {
            panic!("injected task panic");
        }));
        let mut original = Owned::new(handle, (), permit);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !original.task.as_ref().unwrap().inner().is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(original.join_ready_with(|_| panic!("injected diagnostic panic")));
        assert!(
            original.task.is_none() && original.resource.is_none() && original.permit.is_none()
        );
        assert!(
            !original.join_ready(),
            "joined original is not polled twice"
        );
        drop(original);
        reap();
        runtime.shutdown_timeout(Duration::from_secs(2));
    }
}
