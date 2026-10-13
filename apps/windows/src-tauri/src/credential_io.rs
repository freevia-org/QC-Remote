//! Serialized, bounded execution for synchronous native credential-store calls.
//!
//! Keyring backends may block while the desktop unlocks a store. Keep those
//! calls off async and relay callback threads. Reads may time out at the caller;
//! writes wait for a definitive result because timing out a running OS write
//! would leave its final persisted value unknown.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

const SHARED_QUEUE_CAPACITY: usize = 16;

type Operation = Box<dyn FnOnce() + Send + 'static>;
type SubmittedRead<T> = (oneshot::Receiver<Result<T, String>>, Arc<AtomicU8>);
const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const CANCELLED: u8 = 2;

struct Job {
    state: Arc<AtomicU8>,
    operation: Operation,
}

/// One thread serializes the native keyring backend, with bounded admission.
pub(crate) struct CredentialIo {
    sender: SyncSender<Job>,
}

impl CredentialIo {
    fn new(capacity: usize) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(capacity);
        thread::Builder::new()
            .name("qc-credential-io".into())
            .spawn(move || worker(receiver))
            .map_err(|error| format!("Could not start credential-store worker: {error}"))?;
        Ok(Self { sender })
    }

    /// Submit a read and wait at most `timeout` for its result. If the caller
    /// times out or drops this future, a job still waiting in the queue is
    /// skipped. A native read already in progress is allowed to finish and its
    /// late result is discarded.
    pub(crate) async fn read<T, F>(&self, timeout: Duration, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        let (receiver, state) = self.submit(operation)?;
        let _cancel_on_drop = CancelOnDrop(state);
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Credential-store worker stopped before returning a read".into()),
            Err(_) => Err("Timed out reading the system credential store".into()),
        }
    }

    /// Submit a mutation and await the operation's definitive result. There is
    /// no execution timeout: if this future is dropped, only a not-yet-started
    /// job is skipped; an OS write already running continues to completion.
    pub(crate) async fn write<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        let (receiver, state) = self.submit(operation)?;
        let _cancel_on_drop = CancelOnDrop(state);
        receiver
            .await
            .map_err(|_| "Credential-store worker stopped before completing a write".to_string())?
    }

    /// Nonblocking admission for synchronous callback paths. The worker logs
    /// operation failures; callers must report this returned admission error.
    pub(crate) fn try_write<F>(&self, operation: F) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String> + Send + 'static,
    {
        let job = Job {
            state: Arc::new(AtomicU8::new(PENDING)),
            operation: Box::new(move || match catch_unwind(AssertUnwindSafe(operation)) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("Credential-store callback failed: {error}"),
                Err(_) => eprintln!("Credential-store callback panicked"),
            }),
        };
        self.enqueue(job)
    }

    fn submit<T, F>(&self, operation: F) -> Result<SubmittedRead<T>, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let state = Arc::new(AtomicU8::new(PENDING));
        let job = Job {
            state: Arc::clone(&state),
            operation: Box::new(move || {
                let result = match catch_unwind(AssertUnwindSafe(operation)) {
                    Ok(result) => result,
                    Err(_) => Err("Credential-store operation panicked".into()),
                };
                let _ = sender.send(result);
            }),
        };
        self.enqueue(job)?;
        Ok((receiver, state))
    }

    fn enqueue(&self, job: Job) -> Result<(), String> {
        match self.sender.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("Credential-store worker queue is full".into()),
            Err(TrySendError::Disconnected(_)) => {
                Err("Credential-store worker is unavailable".into())
            }
        }
    }
}

struct CancelOnDrop(Arc<AtomicU8>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        // Cancellation and worker start race on one state transition. Exactly
        // one wins: a queued job can be prevented from starting, while a job
        // already marked running is never misrepresented as cancelled.
        let _ = self
            .0
            .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
    }
}

fn worker(receiver: mpsc::Receiver<Job>) {
    while let Ok(job) = receiver.recv() {
        if job
            .state
            .compare_exchange(PENDING, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        (job.operation)();
    }
}

/// Process-wide worker shared by the GUI and background host.
pub(crate) fn shared() -> Result<&'static CredentialIo, String> {
    static SHARED: OnceLock<Result<CredentialIo, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| CredentialIo::new(SHARED_QUEUE_CAPACITY))
        .as_ref()
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    struct ReleaseOnDrop(Option<mpsc::Sender<()>>);

    impl ReleaseOnDrop {
        fn new(sender: mpsc::Sender<()>) -> Self {
            Self(Some(sender))
        }

        fn release(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.release();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delayed_read_times_out_and_its_late_result_is_discarded() {
        let io = CredentialIo::new(2).unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let mut release = ReleaseOnDrop::new(release_tx);
        let (started_tx, started_rx) = mpsc::channel();
        let reader = tokio::spawn(async move {
            io.read(Duration::from_millis(20), move || {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
                Ok("late secret".to_owned())
            })
            .await
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        let result = reader.await.unwrap();
        assert!(result.unwrap_err().contains("Timed out"));
        assert!(started.elapsed() < Duration::from_millis(500));
        release.release();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_read_is_skipped_after_its_caller_times_out() {
        let io = Arc::new(CredentialIo::new(2).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut release = ReleaseOnDrop::new(release_tx);
        let first_io = Arc::clone(&io);
        let first = tokio::spawn(async move {
            first_io
                .write(move || {
                    started_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                    Ok(())
                })
                .await
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let queued_called = Arc::clone(&called);
        let result = io
            .read(Duration::from_millis(20), move || {
                queued_called.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await;
        assert!(result.is_err());
        release.release();
        first.await.unwrap().unwrap();
        let (drained_tx, drained_rx) = mpsc::channel();
        io.try_write(move || {
            drained_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        drained_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn canceled_queued_write_is_skipped_before_start() {
        use std::future::Future;
        use std::task::{Context, Waker};

        let io = CredentialIo::new(2).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut release = ReleaseOnDrop::new(release_tx);
        io.try_write(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Ok(())
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let called = Arc::new(AtomicUsize::new(0));
        let queued_called = Arc::clone(&called);
        let mut queued = Box::pin(io.write(move || {
            queued_called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(queued.as_mut().poll(&mut context).is_pending());
        drop(queued); // The work is definitely enqueued but cannot have started.

        let (drained_tx, drained_rx) = mpsc::channel();
        release.release();
        io.try_write(move || {
            drained_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        drained_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_queue_rejects_admission_without_waiting() {
        let io = CredentialIo::new(1).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut release = ReleaseOnDrop::new(release_tx);
        io.try_write(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Ok(())
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        // Fill the sole queued slot, then require the next producer to return
        // immediately instead of blocking a Tauri or relay callback thread.
        let (drained_tx, drained_rx) = mpsc::channel();
        io.try_write(move || {
            drained_tx.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        let started_at = Instant::now();
        let full = io.try_write(|| Ok(())).unwrap_err();
        assert!(full.contains("queue is full"));
        assert!(started_at.elapsed() < Duration::from_millis(100));
        release.release();
        drained_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let after_drain = io.try_write(|| Ok(()));
        assert!(after_drain.is_ok());
        let _ = after_drain;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn operation_errors_and_panics_are_returned_without_killing_worker() {
        let io = CredentialIo::new(2).unwrap();
        assert_eq!(
            io.read::<(), _>(Duration::from_secs(1), || Err("locked".into()))
                .await,
            Err("locked".into())
        );
        assert_eq!(
            io.write::<(), _>(|| panic!("fixture panic")).await,
            Err("Credential-store operation panicked".into())
        );
        assert_eq!(io.read(Duration::from_secs(1), || Ok(7)).await, Ok(7));
    }
}
