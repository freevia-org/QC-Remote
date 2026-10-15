//! Deadline-bound awaiting of an exactly owned native actor. Dropping the
//! request does not cancel synchronous OS I/O or discard its original thread.
use qc_native_platform::thread_owner::{Budget, Owned};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

const QUEUED: u8 = 0;
const ADMITTED: u8 = 1;
const CANCELED: u8 = 2;
const CANCELED_AFTER_ADMISSION: u8 = 3;

#[derive(Clone)]
pub(crate) struct Context {
    state: Arc<AtomicU8>,
    deadline: Instant,
}
impl Context {
    pub(crate) fn current(&self) -> bool {
        self.state.load(Ordering::Acquire) <= ADMITTED && Instant::now() < self.deadline
    }
    /// Call under the operation's own serialization lock, immediately before
    /// dispatch. One CAS decides admission versus caller cancellation. This
    /// is ordinary monotonic admission, not a hard real-time guarantee.
    pub(crate) fn begin_dispatch(&self) -> bool {
        Instant::now() < self.deadline
            && self
                .state
                .compare_exchange(QUEUED, ADMITTED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    fn cancel(&self) {
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(match state {
                    QUEUED => CANCELED,
                    ADMITTED => CANCELED_AFTER_ADMISSION,
                    other => other,
                })
            });
    }
    fn admitted(&self) -> bool {
        matches!(
            self.state.load(Ordering::Acquire),
            ADMITTED | CANCELED_AFTER_ADMISSION
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Admission,
    Deadline,
    OriginalEnded,
}
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) kind: FailureKind,
    /// Admission is latched evidence, not proof that a device received bytes.
    /// A caller may conservatively classify a lost admitted mutation as
    /// ambiguous, but must never manufacture requestSent=false from timeout.
    pub(crate) admitted: bool,
    pub(crate) message: String,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
struct Running {
    context: Context,
    original: Option<Owned>,
}
impl Running {
    fn failure(&self, kind: FailureKind, message: impl Into<String>) -> Failure {
        // Cancel BEFORE sampling admission. A competing Queued->Admitted CAS
        // either already won and stays latched, or can no longer succeed.
        self.context.cancel();
        Failure {
            kind,
            admitted: self.context.admitted(),
            message: message.into(),
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.context.cancel();
        // Shared thread_owner retains unknown originals/permits until exact
        // native signal plus join, including native TLS after body return.
        drop(self.original.take());
    }
}

pub(crate) async fn run<T, E>(
    name: &str,
    budget: &'static Budget,
    deadline: Instant,
    action: impl FnOnce(Context) -> Result<T, E> + Send + 'static,
) -> Result<Result<T, E>, Failure>
where
    T: Send + 'static,
    E: Send + 'static,
{
    run_with_resource(name, budget, deadline, (), action).await
}

pub(crate) async fn run_with_resource<T, E, R>(
    name: &str,
    budget: &'static Budget,
    deadline: Instant,
    resource: R,
    action: impl FnOnce(Context) -> Result<T, E> + Send + 'static,
) -> Result<Result<T, E>, Failure>
where
    T: Send + 'static,
    E: Send + 'static,
    R: Send + 'static,
{
    if Instant::now() >= deadline {
        return Err(Failure {
            kind: FailureKind::Deadline,
            admitted: false,
            message: "Native operation deadline expired before admission".into(),
        });
    }
    let context = Context {
        state: Arc::new(AtomicU8::new(QUEUED)),
        deadline,
    };
    let worker_context = context.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let original = Owned::spawn(name, budget, resource, move || {
        // A deadline/cancel arriving before thread scheduling cannot invoke
        // the caller at all. Callers must check again after their own locks.
        if !worker_context.current() {
            return;
        }
        let result = action(worker_context);
        let _ = sender.send(result);
    })
    .map_err(|message| Failure {
        kind: FailureKind::Admission,
        admitted: false,
        message,
    })?;
    let mut running = Running {
        context,
        original: Some(original),
    };
    let result =
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), receiver).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(running.failure(
                FailureKind::OriginalEnded,
                "Native operation ended without a result",
            )),
            Err(_) => Err(running.failure(
                FailureKind::Deadline,
                "Native operation reached its total deadline",
            )),
        };
    // Nonblocking exact-original observation only. If TLS is still pending,
    // Drop transfers the original to the existing process-global reaper.
    if let Some(original) = running.original.as_mut() {
        original.finished();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct Release(Option<mpsc::Sender<()>>, &'static Budget);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
            let end = Instant::now() + Duration::from_secs(3);
            while self.1.count() != 0 && Instant::now() < end {
                qc_native_platform::thread_owner::reap();
                std::thread::sleep(Duration::from_millis(5));
            }
            // Unknown originals remain in the shared owner on fixture unwind;
            // never claim the bounded observation forced native exit.
        }
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn caller_cancellation_wins_queued_dispatch_and_keeps_the_original_permit() {
        static BUDGET: Budget = Budget::new(1);
        let runtime = runtime();
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release), &BUDGET);
        let (dispatched, dispatched_rx) = mpsc::channel();
        let request = runtime.spawn(run(
            "cancel-queued-original",
            &BUDGET,
            Instant::now() + Duration::from_secs(20),
            move |context| {
                let _ = entered.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(30));
                let admitted = context.begin_dispatch();
                let _ = dispatched.send(admitted);
                Ok::<_, String>(())
            },
        ));
        runtime.block_on(async { tokio::task::yield_now().await });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        request.abort();
        assert!(runtime.block_on(request).unwrap_err().is_cancelled());
        assert_eq!(
            BUDGET.count(),
            1,
            "Cancellation cannot abandon the started original"
        );
        let refused = runtime
            .block_on(run(
                "must-refuse-original",
                &BUDGET,
                Instant::now() + Duration::from_secs(2),
                |_| -> Result<(), String> {
                    panic!("Headroom refusal must precede callback dispatch")
                },
            ))
            .unwrap_err();
        assert_eq!(refused.kind, FailureKind::Admission);
        assert!(!refused.admitted);
        drop(release);
        assert!(!dispatched_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        assert_eq!(BUDGET.count(), 0);
    }

    #[test]
    fn admitted_operation_timeout_remains_distinct_from_predispatch_failure() {
        static BUDGET: Budget = Budget::new(1);
        let runtime = runtime();
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release), &BUDGET);
        let request = runtime.spawn(run(
            "admitted-timeout-original",
            &BUDGET,
            Instant::now() + Duration::from_secs(1),
            move |context| {
                assert!(context.begin_dispatch());
                let _ = started.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(30));
                Ok::<_, String>(())
            },
        ));
        runtime.block_on(async { tokio::task::yield_now().await });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let failure = runtime.block_on(request).unwrap().unwrap_err();
        assert_eq!(failure.kind, FailureKind::Deadline);
        assert!(
            failure.admitted,
            "Lost admitted work cannot advertise not-sent"
        );
        assert_eq!(BUDGET.count(), 1);
        drop(release);
        assert_eq!(BUDGET.count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn successful_body_does_not_release_admission_before_actual_native_tls_exit() {
        use std::cell::RefCell;
        static BUDGET: Budget = Budget::new(1);
        struct Resource(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let retired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        struct Tls(mpsc::Sender<()>, mpsc::Receiver<()>);
        impl Drop for Tls {
            fn drop(&mut self) {
                let _ = self.0.send(());
                let _ = self.1.recv_timeout(Duration::from_secs(30));
            }
        }
        thread_local! { static TLS: RefCell<Option<Tls>> = const { RefCell::new(None) }; }
        let (entered, entered_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release = Release(Some(release), &BUDGET);
        let result = runtime()
            .block_on(run_with_resource(
                "successful-native-tls",
                &BUDGET,
                Instant::now() + Duration::from_secs(5),
                Resource(retired.clone()),
                move |_| {
                    TLS.with(|slot| *slot.borrow_mut() = Some(Tls(entered, release_rx)));
                    Ok::<_, String>(7)
                },
            ))
            .unwrap()
            .unwrap();
        assert_eq!(result, 7);
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        qc_native_platform::thread_owner::reap();
        assert_eq!(
            BUDGET.count(),
            1,
            "Ready result is not native-thread retirement"
        );
        assert!(
            !retired.load(Ordering::Acquire),
            "attached resource cannot retire at body completion"
        );
        drop(release);
        assert_eq!(BUDGET.count(), 0);
        assert!(
            retired.load(Ordering::Acquire),
            "actual native join retires the attached resource"
        );
    }
}
