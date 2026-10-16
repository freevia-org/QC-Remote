#![cfg(test)]

use super::*;
use crate::updater_relay::fetch::{FetchFailure, FetchRequest, FetchedHop, HttpFetcher};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;

#[test]
fn stale_private_cloud_frames_are_zeroized_before_discard() {
    for message_type in [18, 29, 30, 31, 41, 58, 61] {
        let mut message = IncomingMessage {
            observation: None,
            sequence: 1,
            message_type,
            payload: vec![0xA5; 32],
            received_at_unix_ms: 0,
        };

        assert!(!retain_message_for_current_epoch(&mut message, false));
        // Zeroize for Vec clears its length; assert that observable contract,
        // not a vacuous all() over the now-empty iterator or backing memory.
        assert!(message.payload.is_empty());
    }

    let mut ordinary = IncomingMessage {
        observation: None,
        sequence: 2,
        message_type: 13,
        payload: vec![0xA5; 32],
        received_at_unix_ms: 0,
    };
    assert!(!retain_message_for_current_epoch(&mut ordinary, false));
    assert_eq!(ordinary.payload, vec![0xA5; 32]);

    let mut current = IncomingMessage {
        observation: None,
        sequence: 3,
        message_type: 61,
        payload: vec![0xA5; 32],
        received_at_unix_ms: 0,
    };
    assert!(retain_message_for_current_epoch(&mut current, true));
    assert_eq!(current.payload, vec![0xA5; 32]);
}

#[test]
fn decoder_owner_cancels_when_its_bounded_stop_queue_is_full() {
    let retirement = WorkerRetirement::new(1);
    let lifetime = retirement.reserve().unwrap();
    let (messages, _receiver) = mpsc::sync_channel(1);
    messages.try_send(StateDecoderCommand::Reset).unwrap();
    let (finished, finished_rx) = mpsc::channel();
    let decoder =
        start_owned_decoder_with_lifetime("qc-test-owned-decoder", lifetime, move |stop| {
            while !stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            let _ = finished.send(());
        })
        .unwrap();

    // Stop cannot enter this full queue. The out-of-band stop flag must still
    // retire the owned thread instead of detaching it at shutdown.
    decoder.request_stop(&messages);
    assert!(matches!(
        messages.try_send(StateDecoderCommand::Stop),
        Err(mpsc::TrySendError::Full(StateDecoderCommand::Stop))
    ));
    drop(decoder);

    finished_rx
        .recv_timeout(WORKER_EXIT_BUDGET)
        .expect("decoder should observe cancellation and exit");
}

#[test]
fn discovery_fixtures_are_instance_scoped_and_preserve_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let empty = DeviceDiscovery {
        enumerate: Some(Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        })),
    };
    let unavailable = DeviceDiscovery {
        enumerate: Some(Arc::new(|| Err("injected HID enumeration failure".into()))),
    };
    assert!(empty.devices().unwrap().is_empty());
    assert!(empty.clone().devices().unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        unavailable.devices().unwrap_err(),
        "injected HID enumeration failure"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "another instance cannot use this callback"
    );
}

#[test]
fn a_continuously_replenished_command_queue_yields_after_one_bounded_batch() {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.send(0_usize).unwrap();
    let mut remaining = DEVICE_COMMAND_BATCH_LIMIT;
    let mut processed = Vec::new();
    let ready = Instant::now();
    while let Some(command) = next_command_in_batch(&receiver, &mut remaining, ready) {
        processed.push(command);
        sender.try_send(command + 1).unwrap();
    }
    assert_eq!(
        processed,
        (0..DEVICE_COMMAND_BATCH_LIMIT).collect::<Vec<_>>()
    );
    assert_eq!(remaining, 0);
    // The producer never left an empty queue: termination is the service bound,
    // leaving the next item for the next outer iteration's inbound/clock work.
    assert_eq!(receiver.try_recv().unwrap(), DEVICE_COMMAND_BATCH_LIMIT);
}

#[test]
fn command_batch_boundaries_preserve_disconnect_stop_fifo_and_settle_admission() {
    let (sender, receiver) = mpsc::sync_channel(DEVICE_COMMAND_BATCH_LIMIT + 2);
    for index in 0..DEVICE_COMMAND_BATCH_LIMIT {
        sender
            .send(Command::RecordFlight {
                event: index.to_string(),
                incident: false,
            })
            .unwrap();
    }
    let (reply, _response) = mpsc::channel();
    sender
        .send(Command::Disconnect {
            reply,
            _admission: None,
        })
        .unwrap();
    sender.send(Command::Stop).unwrap();
    let mut remaining = DEVICE_COMMAND_BATCH_LIMIT;
    assert!(next_command_in_batch(
        &receiver,
        &mut remaining,
        Instant::now() + Duration::from_secs(60)
    )
    .is_none());
    assert_eq!(remaining, DEVICE_COMMAND_BATCH_LIMIT);
    let ready = Instant::now();
    for index in 0..DEVICE_COMMAND_BATCH_LIMIT {
        let Some(Command::RecordFlight { event, .. }) =
            next_command_in_batch(&receiver, &mut remaining, ready)
        else {
            panic!("FIFO flight command was skipped");
        };
        assert_eq!(event, index.to_string());
    }
    assert!(next_command_in_batch(&receiver, &mut remaining, ready).is_none());
    remaining = DEVICE_COMMAND_BATCH_LIMIT;
    assert!(matches!(
        next_command_in_batch(&receiver, &mut remaining, ready),
        Some(Command::Disconnect { .. })
    ));
    assert!(matches!(
        next_command_in_batch(&receiver, &mut remaining, ready),
        Some(Command::Stop)
    ));
    assert!(next_command_in_batch(&receiver, &mut remaining, ready).is_none());
}

/// Release a deliberately stalled fixture even when an assertion unwinds.
struct ReleaseOnDrop(Arc<AtomicBool>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

const FIXTURE_OWNER_LIMIT: usize = 32;

trait RetiredFixture: Send {
    fn finished(&self) -> bool;
    fn join(self: Box<Self>);
}

struct FixtureAdmissions {
    limit: usize,
    count: std::sync::atomic::AtomicUsize,
    retained: Mutex<Vec<Box<dyn RetiredFixture>>>,
}
struct FixturePermit(Arc<FixtureAdmissions>);
impl Drop for FixturePermit {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
    }
}
impl FixtureAdmissions {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            count: std::sync::atomic::AtomicUsize::new(0),
            retained: Mutex::new(Vec::new()),
        })
    }
    fn global() -> Arc<Self> {
        static OWNER: std::sync::OnceLock<Arc<FixtureAdmissions>> = std::sync::OnceLock::new();
        Arc::clone(OWNER.get_or_init(|| Self::new(FIXTURE_OWNER_LIMIT)))
    }
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    fn reserve(self: &Arc<Self>) -> Result<Arc<FixturePermit>, &'static str> {
        self.reap();
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.limit).then_some(count + 1)
            })
            .map_err(|_| "fixture owner capacity exhausted")?;
        Ok(Arc::new(FixturePermit(Arc::clone(self))))
    }
    fn reap(&self) {
        // Both the predicate (which may reap a worker) and exact joins run
        // outside this mutex. Admission stays held while records are taken.
        let records = std::mem::take(&mut *self.retained.lock_recover());
        let mut pending = Vec::new();
        for record in records {
            if record.finished() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| record.join()));
            } else {
                pending.push(record);
            }
        }
        self.retained.lock_recover().extend(pending);
    }
    fn retain(&self, record: Box<dyn RetiredFixture>) {
        self.reap();
        if record.finished() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| record.join()));
        } else {
            self.retained.lock_recover().push(record);
        }
    }
}

struct RetiredFixtureThread<T: Send + 'static> {
    handle: thread::JoinHandle<T>,
    _permit: Arc<FixturePermit>,
}
impl<T: Send + 'static> RetiredFixture for RetiredFixtureThread<T> {
    fn finished(&self) -> bool {
        crate::platform::process::thread_signalled(&self.handle)
    }
    fn join(self: Box<Self>) {
        let _ = self.handle.join();
    }
}

pub(super) struct FixtureThread<T: Send + 'static> {
    handle: Option<thread::JoinHandle<T>>,
    permit: Option<Arc<FixturePermit>>,
}
impl<T: Send + 'static> FixtureThread<T> {
    pub(super) fn spawn(run: impl FnOnce() -> T + Send + 'static) -> Self {
        Self::spawn_in(FixtureAdmissions::global(), run)
    }
    fn spawn_in(
        admissions: Arc<FixtureAdmissions>,
        run: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        // Admission and original ownership precede every readiness/assertion.
        let permit = admissions.reserve().expect("fixture thread admission");
        let handle = thread::Builder::new()
            .spawn(run)
            .expect("fixture thread spawn");
        Self {
            handle: Some(handle),
            permit: Some(permit),
        }
    }
    pub(super) fn join(mut self) -> T {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !crate::platform::process::thread_signalled(self.handle.as_ref().unwrap()) {
            assert!(Instant::now() < deadline, "fixture thread did not retire");
            thread::sleep(Duration::from_millis(1));
        }
        let outcome = self.handle.take().unwrap().join();
        self.permit.take();
        outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }
    fn retire_within(&mut self, budget: Duration) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let deadline = Instant::now() + budget;
        while !crate::platform::process::thread_signalled(&handle) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        let permit = self.permit.take().unwrap();
        if crate::platform::process::thread_signalled(&handle) {
            let _ = handle.join();
            drop(permit);
        } else {
            let admissions = Arc::clone(&permit.0);
            admissions.retain(Box::new(RetiredFixtureThread {
                handle,
                _permit: permit,
            }));
            eprintln!("fixture thread did not acknowledge exit; bounded original owner retained");
        }
    }
}
impl<T: Send + 'static> Drop for FixtureThread<T> {
    fn drop(&mut self) {
        self.retire_within(Duration::from_secs(2));
    }
}

/// Temporary bytes outlive the exact worker and every cloned native lease.
struct WorkerRootGuard {
    shutdown: WorkerShutdown,
    root: Arc<crate::device_lease::testing::FixtureRoot>,
    permit: Option<Arc<FixturePermit>>,
}
struct RetiredWorkerRoot {
    shutdown: WorkerShutdown,
    root: Arc<crate::device_lease::testing::FixtureRoot>,
    _permit: Arc<FixturePermit>,
}
impl RetiredFixture for RetiredWorkerRoot {
    fn finished(&self) -> bool {
        self.shutdown.fixture_original_acknowledged()
            && !crate::device_lease::testing::root_has_lease_activity(&self.root.0)
    }
    fn join(self: Box<Self>) {
        drop(self);
    }
}
impl Drop for WorkerRootGuard {
    fn drop(&mut self) {
        let outcome = self.shutdown.stop_and_join(WORKER_EXIT_BUDGET);
        let permit = self.permit.take().expect("fixture root admission");
        let admissions = Arc::clone(&permit.0);
        let record = RetiredWorkerRoot {
            shutdown: self.shutdown.clone(),
            root: Arc::clone(&self.root),
            _permit: permit,
        };
        if record.finished() {
            drop(record);
        } else {
            admissions.retain(Box::new(record));
            eprintln!("fixture worker exit {outcome:?}; bounded exact root owner retained");
        }
    }
}

struct FixtureSweepOnDrop(Arc<FixtureAdmissions>);
impl Drop for FixtureSweepOnDrop {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            self.0.reap();
            if self.0.count.load(Ordering::Acquire) == 0 || Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn retained_fixture_root_waits_for_exact_worker_and_native_lease_retirement() {
    let admissions = FixtureAdmissions::new(1);
    let _cleanup = FixtureSweepOnDrop(Arc::clone(&admissions));
    let permit = admissions.reserve().unwrap();
    let root = crate::device_lease::testing::fresh_root("native-root-retirement");
    let path = root.0.clone();
    let mut lease =
        crate::device_lease::LeaseHolder::new(path.join("device-owner.json"), "fixture".into());
    lease.acquire().unwrap();
    let native = lease.io_owner().unwrap();
    lease.release();
    let wire = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&wire);
    let controller = controller_with_worker(move |receiver| fake_run(receiver, recorded));
    let guard = WorkerRootGuard {
        shutdown: controller.shutdown_handle(),
        root: Arc::clone(&root),
        permit: Some(permit),
    };
    drop(guard);
    drop(controller);
    drop(root);
    assert!(
        path.exists(),
        "an exact native lease still owns these bytes"
    );
    assert!(admissions.reserve().is_err());
    drop(native);
    admissions.reap();
    assert!(
        !path.exists(),
        "actual native retirement releases the retained root"
    );
    assert_eq!(admissions.count.load(Ordering::Acquire), 0);
}

#[test]
fn fixture_admission_waits_for_original_join_and_reaps_after_acknowledgement() {
    let admissions = FixtureAdmissions::new(1);
    let _cleanup = FixtureSweepOnDrop(Arc::clone(&admissions));
    let (entered, ready) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let mut owner = FixtureThread::spawn_in(Arc::clone(&admissions), move || {
        entered.send(()).unwrap();
        let _ = held.recv_timeout(Duration::from_secs(2));
    });
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    owner.retire_within(Duration::ZERO);
    assert!(
        admissions.reserve().is_err(),
        "retention still consumes admission"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while admissions.count.load(Ordering::Acquire) != 0 {
        admissions.reap();
        assert!(
            Instant::now() < deadline,
            "controlled original did not acknowledge"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(admissions.retained.lock_recover().is_empty());
    let fresh = admissions.reserve().unwrap();
    drop(fresh);
}

#[test]
fn fixture_unwind_releases_its_stall_and_joins_the_admitted_thread() {
    let release = Arc::new(AtomicBool::new(false));
    let released = Arc::clone(&release);
    let completed = Arc::new(AtomicBool::new(false));
    let worker_completed = Arc::clone(&completed);
    let worker = FixtureThread::spawn(move || {
        while !released.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        worker_completed.store(true, Ordering::Release);
    });
    let outcome = std::panic::catch_unwind(|| {
        let _release_on_unwind = ReleaseOnDrop(release);
        panic!("injected stalled fixture assertion failure");
    });
    assert!(outcome.is_err());
    worker.join();
    assert!(completed.load(Ordering::Acquire));
}

#[test]
fn fixture_root_unwind_waits_for_its_worker_before_removing_owned_bytes() {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let mut path = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let permit = FixtureAdmissions::global()
            .reserve()
            .expect("fixture root admission");
        let root = crate::device_lease::testing::fresh_root("worker-root-unwind");
        path = Some(root.0.clone());
        std::fs::write(root.0.join("owned"), b"fixture").unwrap();
        let recorded = Arc::clone(&wire);
        let controller = controller_with_worker(move |receiver| fake_run(receiver, recorded));
        let _root_guard = WorkerRootGuard {
            shutdown: controller.shutdown_handle(),
            root,
            permit: Some(permit),
        };
        panic!("injected worker fixture assertion failure");
    }));
    assert!(outcome.is_err());
    assert_eq!(*wire.lock_recover(), ["connection(false)"]);
    assert!(!path.unwrap().exists());
}

fn wait_for_status(
    controller: &DeviceController,
    what: &str,
    mut accept: impl FnMut(&BrokerStatus) -> bool,
) -> BrokerStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = controller.status();
        if accept(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; last status {} / {}",
            status.phase,
            status.detail
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// The contested start from the live test: a hardware broker holds the
/// unit and a second opener auto-connects. It must defer with the visible
/// waiting text, retry without opening anything, leave the owner's record
/// alone, and proceed by itself once the owner is gone.
#[test]
fn a_contested_start_waits_for_the_owner_and_takes_over_when_it_exits() {
    use crate::device_lease::testing::{write_record, Sleeper};
    let permit = FixtureAdmissions::global()
        .reserve()
        .expect("fixture root admission");
    let root = crate::device_lease::testing::fresh_root("contested");
    let path = root.0.join("device-owner.json");
    let mut owner = Sleeper::start();
    write_record(&path, &owner.record());
    let record_bytes = std::fs::read(&path).unwrap();

    // A device id that matches nothing, so this test can never open a real
    // Quad Cortex even if the gate under test regressed.
    let discoveries = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&discoveries);
    let controller = DeviceController::start_for_device_with_test_discovery(
        Some("no-such-device".into()),
        true,
        path.clone(),
        move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        },
    )
    .expect("controller starts");
    let _root_guard = WorkerRootGuard {
        shutdown: controller.shutdown_handle(),
        root,
        permit: Some(permit),
    };

    let expected = format!(
            "The Quad Cortex is in use by a hardware or conformance run (device broker --stdio) \
             (process {}); waiting for it to let go - QC Remote connects by itself as soon as it does",
            owner.pid()
        );
    let waiting = wait_for_status(&controller, "the waiting status", |status| {
        status.detail == expected
    });
    assert_eq!(waiting.phase, "searching");
    assert!(!waiting.connected);
    // Several retry cadences pass (RECONNECT_INTERVAL_MS is 750 ms) and it
    // is still waiting, still searching, and the record is untouched.
    thread::sleep(Duration::from_millis(2_000));
    let still = controller.status();
    assert_eq!(still.detail, expected);
    assert_eq!(std::fs::read(&path).unwrap(), record_bytes);
    assert_eq!(
        discoveries.load(Ordering::SeqCst),
        0,
        "a live lease forbids discovery"
    );

    owner.kill();
    let taken = wait_for_status(&controller, "the takeover", |status| {
        status.detail != expected
    });
    assert!(
        !taken.detail.contains("in use by"),
        "after the owner died: {}",
        taken.detail
    );
    assert!(discoveries.load(Ordering::SeqCst) > 0);
    drop(controller);
}

#[test]
fn synchronous_write_waits_for_every_native_frame_after_queue_admission() {
    use quadcortex::device::delivery::ReportOutcome;
    let controller = DeviceController::start_disconnected().unwrap();
    let (operation, frames, events) = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "delivery-test".into(),
            0,
        ));
        let operation = owner.plan_operation(0, 1_000).unwrap();
        let events = owner.watch_deliveries().unwrap();
        let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
        let first = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        let second = owner.prepare_delivery(&dispatch, 4, 1, 0).unwrap();
        (operation, [first, second], events)
    };
    let (admitted, admission) = mpsc::channel();
    admitted.send(Ok(())).unwrap();
    let (done, result) = mpsc::channel();
    thread::scope(|scope| {
        scope.spawn(|| {
            done.send(controller.wait_for_native_write(&operation, 2, admission, events))
                .unwrap();
        });
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        {
            let mut owner = controller.session_owner.lock_recover();
            owner.begin_native_report(&frames[0], 0, 25).unwrap();
            owner
                .finish_native_report(&frames[0], 0, ReportOutcome::Delivered, 26)
                .unwrap();
        }
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        {
            let mut owner = controller.session_owner.lock_recover();
            owner.begin_native_report(&frames[1], 0, 50).unwrap();
            owner
                .finish_native_report(&frames[1], 0, ReportOutcome::ExpectedStatusStall, 51)
                .unwrap();
        }
        assert_eq!(result.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(()));
    });
}

fn timed_observation(first: u64, last: u64) -> ObservationTicket {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner
        .capture_observation_with_ingress(
            connection,
            Some(quadcortex::device::transport::IngressTime {
                first_report_at_ms: first,
                last_report_at_ms: last,
            }),
        )
        .unwrap()
}

fn test_read_admission(epoch: u64, clock: Instant, budget: Duration) -> RequestAdmission {
    let mut owner = SessionRuntime::new(0);
    owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        epoch,
        "read-session".into(),
        0,
    ));
    RequestAdmission {
        epoch,
        operation: owner.plan_operation(0, budget.as_millis() as u64).unwrap(),
        clock,
    }
}

fn metadata_payload(name: &str) -> Vec<u8> {
    pa::ModelRepoMessage {
            action: pa::message_action::Enum::Update as i32,
            model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
                format!("<Models><Category name=\"Drive\"><Model id=\"7\" name=\"{name}\" /></Category></Models>").into_bytes(),
            )),
            ..Default::default()
        }.encode_to_vec()
}

fn decoder_lane(
    owner: &Arc<Mutex<SessionRuntime>>,
) -> (mpsc::SyncSender<StateDecoderCommand>, DecoderThreadOwner) {
    let shared = StateDecoderShared {
        stream_id: "test-stream".into(),
        session_owner: Arc::clone(owner),
        event_log: Arc::new(Mutex::new(StateEventJournal::default())),
        latest_tempo_clock: Arc::new(Mutex::new(None)),
        latest_master_volume: Arc::new(Mutex::new(None)),
        latest_sequence: Arc::new(AtomicU64::new(0)),
        subscribers: Arc::new(Mutex::new(Vec::new())),
    };
    let (sender, receiver) = mpsc::sync_channel(16);
    let results = sender.clone();
    let worker = start_owned_decoder("fixture-state-decoder", move |stop| {
        run_state_decoder_with_stop(shared, results, receiver, Instant::now(), stop)
    })
    .unwrap();
    (sender, worker)
}

impl DecoderThreadOwner {
    /// Preserve the queued Stop test ordering; unwind still uses the owner's stop flag.
    fn join(mut self) -> thread::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !crate::platform::process::thread_signalled(self.thread.as_ref().unwrap()) {
            assert!(Instant::now() < deadline, "fixture decoder did not retire");
            thread::sleep(Duration::from_millis(1));
        }
        self.thread.take().unwrap().join()
    }
}

#[test]
fn decoder_owner_retires_its_sender_cycle_when_the_next_spawn_is_refused() {
    let owner = Arc::new(Mutex::new(SessionRuntime::new(0)));
    let (sender, worker) = decoder_lane(&owner);
    // This mirrors a failed USB-worker admission: the closure owning the
    // already-running decoder is dropped rather than executed.
    let rejected_next_spawn = move || drop(worker);
    let started = Instant::now();
    drop(rejected_next_spawn);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(
        sender.try_send(StateDecoderCommand::StateWake),
        Err(mpsc::TrySendError::Disconnected(_))
    ));
}

#[test]
fn decoder_stop_flag_retires_a_full_queue_after_a_stalled_receiver_is_released() {
    let (sender, receiver) = mpsc::sync_channel(1);
    sender.try_send(StateDecoderCommand::StateWake).unwrap();
    let (entered, entered_rx) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stopped = Arc::clone(&stopped);
    let worker = start_owned_decoder("fixture-stalled-decoder", move |stop| {
        entered.send(()).unwrap();
        // A bounded independent gate cannot strand this fixture on unwind.
        let _ = released.recv_timeout(Duration::from_secs(1));
        worker_stopped.store(stop.load(Ordering::Acquire), Ordering::Release);
        drop(receiver);
    })
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    worker.request_stop(&sender);
    assert!(worker.stop.load(Ordering::Acquire));
    release.send(()).unwrap();
    worker.join().unwrap();
    assert!(stopped.load(Ordering::Acquire));
    assert!(matches!(
        sender.try_send(StateDecoderCommand::StateWake),
        Err(mpsc::TrySendError::Disconnected(_))
    ));
}

#[test]
fn transport_destructor_completes_before_its_lease_is_released_on_return_or_unwind() {
    struct Lease(Arc<AtomicBool>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    struct Transport {
        entered: mpsc::Sender<()>,
        released: mpsc::Receiver<()>,
        completed: Arc<AtomicBool>,
    }
    impl Drop for Transport {
        fn drop(&mut self) {
            let _ = self.entered.send(());
            if self.released.recv_timeout(Duration::from_secs(1)).is_ok() {
                self.completed.store(true, Ordering::Release);
            }
        }
    }
    for unwind in [false, true] {
        let lease_dropped = Arc::new(AtomicBool::new(false));
        let transport_completed = Arc::new(AtomicBool::new(false));
        let (entered, entered_rx) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let lease = Lease(Arc::clone(&lease_dropped));
        let completed = Arc::clone(&transport_completed);
        let worker = FixtureThread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_transport_lease(lease, |_| {
                    let _transport = Transport {
                        entered,
                        released,
                        completed,
                    };
                    if unwind {
                        panic!("injected transport owner unwind");
                    }
                });
            }))
            .is_err()
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!lease_dropped.load(Ordering::Acquire));
        release.send(()).unwrap();
        assert_eq!(worker.join(), unwind);
        assert!(transport_completed.load(Ordering::Acquire));
        assert!(lease_dropped.load(Ordering::Acquire));
    }
}

#[test]
fn backup_completion_cannot_adopt_a_replacement_ready_session() {
    let (controller, receiver) = refresh_controller();
    let owner = Arc::clone(&controller.session_owner);
    let state = Arc::clone(&controller.state);
    let worker =
        FixtureThread::spawn(move || {
            let Command::CreateBackup { reply, .. } =
                receiver.recv_timeout(Duration::from_secs(1)).unwrap()
            else {
                panic!("expected a single backup request")
            };
            owner.lock_recover().lifecycle = Some(
                quadcortex::device::session::SessionDriver::start(8, "replacement".into(), 1),
            );
            state.lock_recover().phase = "ready".into();
            reply
                .send(Ok("completed original document".into()))
                .unwrap();
        });
    let error = controller
        .create_backup(Duration::from_secs(1))
        .unwrap_err();
    assert!(error.contains("session changed"), "{error}");
    worker.join();
}

#[test]
fn dialog_bound_backup_refuses_a_reconnected_session_before_queueing() {
    let (controller, receiver) = refresh_controller();
    let context = controller.backup_context().unwrap();
    controller.session_owner.lock_recover().lifecycle = Some(
        quadcortex::device::session::SessionDriver::start(8, "replacement".into(), 1),
    );
    let error = controller
        .create_backup_in_context(Duration::from_secs(1), Some(&context))
        .unwrap_err();
    assert!(error.contains("session changed"), "{error}");
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn dialog_bound_backup_refuses_another_device_before_queueing() {
    let (original, _original_receiver) = refresh_controller();
    let context = original.backup_context().unwrap();
    let (selected, receiver) = refresh_controller();
    selected.session_owner.lock_recover().lifecycle = Some(
        quadcortex::device::session::SessionDriver::start(7, "another-device".into(), 0),
    );
    let error = selected
        .create_backup_in_context(Duration::from_secs(1), Some(&context))
        .unwrap_err();
    assert!(error.contains("session changed"), "{error}");
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn backup_completion_preserves_the_original_document_and_transfer_budget() {
    let (controller, receiver) = refresh_controller();
    let owner = Arc::clone(&controller.session_owner);
    let state = Arc::clone(&controller.state);
    let clock = controller.session_clock;
    let worker = FixtureThread::spawn(move || {
        let Command::CreateBackup { admission, reply } =
            receiver.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected a single backup request")
        };
        let now = clock.elapsed().as_millis() as u64;
        owner
            .lock_recover()
            .validate_operation(&admission, now)
            .unwrap();
        assert!(admission.remaining_ms(now) <= 1000);
        state.lock_recover().phase = "ready".into();
        reply
            .send(Ok("completed original document".into()))
            .unwrap();
    });
    assert_eq!(
        controller.create_backup(Duration::from_secs(1)).unwrap(),
        "completed original document"
    );
    worker.join();
}

#[test]
fn ready_status_does_not_override_an_expired_recovery_scope() {
    let (controller, _receiver) = refresh_controller();
    let context = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    let expired = controller
        .begin_gateway_read_scope_for(&context, 0)
        .unwrap();
    controller.state.lock_recover().phase = "ready".into();
    assert!(controller
        .wait_for_ready_in_scope(&expired, Duration::from_secs(1))
        .unwrap_err()
        .contains("budget expired"));
}

pub(super) fn refresh_controller() -> (DeviceController, mpsc::Receiver<Command>) {
    let mut owner = SessionRuntime::new(0);
    owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        7,
        "original".into(),
        0,
    ));
    let (commands, receiver) = mpsc::sync_channel(4);
    (
        DeviceController {
            discovery: DeviceDiscovery::default(),
            device_id: Arc::new(Mutex::new(None)),
            identity_observation_enabled: Arc::new(AtomicBool::new(false)),
            state_stream_id: "test-stream".into(),
            state: Default::default(),
            latest_messages: Default::default(),
            raw_events: Default::default(),
            state_event_log: Default::default(),
            latest_tempo_clock: Default::default(),
            latest_master_volume: Default::default(),
            state_event_sequence: Default::default(),
            state_subscribers: Default::default(),
            session_owner: Arc::new(Mutex::new(owner)),
            session_clock: Instant::now(),
            commands,
            updater_relay: Arc::new(UpdaterRelay::new()),
            touch_guard: Default::default(),
            connect_interrupt: ConnectInterrupt::default(),
            worker: Default::default(),
        },
        receiver,
    )
}

/// Runs `work` on its own thread and fails the test if it does not finish:
/// the property under test is "does not block", and a regression would
/// otherwise hang the whole suite instead of failing it.
fn finishes_within<T: Send + 'static>(
    budget: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (done, finished) = mpsc::channel();
    let worker = FixtureThread::spawn(move || {
        let _ = done.send(work());
    });
    let result = finished
        .recv_timeout(budget)
        .expect("the call blocked instead of returning");
    worker.join();
    result
}

#[test]
fn diagnostics_never_block_a_request_while_the_worker_is_inside_a_connect() {
    // The worker drains nothing during a connect, so the bounded queue
    // fills; `handle` records a flight event for every RPC, status polls
    // included. A blocking send there parked the request thread for good,
    // which is what made `system.status` unanswerable during the wedge.
    let (controller, receiver) = refresh_controller();
    let controller = Arc::new(controller);
    let caller = Arc::clone(&controller);
    finishes_within(Duration::from_secs(5), move || {
        for index in 0..1_000 {
            caller.record_flight_event(format!("rpc-start-{index}-system.status"));
        }
        caller.quarantine_writes_for("device.selectScene", "unconfirmed write");
        let _status = caller.status();
    });
    assert_eq!(
        std::iter::from_fn(|| receiver.try_recv().ok()).count(),
        4,
        "the queue holds its capacity and the rest were dropped"
    );
}

#[test]
fn a_forced_reset_interrupts_a_running_connect_and_is_queued_behind_nothing() {
    let (controller, receiver) = refresh_controller();
    let interrupted = controller.connect_interrupt.watch();
    assert!(!interrupted());
    let controller = Arc::new(controller);
    let caller = Arc::clone(&controller);
    let call = FixtureThread::spawn(move || caller.reset_session());
    match receiver.recv_timeout(Duration::from_secs(2)).unwrap() {
        Command::Reconnect { force, reply } => {
            assert!(force);
            assert!(interrupted(), "the connect was told to stop first");
            reply.send(ReconnectOutcome::Restarted).unwrap();
        }
        _ => panic!("expected the reset command"),
    }
    assert_eq!(call.join(), Ok(ReconnectOutcome::Restarted));
}

#[test]
fn a_reset_reaches_the_worker_even_when_its_queue_is_full() {
    let (controller, receiver) = refresh_controller();
    for index in 0..4 {
        controller.record_flight_event(format!("fill-{index}"));
    }
    let controller = Arc::new(controller);
    let caller = Arc::clone(&controller);
    let call = FixtureThread::spawn(move || caller.reset_session());
    // The worker leaves the connect and drains: the reset is admitted.
    thread::sleep(Duration::from_millis(100));
    let mut reset = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while reset.is_none() && Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Reconnect { reply, .. }) => reset = Some(reply),
            Ok(_) => {}
            Err(_) => {}
        }
    }
    reset
        .expect("the reset was admitted once the queue drained")
        .send(ReconnectOutcome::Restarted)
        .unwrap();
    assert_eq!(call.join(), Ok(ReconnectOutcome::Restarted));
}

#[test]
fn control_commands_give_up_with_a_reason_instead_of_blocking_for_ever() {
    let (commands, _receiver) = mpsc::sync_channel(1);
    commands
        .try_send(Command::RecordFlight {
            event: "fill".into(),
            incident: false,
        })
        .unwrap();
    let started = Instant::now();
    let (reply, _) = mpsc::channel();
    let error = send_control(
        &commands,
        Command::Disconnect {
            reply,
            _admission: None,
        },
        Duration::from_millis(60),
        None,
    )
    .unwrap_err();
    assert!(error.contains("busy"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn disconnect_admission_survives_the_callers_timeout_until_the_worker_command_is_retired() {
    struct Admission(Arc<AtomicBool>);
    impl Drop for Admission {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let (controller, receiver) = refresh_controller();
    let controller = Arc::new(controller);
    let released = Arc::new(AtomicBool::new(false));
    let admission = Admission(Arc::clone(&released));
    let caller = Arc::clone(&controller);
    let call =
        FixtureThread::spawn(move || caller.disconnect_with_admission(Some(Box::new(admission))));
    let Command::Disconnect {
        reply,
        _admission: held_admission,
    } = receiver.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("expected one disconnect command");
    };
    assert!(!released.load(Ordering::Acquire));
    assert!(call.join().unwrap_err().contains("did not acknowledge"));
    assert!(
        !released.load(Ordering::Acquire),
        "caller timeout must not admit a new holder"
    );
    drop(held_admission);
    assert!(
        released.load(Ordering::Acquire),
        "worker command retirement releases admission"
    );
    assert!(
        reply.send(Ok(())).is_err(),
        "the original timed-out caller is gone"
    );

    let rejected = Arc::new(AtomicBool::new(false));
    for index in 0..4 {
        controller.record_flight_event(format!("full-{index}"));
    }
    assert!(controller
        .disconnect_with_admission(Some(Box::new(Admission(Arc::clone(&rejected)))))
        .is_err());
    assert!(
        rejected.load(Ordering::Acquire),
        "a never-admitted command releases its reservation"
    );
}

#[test]
fn an_unforced_reconnect_during_a_handshake_is_answered_without_queueing() {
    let (controller, receiver) = refresh_controller();
    let controller = Arc::new(controller);
    for phase in ["connecting", "handshaking", "syncing", "ready"] {
        controller.state.lock_recover().phase = phase.into();
        let probe = Arc::clone(&controller);
        let outcome = finishes_within(Duration::from_secs(2), move || probe.reconnect());
        assert_eq!(outcome, Ok(ReconnectOutcome::AlreadyConnected), "{phase}");
    }
    assert!(
        receiver.try_recv().is_err(),
        "nothing was queued behind the connect"
    );
}

#[test]
fn connect_watchdog_expires_without_progress_and_uses_failure_backoff() {
    let start = Instant::now();
    let budget = ConnectAttemptBudget::new(start, STARTUP_PROGRESS_STALL_LIMIT);
    assert!(!budget.expired(start + STARTUP_PROGRESS_STALL_LIMIT - Duration::from_millis(1)));
    let now = start + STARTUP_PROGRESS_STALL_LIMIT;
    assert!(budget.expired(now));
    let result: Result<(), _> = budget.classify(Err(UsbError::Cancelled), now, false);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("automatically reopening"));
    assert!(!matches!(error, UsbError::Cancelled));

    // The failure path schedules the same bounded retry as any failed open.
    let mut owner = SessionRuntime::new(0);
    owner.disconnect(60_000, true);
    owner.transport.connect_failed(60_000);
    assert_eq!(owner.transport.connect_failures(), 1);
    assert!(!owner.transport.reconnect_due(60_000));
    assert!(owner.transport.reconnect_due(120_000));
}

#[test]
fn connect_watchdog_preserves_user_cancellation_success_and_native_errors() {
    let start = Instant::now();
    let budget = ConnectAttemptBudget::new(start, STARTUP_PROGRESS_STALL_LIMIT);
    let now = start + STARTUP_PROGRESS_STALL_LIMIT;
    let cancelled: Result<(), _> = budget.classify(Err(UsbError::Cancelled), now, true);
    assert!(matches!(cancelled, Err(UsbError::Cancelled)));
    assert_eq!(budget.classify(Ok(42), now, false).unwrap(), 42);
    let native: Result<(), _> = budget.classify(Err(UsbError::HandshakeTimeout), now, false);
    assert!(matches!(native, Err(UsbError::HandshakeTimeout)));
    // A subsequent attempt receives its own full budget.
    let next = ConnectAttemptBudget::new(now, STARTUP_PROGRESS_STALL_LIMIT);
    assert!(!next.expired(now));
}

#[test]
fn connect_watchdog_allows_slow_forward_startup_but_not_rebuild_livelock() {
    use quadcortex::device::initialization::DeviceStartupPhase::*;
    let start = Instant::now();
    let budget = ConnectAttemptBudget::new(start, STARTUP_PROGRESS_STALL_LIMIT);
    // Beyond the observed 221 s startup: each forward protocol gate is real
    // progress even when contention makes the complete startup take minutes.
    for (seconds, phase) in [
        (50, VersionValidating),
        (100, Building),
        (150, Initializing),
        (200, Booting),
        (250, Connected),
    ] {
        let now = start + Duration::from_secs(seconds);
        assert!(!budget.expired(now));
        budget.observe_phase(phase, now);
        assert!(!budget.expired(now));
    }
    let stuck = ConnectAttemptBudget::new(start, STARTUP_PROGRESS_STALL_LIMIT);
    stuck.observe_phase(Building, start + Duration::from_secs(30));
    for seconds in [60, 90, 120] {
        stuck.observe_phase(Disconnected, start + Duration::from_secs(seconds));
        stuck.observe_phase(Building, start + Duration::from_secs(seconds));
    }
    assert!(stuck.expired(start + Duration::from_secs(150)));
    // Even fresh forward progress cannot move the absolute ceiling.
    let capped = ConnectAttemptBudget::new(start, STARTUP_ATTEMPT_LIMIT * 2);
    capped.observe_phase(
        Connected,
        start + STARTUP_ATTEMPT_LIMIT - Duration::from_secs(1),
    );
    assert!(!capped.expired(start + STARTUP_ATTEMPT_LIMIT - Duration::from_millis(1)));
    assert!(capped.expired(start + STARTUP_ATTEMPT_LIMIT));
}

#[test]
fn a_session_stuck_outside_connected_is_restarted_but_a_healthy_one_never_is() {
    let limit = Duration::from_secs(60);
    let start = Instant::now();
    let mut watch = StartupWatch::default();
    assert!(!watch.stalled(false, start, limit));
    assert!(!watch.stalled(false, start + Duration::from_secs(59), limit));
    assert!(watch.stalled(false, start + Duration::from_secs(60), limit));
    // It re-arms rather than firing on every later tick.
    assert!(!watch.stalled(false, start + Duration::from_secs(61), limit));
    // Reaching Connected clears the clock.
    let mut healthy = StartupWatch::default();
    assert!(!healthy.stalled(false, start, limit));
    assert!(!healthy.stalled(true, start + Duration::from_secs(50), limit));
    assert!(!healthy.stalled(false, start + Duration::from_secs(100), limit));
}

#[test]
fn a_control_queued_after_a_connect_watch_still_interrupts_that_attempt() {
    let interrupt = ConnectInterrupt::default();
    interrupt.raise();
    // The producer was preempted after its initial raise, before enqueue.
    let watch = interrupt.watch();
    assert!(!watch());
    let (sender, receiver) = mpsc::sync_channel(1);
    send_control(&sender, Command::Stop, Duration::ZERO, Some(&interrupt)).unwrap();
    assert!(
        watch(),
        "queue admission must close the raise-before-enqueue window"
    );
    assert!(matches!(receiver.try_recv(), Ok(Command::Stop)));
}

#[test]
fn a_request_that_arrived_before_a_connect_does_not_cancel_it() {
    let interrupt = ConnectInterrupt::default();
    interrupt.raise();
    let watch = interrupt.watch();
    assert!(!watch(), "only requests after the connect began count");
    interrupt.raise();
    assert!(watch());
}

#[test]
fn status_text_says_how_often_connecting_has_failed_and_what_happens_next() {
    assert_eq!(
        handshake_detail(1, 0),
        "Waiting for Quad Cortex ResetCommsBuffers reply (attempt 1)"
    );
    assert!(handshake_detail(1, 1).contains("1 earlier connect attempt failed"));
    assert!(handshake_detail(2, 3).contains("3 earlier connect attempts failed"));
    let failed = failed_connect_detail("Quad Cortex initialization failed: x", 3);
    assert!(failed.contains("Connect attempt 3 failed"), "{failed}");
    assert!(failed.contains("retrying in 3 s"), "{failed}");
    assert!(failed.contains("Resetting the session retries immediately"));
    let building = startup_progress_detail("building", 2);
    assert!(building.contains("building"), "{building}");
    assert!(building.contains("2 of 3"), "{building}");
    assert_eq!(
        startup_progress_detail("initializing", 0),
        "Starting the Quad Cortex session (initializing)"
    );
}

#[test]
fn native_write_guard_sees_received_state_before_event_publication() {
    let owner = Arc::new(Mutex::new(SessionRuntime::new(0)));
    let (operation, observation, address_revision) = {
        let mut runtime = owner.lock_recover();
        let connection = runtime.opened(0).unwrap();
        runtime.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "session".into(),
            0,
        ));
        let observation = runtime.capture_observation(connection).unwrap();
        runtime
            .observe_state(
                &observation,
                profile::MESSAGE_TYPE_RECALL_PRESET,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(
                        quadcortex::proto::BinaryPreset {
                            name: Some(quadcortex::proto::binary_preset::Name::Name(
                                "Preset".into(),
                            )),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        let snapshot = runtime.state.snapshot_mut();
        snapshot.preset_position = 1;
        snapshot.setlist_key = "/media/p4/Presets/Live".into();
        let context = runtime
            .context_for_write(
                "device.selectScene",
                &serde_json::json!({"scene": 2, "expectedPresetName": "Preset"}),
                &runtime_request::PlannedWrite::HidCommand(
                    quadcortex::commands::DeviceCommand::SelectScene(2),
                ),
            )
            .unwrap();
        let (context, _) = runtime
            .bind_write_preflight(
                &context,
                "device.selectScene",
                &serde_json::json!({"scene": 2, "expectedPresetName": "Preset"}),
                &serde_json::json!({"presetName": "Preset", "activeScene": 0}),
                0,
            )
            .unwrap();
        let operation = runtime
            .plan_operation_in_context(&context, 0, 3000)
            .unwrap();
        runtime.validate_operation(&operation, 0).unwrap();
        (
            operation,
            observation,
            runtime.state.snapshot().address_revision,
        )
    };
    let (sender, queued) = mpsc::sync_channel(2);
    queue_state_observation(
        &sender,
        &owner,
        IncomingMessage {
            observation: Some(observation),
            sequence: 1,
            message_type: profile::MESSAGE_TYPE_SETLIST_POSITION,
            payload: pa::SetlistPositionMessage {
                action: pa::message_action::Enum::Update as i32,
                position: Some(pa::setlist_position_message::Position::Position(4)),
                ..Default::default()
            }
            .encode_to_vec(),
            received_at_unix_ms: 1,
        },
    )
    .unwrap();
    // No event-publication thread has run or consumed this queue.
    let runtime = owner.lock_recover();
    assert_eq!(runtime.state.snapshot().preset_name, "Preset");
    assert_eq!(runtime.state.snapshot().preset_position, 4);
    assert!(runtime.state.snapshot().address_revision > address_revision);
    assert_eq!(
        runtime.validate_operation(&operation, 1).unwrap_err(),
        "QC write target changed after planning; refresh before starting a new operation"
    );
    assert!(matches!(
        queued.try_recv(),
        Ok(StateDecoderCommand::StateWake)
    ));
}

#[test]
fn publication_reset_clears_only_a_revoked_sessions_log_and_caches() {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        7,
        "first".into(),
        0,
    ));
    let mut published = Some(owner.capture_observation(connection).unwrap());
    let frame = DecodedStateFrame {
        stream_id: "test-stream".into(),
        sequence: 1,
        observed_at: 1,
        states: vec![StateUpdate::empty("position")],
        tempo_clock: None,
    };
    let shared = StateDecoderShared {
        stream_id: "test-stream".into(),
        session_owner: Arc::new(Mutex::new(SessionRuntime::new(0))),
        event_log: Arc::new(Mutex::new({
            let mut journal = StateEventJournal::default();
            journal.push(frame.clone());
            journal
        })),
        latest_tempo_clock: Arc::new(Mutex::new(Some(frame.clone()))),
        latest_master_volume: Arc::new(Mutex::new(Some(frame))),
        latest_sequence: Arc::new(AtomicU64::new(1)),
        subscribers: Arc::new(Mutex::new(Vec::new())),
    };
    clear_revoked_state_publications(&shared, &owner, &mut published);
    assert_eq!(shared.event_log.lock_recover().len(), 1);
    assert!(published.is_some());
    owner.disconnect(1, false);
    clear_revoked_state_publications(&shared, &owner, &mut published);
    assert!(shared.event_log.lock_recover().is_empty());
    assert!(shared.latest_master_volume.lock_recover().is_none());
    assert!(shared.latest_tempo_clock.lock_recover().is_none());
    assert!(published.is_none());
    // A cleared session must not recycle the producer's event cursor.
    assert_eq!(shared.latest_sequence.load(Ordering::Acquire), 1);
}

#[test]
fn metadata_wakeup_cannot_publish_ahead_of_an_earlier_wire_reduction() {
    use quadcortex::device::session::SessionDriver;
    let owner = Arc::new(Mutex::new(SessionRuntime::new(0)));
    let metadata = {
        let mut runtime = owner.lock_recover();
        let connection = runtime.opened(0).unwrap();
        runtime.lifecycle = Some(SessionDriver::start(7, "publication-order".into(), 0));
        let metadata = runtime.capture_observation(connection).unwrap();
        assert!(runtime.queue_model_repo(metadata.clone(), metadata_payload("Installed")));
        assert!(runtime.take_model_repo().is_some());
        let wire = runtime.capture_observation(connection).unwrap();
        runtime
            .observe_state(
                &wire,
                15,
                &pa::RecallPresetMessage {
                    action: pa::message_action::Enum::Update as i32,
                    preset: Some(pa::recall_preset_message::Preset::Preset(
                        quadcortex::proto::BinaryPreset::default(),
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
            )
            .unwrap();
        metadata
    };
    let event_log = Arc::new(Mutex::new(StateEventJournal::default()));
    let shared = StateDecoderShared {
        stream_id: "test-stream".into(),
        session_owner: Arc::clone(&owner),
        event_log: Arc::clone(&event_log),
        latest_tempo_clock: Arc::new(Mutex::new(None)),
        latest_master_volume: Arc::new(Mutex::new(None)),
        latest_sequence: Arc::new(AtomicU64::new(0)),
        subscribers: Arc::new(Mutex::new(Vec::new())),
    };
    let (sender, receiver) = mpsc::sync_channel(16);
    // Force metadata installation before the reader can send its wakeup.
    sender
        .send(StateDecoderCommand::Catalog(
            metadata,
            Ok(parse_model_repo(&metadata_payload("Installed")).unwrap()),
        ))
        .unwrap();
    sender.send(StateDecoderCommand::StateWake).unwrap();
    // An earlier USB reset command may reach the decoder after a catalog
    // wakeup has already drained state from the replacement session.
    sender.send(StateDecoderCommand::Reset).unwrap();
    sender.send(StateDecoderCommand::Stop).unwrap();
    run_state_decoder(shared, sender, receiver, Instant::now());
    let frames = event_log.lock_recover();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].states[0].kind, "preset");
    assert_eq!(frames[0].sequence, 1);
    assert_eq!(frames[1].sequence, 2);
    let wire_version = frames[0].states[0].state_version.as_ref().unwrap();
    let metadata_version = frames[1].states[0].state_version.as_ref().unwrap();
    assert_eq!(wire_version.session, metadata_version.session);
    assert!(wire_version.revision < metadata_version.revision);
}

#[test]
fn windows_decoder_drops_queued_old_frames_and_parser_results_after_owner_replacement() {
    use quadcortex::device::session::SessionDriver;
    let owner = Arc::new(Mutex::new(SessionRuntime::new(0)));
    let (old, current) = {
        let mut runtime = owner.lock_recover();
        let first = runtime.opened(0).unwrap();
        runtime.lifecycle = Some(SessionDriver::start(7, "same-wire-id".into(), 0));
        let old = runtime.capture_observation(first).unwrap();
        assert!(runtime.queue_model_repo(old.clone(), metadata_payload("Old parse")));
        assert!(runtime.take_model_repo().is_some());
        runtime.disconnect(0, false);
        let second = runtime.opened(0).unwrap();
        runtime.lifecycle = Some(SessionDriver::start(7, "same-wire-id".into(), 0));
        let current = runtime.capture_observation(second).unwrap();
        assert!(runtime.queue_model_repo(current.clone(), metadata_payload("Current")));
        assert!(runtime.take_model_repo().is_some());
        (old, current)
    };
    let (sender, worker) = decoder_lane(&owner);
    sender
        .send(StateDecoderCommand::Catalog(
            current,
            Ok(parse_model_repo(&metadata_payload("Current")).unwrap()),
        ))
        .unwrap();
    queue_state_observation(
        &sender,
        &owner,
        IncomingMessage {
            observation: Some(old.clone()),
            sequence: 1,
            message_type: profile::MESSAGE_TYPE_MODEL_REPO,
            payload: metadata_payload("Old frame"),
            received_at_unix_ms: 0,
        },
    )
    .unwrap();
    sender
        .send(StateDecoderCommand::Catalog(
            old,
            Ok(parse_model_repo(&metadata_payload("Old parser")).unwrap()),
        ))
        .unwrap();
    sender.send(StateDecoderCommand::Stop).unwrap();
    worker.join().unwrap();
    let runtime = owner.lock_recover();
    let models = runtime.state.model_list().unwrap();
    assert_eq!(models.models.len(), 1);
    assert_eq!(models.models[0].name, "Current");
    assert!(runtime.lifecycle.as_ref().unwrap().failure().is_none());
}

#[test]
fn windows_metadata_lane_drives_the_shared_building_success_and_failure_gates() {
    use quadcortex::device::initialization::{DeviceStartupError, DeviceStartupPhase};
    use quadcortex::device::session::{DispatchOutcome, SessionDriver, SessionFailure};
    for valid in [true, false] {
        let owner = Arc::new(Mutex::new(SessionRuntime::new(0)));
        let ticket = {
            let mut runtime = owner.lock_recover();
            let connection = runtime.opened(0).unwrap();
            let mut driver = SessionDriver::start(7, "session".into(), 0);
            let reset = driver.next_batch(0, 0).unwrap();
            driver.dispatched(&reset.ticket, DispatchOutcome::Accepted);
            driver.observe(
                52,
                &pa::ResetCommsBuffersMessage {
                    session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
                        "session".into(),
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
                0,
            );
            let version = driver.next_batch(0, 0).unwrap();
            driver.dispatched(&version.ticket, DispatchOutcome::Accepted);
            driver.observe(
                10,
                &pa::VersionMessage {
                    action: pa::message_action::Enum::Update as i32,
                    device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
                        pa::version_message::DeviceType::Qc as i32,
                    )),
                    ..Default::default()
                }
                .encode_to_vec(),
                0,
            );
            let disconnect = driver.next_batch(0, 0).unwrap();
            driver.dispatched(&disconnect.ticket, DispatchOutcome::Accepted);
            let building = driver.next_batch(0, 0).unwrap();
            driver.dispatched(&building.ticket, DispatchOutcome::Accepted);
            assert_eq!(driver.phase(), DeviceStartupPhase::Building);
            runtime.lifecycle = Some(driver);
            runtime.capture_observation(connection).unwrap()
        };
        let (sender, worker) = decoder_lane(&owner);
        queue_state_observation(
            &sender,
            &owner,
            IncomingMessage {
                observation: Some(ticket),
                sequence: 1,
                message_type: profile::MESSAGE_TYPE_MODEL_REPO,
                payload: if valid {
                    metadata_payload("Installed")
                } else {
                    vec![0xff]
                },
                received_at_unix_ms: 0,
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if owner.lock_recover().lifecycle.as_ref().unwrap().phase()
                != DeviceStartupPhase::Building
            {
                break;
            }
            assert!(Instant::now() < deadline, "metadata lane did not complete");
            thread::sleep(Duration::from_millis(1));
        }
        sender.send(StateDecoderCommand::Stop).unwrap();
        worker.join().unwrap();
        let mut runtime = owner.lock_recover();
        let driver = runtime.lifecycle.as_mut().unwrap();
        if valid {
            assert_eq!(driver.phase(), DeviceStartupPhase::Initializing);
            let batch = driver.next_batch(1, 0).unwrap();
            assert_eq!(
                batch
                    .messages
                    .iter()
                    .map(|m| m.message_type)
                    .collect::<Vec<_>>(),
                vec![35]
            );
            assert_eq!(
                runtime.state.model_list().unwrap().models[0].name,
                "Installed"
            );
        } else {
            assert_eq!(
                driver.failure(),
                Some(&SessionFailure::Protocol(
                    DeviceStartupError::ModelRepositoryDataError
                ))
            );
            assert!(driver.next_batch(1, 0).is_none());
            assert_eq!(runtime.state.model_count(), 0);
        }
    }
}

#[test]
fn a_cancelled_read_unsubscribes_even_when_the_device_is_silent() {
    let bus = Arc::new(RawEventBus::default());
    let first = subscribe_raw(&bus);
    let second = subscribe_raw(&bus);
    assert_eq!(bus.subscriber_count(), 2);
    drop(first);
    assert_eq!(bus.subscriber_count(), 1);
    drop(second);
    assert!(bus.subscriber_count() == 0);
}

#[test]
fn an_old_epoch_waiter_cannot_accept_a_timely_matching_reply() {
    let (reply, result) = mpsc::channel();
    let mut pending = vec![PendingRequest {
        epoch: 7,
        expectation: ResponseExpectation::new(13, Some(42), 0, 100),
        reply,
    }];
    let message = IncomingMessage {
        observation: None,
        sequence: 1,
        message_type: 13,
        payload: vec![8, 1, 16, 42],
        received_at_unix_ms: 0,
    };
    deliver_pending(&mut pending, &message, 1, 8);
    assert_eq!(
        result.try_recv().unwrap().unwrap_err(),
        "Device session changed while awaiting a QC read"
    );
    assert!(pending.is_empty());
}

fn raw_frame(sequence: u64, bytes: usize) -> IncomingMessage {
    IncomingMessage {
        observation: None,
        sequence,
        message_type: profile::MESSAGE_TYPE_FILE,
        payload: vec![0; bytes],
        received_at_unix_ms: 0,
    }
}

#[test]
fn a_stalled_raw_subscriber_is_held_to_its_byte_budget_without_blocking_others() {
    let bus = Arc::new(RawEventBus::default());
    let stalled = subscribe_raw(&bus);
    let live = subscribe_raw(&bus);
    let half = RAW_SUBSCRIBER_QUEUE_MAX_BYTES / 2;
    // Just below, then exactly at, the stalled subscriber's budget.
    fan_out_raw_event(&bus, &raw_frame(1, half));
    assert_eq!(live.try_recv().unwrap().sequence, 1);
    fan_out_raw_event(&bus, &raw_frame(2, half));
    assert_eq!(live.try_recv().unwrap().sequence, 2);
    assert_eq!(stalled.queued_bytes(), RAW_SUBSCRIBER_QUEUE_MAX_BYTES);
    // One byte past it: dropped for the stalled subscriber only, and the
    // fan-out returned (it never waited for the stalled reader).
    fan_out_raw_event(&bus, &raw_frame(3, 1));
    assert_eq!(live.try_recv().unwrap().sequence, 3);
    assert_eq!(stalled.dropped_frames(), 1);
    assert_eq!(live.dropped_frames(), 0);
    let counters = bus.retention.snapshot();
    assert_eq!(counters.raw_subscriber_dropped_frames, 1);
    assert_eq!(counters.raw_subscriber_dropped_bytes, 1);
    // Both subscriptions remain; draining restores the budget.
    assert_eq!(bus.subscriber_count(), 2);
    assert_eq!(stalled.try_recv().unwrap().sequence, 1);
    fan_out_raw_event(&bus, &raw_frame(4, 1));
    assert_eq!(stalled.try_recv().unwrap().sequence, 2);
    assert_eq!(stalled.try_recv().unwrap().sequence, 4);
}

#[test]
fn a_stalled_state_subscriber_is_held_to_the_journal_budget_and_counted() {
    let controller = DeviceController::start_disconnected().unwrap();
    let stalled = controller.subscribe_state_events();
    // Frames are counted in their encoded bytes, so fill the queue with
    // frames until the next one would pass the 8 MiB budget.
    let frame = |sequence: u64| DecodedStateFrame {
        stream_id: controller.state_stream_id.clone(),
        sequence,
        observed_at: 0,
        states: vec![serde_json::from_value(serde_json::json!({
            "kind": "preset",
            "presetName": "x".repeat(1_000_000)
        }))
        .unwrap()],
        tempo_clock: None,
    };
    let mut sequence = 0;
    while stalled.dropped_frames() == 0 {
        sequence += 1;
        assert!(sequence < 64, "the byte budget never applied");
        controller.inject_state_frame_for_test(frame(sequence));
    }
    assert!(stalled.queued_bytes() <= STATE_SUBSCRIBER_QUEUE_MAX_BYTES);
    assert!(stalled.queued_bytes() > STATE_SUBSCRIBER_QUEUE_MAX_BYTES - 1_100_000);
    assert!(sequence < SUBSCRIBER_QUEUE_CAPACITY as u64);
    let retention = controller.status().retention;
    assert_eq!(retention.state_subscriber_dropped_frames, 1);
    assert!(retention.state_subscriber_dropped_bytes > 1_000_000);
    // The journal still holds the dropped frame for catch-up.
    assert_eq!(controller.latest_state_sequence(), sequence);
}

#[test]
fn latest_cache_refusals_are_reported_by_size() {
    let mut latest = HashMap::new();
    let capacity = quadcortex_hid::LATEST_MESSAGE_CACHE_MAX_ENTRIES as u16;
    assert!(capacity > 0, "the latest-message cache must accept entries");
    for message_type in 0..capacity {
        let mut message = raw_frame(1, 1);
        message.message_type = 1000 + message_type;
        assert_eq!(cache_latest_message(&mut latest, message), None);
    }
    assert_eq!(latest.len(), usize::from(capacity));
    let mut one_more = raw_frame(2, 7);
    one_more.message_type = 999;
    assert_eq!(cache_latest_message(&mut latest, one_more), Some(7));
    assert!(!latest.contains_key(&999));
}

#[test]
fn a_waiter_does_not_keep_the_event_bus_alive_after_owner_shutdown() {
    let bus = Arc::new(RawEventBus::default());
    let subscription = subscribe_raw(&bus);
    let weak = Arc::downgrade(&bus);
    drop(bus);
    assert!(weak.upgrade().is_none());
    assert!(matches!(
        subscription.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    drop(subscription);
}

#[test]
fn a_stalled_read_batch_does_not_submit_its_expired_remainder() {
    let started = Instant::now();
    let admission = test_read_admission(7, started, Duration::from_millis(50));
    let mut times = [started, started + Duration::from_millis(50)].into_iter();
    let messages = vec![commands::read_version(), commands::read_version()];
    let mut writes = 0;
    let result = submit_read_batch(
        messages,
        &admission,
        7,
        || times.next().unwrap(),
        |_| {
            writes += 1;
            Ok(())
        },
    );
    assert_eq!(writes, 1);
    assert_eq!(result.unwrap_err(), "QC read expired before USB submission");
}

#[test]
fn failed_read_submission_is_not_replayed_or_followed_by_more_messages() {
    let admission = test_read_admission(7, Instant::now(), Duration::from_secs(1));
    let mut writes = 0;
    let result = submit_read_batch(
        vec![commands::read_version(), commands::read_version()],
        &admission,
        7,
        Instant::now,
        |_| {
            writes += 1;
            Err("native write failed".into())
        },
    );
    assert_eq!(writes, 1);
    assert_eq!(result.unwrap_err(), "native write failed");
}

#[test]
fn delayed_timeout_poll_cannot_turn_a_late_reply_into_success() {
    let (expired_reply, expired_result) = mpsc::channel();
    let (live_reply, live_result) = mpsc::channel();
    let mut pending = vec![
        PendingRequest {
            epoch: 7,
            expectation: ResponseExpectation::new(13, Some(42), 100, 50),
            reply: expired_reply,
        },
        PendingRequest {
            epoch: 7,
            expectation: ResponseExpectation::new(13, Some(42), 120, 50),
            reply: live_reply,
        },
    ];
    let message = IncomingMessage {
        observation: Some(timed_observation(149, 149)),
        sequence: 1,
        message_type: 13,
        payload: vec![0x08, 0x01, 0x10, 0x2a],
        received_at_unix_ms: 0,
    };
    // No maintenance expiry was run before this receive, just as happens
    // when the worker is occupied with native output or decoder backpressure.
    deliver_pending(&mut pending, &message, 150, 7);
    assert!(expired_result
        .try_recv()
        .unwrap()
        .unwrap_err()
        .ends_with("before timeout"));
    assert_eq!(live_result.try_recv().unwrap().unwrap().sequence, 1);
    assert!(pending.is_empty());
}

#[test]
fn queued_read_cannot_gain_a_new_budget_or_cross_a_session_change() {
    let start = Instant::now();
    let admission = test_read_admission(7, start, Duration::from_millis(50));
    assert_eq!(
        admission
            .remaining(7, start + Duration::from_millis(40))
            .unwrap(),
        Duration::from_millis(10)
    );
    assert_eq!(
        admission
            .remaining(7, start + Duration::from_millis(50))
            .unwrap_err(),
        "QC read expired before USB submission"
    );
    assert!(admission
        .remaining(8, start)
        .unwrap_err()
        .contains("session changed"));
}

#[test]
fn a_buffered_matching_frame_does_not_complete_a_new_raw_request() {
    let (reply, receiver) = mpsc::channel();
    let mut pending = vec![PendingRequest {
        epoch: 7,
        expectation: ResponseExpectation::new(13, Some(42), 100, 100),
        reply,
    }];
    let mut message = IncomingMessage {
        observation: Some(timed_observation(90, 110)),
        sequence: 1,
        message_type: 13,
        payload: vec![8, 1, 16, 42],
        received_at_unix_ms: 0,
    };
    deliver_pending(&mut pending, &message, 120, 7);
    assert_eq!(pending.len(), 1);
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    message.observation = Some(timed_observation(121, 125));
    deliver_pending(&mut pending, &message, 130, 7);
    assert!(pending.is_empty());
    assert_eq!(receiver.try_recv().unwrap().unwrap().sequence, 1);
}

#[test]
fn read_planning_retains_its_original_clock_and_rejects_a_replacement_session() {
    let controller = DeviceController::start_disconnected().unwrap();
    let fence = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "read-session".into(),
            0,
        ));
        owner.session_fence().unwrap()
    };
    let started = controller.session_clock;
    let admission = controller
        .read_admission(7, &fence, started, Duration::from_millis(50))
        .unwrap();
    assert_eq!(admission.operation.remaining_ms(49), 1);
    assert!(admission
        .remaining(7, started + Duration::from_millis(50))
        .is_err());
    {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "read-session".into(),
            0,
        ));
    }
    assert!(controller
        .read_admission(7, &fence, started, Duration::from_millis(50))
        .is_err());
    assert!(controller
        .session_owner
        .lock_recover()
        .validate_operation(&admission.operation, 1)
        .is_err());
}

#[test]
fn realtime_plan_queues_one_paced_batch_and_cannot_cross_physical_replacement() {
    let mut controller = DeviceController::start_disconnected().unwrap();
    let (sender, receiver) = mpsc::sync_channel(4);
    let original_commands = std::mem::replace(&mut controller.commands, sender);
    let fence = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "same-wire-id".into(),
            0,
        ));
        owner.session_context().unwrap()
    };
    // A hold is the one two-frame screen gesture left (touch-down, wait,
    // touch-up); a tap is a single TAP frame and no longer goes through the
    // sequence lane.
    let operation = DeviceOperation::ScreenHold { x: 120.0, y: 80.0 };
    let expected = operation.clone().try_encode().unwrap();
    assert!(expected.len() > 1);
    controller
        .queue_operation_for_plan(operation.clone(), &fence)
        .unwrap();
    let Command::SendSequence {
        messages,
        interval,
        admission,
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("realtime gesture was not admitted as one batch");
    };
    assert_eq!(messages, expected);
    assert_eq!(
        interval,
        Duration::from_millis(profile::REMOTE_HOLD_INTERVAL_MS)
    );
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(1).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "same-wire-id".into(),
            1,
        ));
        assert!(owner.validate_operation(&admission, 1).is_err());
    }
    assert!(controller
        .queue_operation_for_plan(operation, &fence)
        .is_err());
    assert!(controller
        .send_command_for_plan(commands::read_version(), &fence)
        .is_err());
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    controller.commands = original_commands;
}

#[test]
fn swipe_queues_exactly_one_drag_frame_as_a_single_write_with_no_release_frame() {
    let mut controller = DeviceController::start_disconnected().unwrap();
    let (sender, receiver) = mpsc::sync_channel(4);
    let original_commands = std::mem::replace(&mut controller.commands, sender);
    let fence = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "swipe-session".into(),
            0,
        ));
        owner.session_context().unwrap()
    };
    let operation = DeviceOperation::ScreenDrag {
        x: 400.0,
        y: 300.0,
        to_x: 400.0,
        to_y: 250.0,
    };
    let expected = operation.clone().try_encode().unwrap();
    // One DRAG frame: no begin frame, no end frame to pace or to abort.
    assert_eq!(expected.len(), 1);
    assert_eq!(operation.abort_release_frame(), None);
    controller
        .queue_operation_for_plan(operation, &fence)
        .unwrap();
    let Command::Send(message_type, payload, _, _) = receiver.try_recv().unwrap() else {
        panic!("a swipe is one frame and must not be admitted as a paced sequence");
    };
    assert_eq!(message_type, expected[0].message_type);
    assert_eq!(payload, expected[0].payload);
    // Nothing else is queued: no second frame follows the DRAG frame.
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    controller.commands = original_commands;
}

#[test]
fn a_swipe_is_never_confused_with_the_stepped_drag_lane() {
    // On CorOS 4.1.0 (2026-10-06) the stepped MOVE stream did not scroll and
    // the single DRAG frame did.
    let swipe = DeviceOperation::ScreenDrag {
        x: 10.0,
        y: 20.0,
        to_x: 30.0,
        to_y: 40.0,
    };
    let stepped = DeviceOperation::ScreenDragSteps {
        x: 10.0,
        y: 20.0,
        to_x: 30.0,
        to_y: 40.0,
        steps: 18,
        hold_ms: 200,
    };
    assert_eq!(swipe.clone().try_encode().unwrap().len(), 1);
    assert!(stepped.clone().try_encode().unwrap().len() > 1);
    assert_ne!(
        swipe.try_encode().unwrap(),
        stepped.try_encode().unwrap()[..1].to_vec()
    );
}

fn raw_mouse_frame(kind: commands::ScreenMouseType, x: f32, y: f32) -> DeviceOperation {
    DeviceOperation::ScreenMouseFrame {
        kind,
        x,
        y,
        to: None,
    }
}

fn queued_frame(
    receiver: &mpsc::Receiver<Command>,
) -> (u16, Vec<u8>, mpsc::Sender<Result<(), String>>) {
    match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
        Command::Send(message_type, payload, _, reply) => (message_type, payload, reply),
        _ => panic!("a single raw frame must be queued as one plain write"),
    }
}

#[test]
fn a_definitely_unqueued_raw_press_preserves_the_previous_touch_guard() {
    use commands::ScreenMouseType::Press;
    for previously_held in [false, true] {
        for refusal in ["full", "disconnected", "stale"] {
            let (controller, receiver) = refresh_controller();
            let fence = controller
                .session_owner
                .lock_recover()
                .session_context()
                .unwrap();
            if previously_held {
                controller.touch_guard.lock_recover().note_unreleased_hold(
                    9.0,
                    11.0,
                    0,
                    "existing uncertain press".into(),
                );
            }
            let previous = controller.touch_guard.lock_recover().held();
            match refusal {
                "full" => {
                    for index in 0..4 {
                        controller.record_flight_event(format!("full-{index}"));
                    }
                }
                "disconnected" => {
                    controller.session_owner.lock_recover().disconnect(1, false);
                }
                "stale" => {
                    let mut owner = controller.session_owner.lock_recover();
                    owner.disconnect(1, false);
                    owner.opened(2).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(controller
                .queue_operation_for_plan(raw_mouse_frame(Press, 40.0, 50.0), &fence)
                .is_err());
            let guard = controller.touch_guard.lock_recover();
            assert_eq!(
                guard.held(),
                previous,
                "{refusal}, previously held={previously_held}"
            );
            assert_eq!(
                guard
                    .admit(&DeviceOperation::ScreenTap { x: 1.0, y: 2.0 }, 3)
                    .is_err(),
                previously_held
            );
            drop(guard);
            assert!(receiver
                .try_iter()
                .all(|command| matches!(command, Command::RecordFlight { .. })));
        }
    }
}

#[test]
fn a_raw_press_holds_the_pointer_and_every_other_gesture_is_refused_until_a_release() {
    use commands::ScreenMouseType::{Move, Press, Release};
    let (controller, receiver) = tests::refresh_controller();
    let fence = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    // The press is queued as exactly one frame and marks the pointer held.
    controller
        .queue_operation_for_plan(raw_mouse_frame(Press, 40.0, 50.0), &fence)
        .unwrap();
    let (message_type, payload, _) = queued_frame(&receiver);
    assert_eq!(message_type, 72);
    assert_eq!(
        payload,
        commands::screen_mouse_frame(Press, 40.0, 50.0, None).payload
    );
    assert!(controller.touch_guard.lock_recover().held().is_some());
    // Every other gesture gets a typed refusal and puts nothing on the queue.
    for refused in [
        DeviceOperation::ScreenTap { x: 1.0, y: 2.0 },
        DeviceOperation::ScreenHold { x: 1.0, y: 2.0 },
        DeviceOperation::ScreenDrag {
            x: 1.0,
            y: 2.0,
            to_x: 3.0,
            to_y: 4.0,
        },
        DeviceOperation::ScreenDragSteps {
            x: 1.0,
            y: 2.0,
            to_x: 3.0,
            to_y: 4.0,
            steps: 4,
            hold_ms: 0,
        },
        raw_mouse_frame(commands::ScreenMouseType::Drag, 1.0, 2.0),
    ] {
        let error = controller
            .queue_operation_for_plan(refused.clone(), &fence)
            .unwrap_err();
        assert!(error.starts_with("TOUCH_HELD"), "{refused:?}: {error}");
        // The synchronous path is refused identically.
        let error = controller
            .send_operation_for_plan(refused.clone(), &fence)
            .unwrap_err();
        assert!(error.starts_with("TOUCH_HELD"), "{refused:?}: {error}");
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    // A raw move is allowed, a further press is a pointer-position update
    // (item drag) and is allowed too, and the release lifts the pointer.
    controller
        .queue_operation_for_plan(raw_mouse_frame(Move, 41.0, 51.0), &fence)
        .unwrap();
    let _ = queued_frame(&receiver);
    controller
        .queue_operation_for_plan(raw_mouse_frame(Press, 41.0, 51.0), &fence)
        .unwrap();
    let _ = queued_frame(&receiver);
    assert!(controller.touch_guard.lock_recover().held().is_some());
    controller
        .queue_operation_for_plan(raw_mouse_frame(Release, 41.0, 51.0), &fence)
        .unwrap();
    let _ = queued_frame(&receiver);
    assert_eq!(controller.touch_guard.lock_recover().held(), None);
    // After it, an ordinary tap is one TAP frame again.
    controller
        .queue_operation_for_plan(DeviceOperation::ScreenTap { x: 1.0, y: 2.0 }, &fence)
        .unwrap();
    let (_, payload, _) = queued_frame(&receiver);
    assert_eq!(payload, commands::screen_tap(1.0, 2.0)[0].payload);
}

#[test]
fn a_raw_tap_ends_a_held_press_like_a_release_and_a_failed_one_does_not() {
    use commands::ScreenMouseType::{Press, Tap};
    let (controller, receiver) = tests::refresh_controller();
    let fence = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    // Long press on a Grid block, then one TAP on the target cell drops it.
    controller
        .queue_operation_for_plan(raw_mouse_frame(Press, 98.0, 147.0), &fence)
        .unwrap();
    let _ = queued_frame(&receiver);
    assert!(controller.touch_guard.lock_recover().held().is_some());
    controller
        .queue_operation_for_plan(raw_mouse_frame(Tap, 448.0, 147.0), &fence)
        .unwrap();
    let (_, payload, _) = queued_frame(&receiver);
    assert_eq!(
        payload,
        commands::screen_mouse_frame(Tap, 448.0, 147.0, None).payload
    );
    assert_eq!(controller.touch_guard.lock_recover().held(), None);
    // The ordinary tap command is not that: refused while held, and the hold stays.
    controller
        .queue_operation_for_plan(raw_mouse_frame(Press, 98.0, 147.0), &fence)
        .unwrap();
    let _ = queued_frame(&receiver);
    let error = controller
        .queue_operation_for_plan(DeviceOperation::ScreenTap { x: 1.0, y: 2.0 }, &fence)
        .unwrap_err();
    assert!(error.starts_with("TOUCH_HELD"), "{error}");
}

#[test]
fn a_press_that_cannot_be_encoded_never_marks_the_pointer_held() {
    let (controller, receiver) = tests::refresh_controller();
    let fence = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    let off_screen = raw_mouse_frame(commands::ScreenMouseType::Press, 800.0, 10.0);
    assert!(controller
        .queue_operation_for_plan(off_screen, &fence)
        .is_err());
    assert_eq!(controller.touch_guard.lock_recover().held(), None);
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn an_abandoned_press_is_never_released_by_the_host() {
    use commands::ScreenMouseType::{Press, Release};
    let (controller, receiver) = tests::refresh_controller();
    let fence = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    controller
        .queue_operation_for_plan(raw_mouse_frame(Press, 123.0, 45.0), &fence)
        .unwrap();
    let (_, payload, _) = queued_frame(&receiver);
    assert_eq!(
        payload,
        commands::screen_mouse_frame(Press, 123.0, 45.0, None).payload
    );
    // An earlier version sent a RELEASE of its own five seconds later. It was
    // unrequested and the delay was never measured, and a release is itself a
    // click (or a scene assignment, or a drop onto the delete bar), so the
    // host now sends nothing: no timer thread exists, however long we wait.
    assert!(receiver.recv_timeout(Duration::from_millis(400)).is_err());
    assert!(controller.touch_guard.lock_recover().held().is_some());
    // The refusal names the way out and says nobody lifts it for the caller.
    let refusal = controller
        .queue_operation_for_plan(DeviceOperation::ScreenTap { x: 1.0, y: 2.0 }, &fence)
        .unwrap_err();
    assert!(refusal.starts_with("TOUCH_HELD"), "{refusal}");
    assert!(refusal.contains("never releases it by itself"), "{refusal}");
    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    // Only the caller's own release lifts it, and it goes out as exactly the
    // frame the caller asked for.
    controller
        .queue_operation_for_plan(raw_mouse_frame(Release, 123.0, 45.0), &fence)
        .unwrap();
    let (_, payload, _) = queued_frame(&receiver);
    assert_eq!(
        payload,
        commands::screen_mouse_frame(Release, 123.0, 45.0, None).payload
    );
    assert_eq!(controller.touch_guard.lock_recover().held(), None);
}

#[test]
fn a_hold_is_queued_with_its_own_touch_up_and_its_held_point_for_the_worker() {
    let (controller, receiver) = tests::refresh_controller();
    let fence = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    let hold = DeviceOperation::ScreenHold { x: 120.0, y: 80.0 };
    let frames = hold.clone().try_encode().unwrap();
    controller.queue_operation_for_plan(hold, &fence).unwrap();
    let Command::SendSequence {
        messages,
        release_on_failure,
        hold: record,
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("a hold is one paced batch");
    };
    assert_eq!(messages, frames);
    // The cleanup frame is the hold's own touch-up (a RELEASE), never a PRESS.
    assert_eq!(release_on_failure.as_ref(), frames.last());
    let record = record.expect("a hold carries its guard record");
    assert_eq!((record.x, record.y), (120.0, 80.0));
    // Queueing a hold records nothing as held: it balances its own press.
    assert_eq!(controller.touch_guard.lock_recover().held(), None);
    // A tap and a swipe are single plain writes and carry no hold record.
    for operation in [
        DeviceOperation::ScreenTap { x: 1.0, y: 2.0 },
        DeviceOperation::ScreenDrag {
            x: 1.0,
            y: 2.0,
            to_x: 30.0,
            to_y: 40.0,
        },
    ] {
        controller
            .queue_operation_for_plan(operation, &fence)
            .unwrap();
        assert!(matches!(receiver.try_recv().unwrap(), Command::Send(..)));
    }
}

#[test]
fn a_hold_whose_touch_up_could_not_be_sent_is_recorded_as_held_and_nothing_else_is() {
    let guard = Arc::new(Mutex::new(TouchGuard::default()));
    let record = HoldRecord {
        guard: Arc::clone(&guard),
        x: 120.0,
        y: 80.0,
    };
    let tap = DeviceOperation::ScreenTap { x: 1.0, y: 2.0 };
    // Delivered: nothing is held.
    note_hold_outcome(Some(&record), &Ok(()), 10);
    assert_eq!(guard.lock_recover().held(), None);
    // The touch-down itself failed: nothing is known to be pressed.
    note_hold_outcome(Some(&record), &Err("write failed".into()), 10);
    assert_eq!(guard.lock_recover().held(), None);
    // The touch-up failed but the one second attempt went out: released.
    note_hold_outcome(
        Some(&record),
        &Err("write failed; the touch was released".into()),
        10,
    );
    assert_eq!(guard.lock_recover().held(), None);
    // No record (every other sequence): never recorded.
    note_hold_outcome(None, &Err(format!("x; {TOUCH_UP_UNSENT}: y")), 10);
    assert_eq!(guard.lock_recover().held(), None);
    // The touch-up could not be sent even on the second attempt: held, with
    // the reason, until the caller sends a release.
    note_hold_outcome(
        Some(&record),
        &Err(format!("write failed; {TOUCH_UP_UNSENT}: gone")),
        500,
    );
    let held = guard.lock_recover().held().expect("recorded as held");
    assert_eq!((held.x, held.y), (120.0, 80.0));
    let refusal = guard.lock_recover().admit(&tap, 900).unwrap_err();
    assert!(refusal.starts_with("TOUCH_HELD"), "{refusal}");
    assert!(refusal.contains("(120, 80)"), "{refusal}");
    assert!(refusal.contains("gone"), "{refusal}");
    guard.lock_recover().note_release_sent();
    assert_eq!(guard.lock_recover().admit(&tap, 1_000), Ok(()));
}

#[test]
fn stepped_drag_queues_one_batch_paced_at_the_drag_interval() {
    let mut controller = DeviceController::start_disconnected().unwrap();
    let (sender, receiver) = mpsc::sync_channel(4);
    let original_commands = std::mem::replace(&mut controller.commands, sender);
    let fence = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "drag-session".into(),
            0,
        ));
        owner.session_context().unwrap()
    };
    let operation = DeviceOperation::ScreenDragSteps {
        x: 400.0,
        y: 100.0,
        to_x: 400.0,
        to_y: 300.0,
        steps: 18,
        hold_ms: 200,
    };
    let expected = operation.clone().try_encode().unwrap();
    assert_eq!(expected.len(), 30);
    controller
        .queue_operation_for_plan(operation, &fence)
        .unwrap();
    let Command::SendSequence {
        messages,
        interval,
        release_on_failure,
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("a stepped drag was not admitted as one ordered batch");
    };
    // The batch carries the touch-up a host sends if it stops early, and
    // that frame is the batch's own last frame.
    assert_eq!(release_on_failure.as_ref(), expected.last());
    // One batch of every frame, in order, with one uniform pace: not the
    // 20 ms swipe interval.
    assert_eq!(messages, expected);
    assert_eq!(
        interval,
        Duration::from_millis(profile::REMOTE_DRAG_INTERVAL_MS)
    );
    assert_ne!(
        interval,
        Duration::from_millis(profile::REMOTE_GESTURE_INTERVAL_MS)
    );
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    controller.commands = original_commands;
}

#[test]
fn a_failed_press_release_gesture_sends_one_touch_up_and_replays_nothing_else() {
    let frames = |count: u8| -> Vec<OutboundMessage> {
        (0..count)
            .map(|index| OutboundMessage {
                message_type: 72,
                payload: vec![index],
            })
            .collect()
    };
    let messages = frames(6);
    let release = OutboundMessage {
        message_type: 72,
        payload: vec![5],
    };
    let run = |fail_at: Option<usize>, release_fails: bool, with_release: bool| {
        let mut sent: Vec<(u8, bool)> = Vec::new();
        let mut pauses = 0usize;
        let result = send_paced_sequence(
            &messages,
            with_release.then_some(&release),
            Duration::from_millis(50),
            |message, is_release| {
                let id = message.payload[0];
                sent.push((id, is_release));
                if is_release && release_fails {
                    return Err("release write failed".into());
                }
                if !is_release && fail_at == Some(usize::from(id)) {
                    return Err("write failed".into());
                }
                Ok(())
            },
            |interval| {
                assert_eq!(interval, Duration::from_millis(50));
                pauses += 1;
            },
        );
        (result, sent, pauses)
    };

    // No failure: every frame once, in order, one pause between frames, no extra frame.
    let (result, sent, pauses) = run(None, false, true);
    assert_eq!(result, Ok(()));
    assert_eq!(sent, (0..6).map(|id| (id, false)).collect::<Vec<_>>());
    assert_eq!(pauses, 5);

    // A mid-gesture failure stops the stream, never retries the failed
    // frame, and sends the touch-up exactly once.
    let (result, sent, _) = run(Some(3), false, true);
    assert_eq!(result, Err("write failed; the touch was released".into()));
    assert_eq!(
        sent,
        vec![(0, false), (1, false), (2, false), (3, false), (5, true)]
    );

    // If the touch-up cannot be sent either, the error says so.
    let (result, sent, _) = run(Some(2), true, true);
    assert_eq!(
        result,
        Err("write failed; the touch-up could not be sent: release write failed".into())
    );
    assert_eq!(sent.iter().filter(|(_, release)| *release).count(), 1);

    // Nothing is known to be pressed when the first frame fails, so it gets
    // no release (a release is itself a click on some screens).
    let (result, sent, _) = run(Some(0), false, true);
    assert_eq!(result, Err("write failed".into()));
    assert_eq!(sent, vec![(0, false)]);
    // A failed final frame is the touch-up itself, and the touch-down was
    // accepted, so it gets exactly one second attempt: a hold's only cleanup.
    let (result, sent, _) = run(Some(5), false, true);
    assert_eq!(result, Err("write failed; the touch was released".into()));
    assert_eq!(sent.last(), Some(&(5, true)));
    assert_eq!(sent.iter().filter(|(_, release)| *release).count(), 1);
    let (result, sent, _) = run(Some(5), true, true);
    assert_eq!(
        result,
        Err(format!(
            "write failed; {TOUCH_UP_UNSENT}: release write failed"
        ))
    );
    assert_eq!(sent.iter().filter(|(_, release)| *release).count(), 1);

    // Without a release frame (every other sequence) the behavior is the
    // original one: stop at the first failure and report it unchanged.
    let (result, sent, _) = run(Some(3), false, false);
    assert_eq!(result, Err("write failed".into()));
    assert_eq!(sent.len(), 4);
    assert!(sent.iter().all(|(_, release)| !release));
}

#[test]
fn saturated_request_queue_rejects_without_waiting_or_enqueuing() {
    let (commands, receiver) = mpsc::sync_channel(1);
    commands.send(Command::Stop).unwrap();
    let (reply, _response) = mpsc::channel();
    let rejected = Command::Request {
        message_type: 13,
        payload: vec![],
        expected_type: 13,
        request_id: Some(42),
        admission: test_read_admission(1, Instant::now(), Duration::from_secs(1)),
        reply,
    };
    assert_eq!(
        queue_request(&commands, rejected).unwrap_err(),
        "QC command queue is full; the read was not submitted"
    );
    assert!(matches!(receiver.try_recv(), Ok(Command::Stop)));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn unobserved(sequence: u64, message_type: u16, payload: Vec<u8>) -> IncomingMessage {
    IncomingMessage {
        observation: None,
        sequence,
        message_type,
        payload,
        received_at_unix_ms: 0,
    }
}

#[test]
fn an_oversized_payload_is_not_cached_and_drops_the_superseded_entry() {
    let mut latest = HashMap::new();
    cache_latest_message(
        &mut latest,
        unobserved(1, profile::MESSAGE_TYPE_FILE, vec![8, 1]),
    );
    assert_eq!(latest[&profile::MESSAGE_TYPE_FILE].sequence, 1);
    cache_latest_message(
        &mut latest,
        unobserved(
            2,
            profile::MESSAGE_TYPE_FILE,
            vec![0; RETAINED_PAYLOAD_MAX_BYTES + 1],
        ),
    );
    assert!(
        !latest.contains_key(&profile::MESSAGE_TYPE_FILE),
        "a superseded publication must not keep answering reads"
    );
    cache_latest_message(
        &mut latest,
        unobserved(3, profile::MESSAGE_TYPE_FILE, vec![8, 2]),
    );
    assert_eq!(latest[&profile::MESSAGE_TYPE_FILE].payload, vec![8, 2]);

    // The connection's own copy follows the same budget, while the
    // receive counters still see the frame.
    let state = Arc::new(Mutex::new(BrokerStatus::default()));
    let mut connected = HashMap::new();
    update_message(
        &state,
        &mut connected,
        unobserved(4, 13, vec![0; RETAINED_PAYLOAD_MAX_BYTES + 1]),
    );
    assert!(connected.is_empty());
    assert_eq!(state.lock_recover().messages_received, 1);
    update_message(&state, &mut connected, unobserved(5, 13, vec![8, 1]));
    assert_eq!(connected[&13].sequence, 5);
}

#[test]
fn a_tagged_io_settings_publication_never_replaces_or_poisons_the_cached_one() {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        7,
        "original".into(),
        0,
    ));
    let io = |request_id: Option<u64>, out_ports: &[u32]| {
        pa::IoSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            request_id: request_id.map(pa::io_settings_message::RequestId::RequestId),
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    in_port: if out_ports.len() > 1 {
                        vec![pa::InputPortSettings {
                            input_port_id: 1,
                            ..Default::default()
                        }]
                    } else {
                        Vec::new()
                    },
                    out_port: out_ports
                        .iter()
                        .map(|&output_port_id| pa::OutputPortSettings {
                            output_port_id,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec()
    };
    let mut sequence = 0;
    let mut frame = |payload: Vec<u8>| {
        sequence += 1;
        IncomingMessage {
            observation: Some(owner.capture_observation(connection).unwrap()),
            sequence,
            message_type: profile::MESSAGE_TYPE_IO_SETTINGS,
            payload,
            received_at_unix_ms: 100 + u128::from(sequence),
        }
    };
    for request_id in [0, 7] {
        // A tagged first publication is not cached.
        let mut latest = HashMap::new();
        assert_eq!(
            cache_latest_message(&mut latest, frame(io(Some(request_id), &[1, 4]))),
            None
        );
        assert!(!latest.contains_key(&profile::MESSAGE_TYPE_IO_SETTINGS));

        // Full, then a tagged sparse update in the same session: the full one stays readable.
        let full = io(None, &[1, 4]);
        let mut latest = HashMap::new();
        assert_eq!(cache_latest_message(&mut latest, frame(full.clone())), None);
        assert_eq!(
            cache_latest_message(&mut latest, frame(io(Some(request_id), &[4]))),
            None
        );
        let cached = &latest[&profile::MESSAGE_TYPE_IO_SETTINGS];
        assert_eq!(cached.payload, full);
        assert!(quadcortex::responses::decode_io_settings(&cached.payload).is_ok());
    }
}

#[test]
fn sparse_general_settings_echo_merges_into_the_cached_snapshot() {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        7,
        "original".into(),
        0,
    ));
    let mut latest = HashMap::new();
    cache_latest_message(
        &mut latest,
        IncomingMessage {
            observation: Some(owner.capture_observation(connection).unwrap()),
            sequence: 7,
            message_type: profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            payload: pa::GeneralSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                led_brightness: Some(pa::general_settings_message::LedBrightness::LedBrightness(
                    100,
                )),
                master_volume_assignment: Some(
                    pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                        pa::MasterVolumeAssignmentOptions {
                            out12: true,
                            out34: true,
                            send12: true,
                            headphones: true,
                        },
                    ),
                ),
                enable_scene_dimmed: Some(
                    pa::general_settings_message::EnableSceneDimmed::EnableSceneDimmed(false),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
            received_at_unix_ms: 100,
        },
    );
    cache_latest_message(
        &mut latest,
        IncomingMessage {
            observation: Some(owner.capture_observation(connection).unwrap()),
            sequence: 8,
            message_type: profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            payload: pa::GeneralSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                enable_scene_dimmed: Some(
                    pa::general_settings_message::EnableSceneDimmed::EnableSceneDimmed(true),
                ),
                master_volume_assignment: Some(
                    pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                        pa::MasterVolumeAssignmentOptions::default(),
                    ),
                ),
                ..Default::default()
            }
            .encode_to_vec(),
            received_at_unix_ms: 101,
        },
    );

    let cached = latest
        .get(&profile::MESSAGE_TYPE_GENERAL_SETTINGS)
        .expect("type-9 snapshot cached");
    let decoded = pa::GeneralSettingsMessage::decode(cached.payload.as_slice()).unwrap();
    assert_eq!(cached.sequence, 8);
    assert_eq!(
        decoded.master_volume_assignment,
        Some(
            pa::general_settings_message::MasterVolumeAssignment::MasterVolumeAssignment(
                pa::MasterVolumeAssignmentOptions::default()
            )
        )
    );
    assert_eq!(decoded.action, pa::message_action::Enum::Update as i32);
    assert_eq!(
        decoded.led_brightness,
        Some(pa::general_settings_message::LedBrightness::LedBrightness(
            100
        ))
    );
    assert_eq!(
        decoded.enable_scene_dimmed,
        Some(pa::general_settings_message::EnableSceneDimmed::EnableSceneDimmed(true))
    );
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        8,
        "replacement".into(),
        2,
    ));
    let mut replacement = cached.clone();
    replacement.observation = Some(owner.capture_observation(connection).unwrap());
    replacement.payload = pa::GeneralSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        enable_scene_dimmed: Some(
            pa::general_settings_message::EnableSceneDimmed::EnableSceneDimmed(false),
        ),
        ..Default::default()
    }
    .encode_to_vec();
    cache_latest_message(&mut latest, replacement);
    let decoded = pa::GeneralSettingsMessage::decode(
        latest[&profile::MESSAGE_TYPE_GENERAL_SETTINGS]
            .payload
            .as_slice(),
    )
    .unwrap();
    assert!(
        decoded.led_brightness.is_none(),
        "a new session must not inherit an old field through a sparse echo"
    );
}

#[test]
fn sparse_io_echo_retains_input_ports_for_output_write_readback() {
    let mut owner = SessionRuntime::new(0);
    let connection = owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        7,
        "test".into(),
        0,
    ));
    let mut latest = HashMap::new();
    for (sequence, settings) in [
        pa::PortSettings {
            in_port: vec![pa::InputPortSettings {
                input_port_id: 0,
                ..Default::default()
            }],
            out_port: vec![pa::OutputPortSettings {
                output_port_id: 4,
                mute: Some(pa::output_port_settings::Mute::Mute(false)),
                ..Default::default()
            }],
            ..Default::default()
        },
        pa::PortSettings {
            out_port: vec![pa::OutputPortSettings {
                output_port_id: 4,
                mute: Some(pa::output_port_settings::Mute::Mute(true)),
                ..Default::default()
            }],
            ..Default::default()
        },
    ]
    .into_iter()
    .enumerate()
    {
        cache_latest_message(
            &mut latest,
            IncomingMessage {
                observation: Some(owner.capture_observation(connection).unwrap()),
                sequence: sequence as u64,
                message_type: profile::MESSAGE_TYPE_IO_SETTINGS,
                received_at_unix_ms: 0,
                payload: pa::IoSettingsMessage {
                    action: pa::message_action::Enum::Update as i32,
                    settings: Some(pa::io_settings_message::Settings::Settings(settings)),
                    ..Default::default()
                }
                .encode_to_vec(),
            },
        );
    }
    let projected = quadcortex::responses::decode_io_settings(
        &latest[&profile::MESSAGE_TYPE_IO_SETTINGS].payload,
    )
    .unwrap();
    assert_eq!(projected.inputs.len(), 1);
    assert_eq!(projected.outputs.len(), 1);
    assert_eq!(projected.outputs[0].muted, Some(true));
}

#[test]
fn scoped_id_reservation_does_not_retry_a_dropped_worker_reply() {
    let (commands, receiver) = mpsc::sync_channel::<Command>(4);
    let worker = FixtureThread::spawn(move || {
        let Command::ReserveRequestId { reply } =
            receiver.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("expected one request ID reservation")
        };
        drop(reply);
        assert!(
            receiver.recv_timeout(Duration::from_millis(20)).is_err(),
            "a failed reservation must not enqueue a second attempt"
        );
    });
    assert!(reserve_request_id_before(&commands, Instant::now() + Duration::from_secs(1)).is_err());
    worker.join();
}

fn started_backup(
    timeout: Duration,
) -> (
    SessionRuntime,
    BackupInProgress,
    mpsc::Receiver<Result<String, String>>,
) {
    let mut owner = SessionRuntime::new(0);
    owner.opened(0).unwrap();
    owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
        7,
        "backup".into(),
        0,
    ));
    let mut operation = owner.plan_operation(0, timeout.as_millis() as u64).unwrap();
    owner.begin_backup(&mut operation, 0).unwrap();
    let dispatch = owner.prepare_dispatch(Some(&operation), None, 0).unwrap();
    let delivery = owner
        .prepare_delivery(&dispatch, profile::MESSAGE_TYPE_LOCAL_BACKUP, 1, 0)
        .unwrap();
    owner.begin_native_report(&delivery, 0, 0).unwrap();
    owner
        .finish_native_report(
            &delivery,
            0,
            quadcortex::device::delivery::ReportOutcome::Delivered,
            0,
        )
        .unwrap();
    let (reply, receiver) = mpsc::channel();
    (owner, BackupInProgress { operation, reply }, receiver)
}

#[test]
fn paced_touch_gestures_use_the_sequence_lane_and_the_single_frame_tap_and_swipe_do_not() {
    // A tap is a single TAP frame: nothing to pace, so it is not a sequence.
    assert_eq!(
        quadcortex::device::request::operation_inter_message_interval_ms(
            &DeviceOperation::ScreenTap { x: 10.0, y: 20.0 }
        ),
        0
    );
    // A hold is touch-down, the hold interval, touch-up.
    assert_eq!(
        quadcortex::device::request::operation_inter_message_interval_ms(
            &DeviceOperation::ScreenHold { x: 10.0, y: 20.0 }
        ),
        quadcortex::profile::REMOTE_HOLD_INTERVAL_MS
    );
    // A swipe is a single DRAG frame: there is no gap to pace, so it leaves
    // the sequence lane and goes out as one plain write.
    assert_eq!(
        quadcortex::device::request::operation_inter_message_interval_ms(
            &DeviceOperation::ScreenDrag {
                x: 10.0,
                y: 20.0,
                to_x: 30.0,
                to_y: 40.0,
            }
        ),
        0
    );
    assert_eq!(
        quadcortex::device::request::operation_inter_message_interval_ms(
            &DeviceOperation::ScreenDragSteps {
                x: 10.0,
                y: 20.0,
                to_x: 30.0,
                to_y: 40.0,
                steps: 18,
                hold_ms: 200,
            }
        ),
        quadcortex::profile::REMOTE_DRAG_INTERVAL_MS
    );
    assert_eq!(
        quadcortex::device::request::operation_inter_message_interval_ms(&DeviceOperation::Undo),
        0
    );
}

#[test]
fn backup_traffic_defers_the_idle_keepalive_deadline() {
    let mut session = TransportRuntime::new(0);
    session.transport_opened(0);
    session.handshake_completed(1, true);
    // Backup commands own their own unconditional keepalive cadence. Each
    // one restarts the ordinary idle deadline without inventing a second
    // liveness-probe state machine.
    session.defer_keepalive(2);
    assert!(!session.keepalive_due(2));
    assert!(session.keepalive_due(2 + quadcortex::profile::KEEPALIVE_INTERVAL_MS));
}

#[test]
fn an_abandoned_session_reports_to_the_backup_caller() {
    let (_owner, backup, receiver) = started_backup(Duration::from_secs(600));
    let mut active = Some(backup);
    fail_backup(&mut active, "USB link lost: cable removed");
    assert!(active.is_none());
    assert_eq!(
        receiver.recv().expect("caller notified"),
        Err("USB link lost: cable removed".to_string())
    );
}

#[test]
fn poisoned_cache_locks_recover_without_cascading_worker_failure() {
    let value = Arc::new(Mutex::new(41_u32));
    let poison = Arc::clone(&value);
    let _ = thread::spawn(move || {
        let _guard = poison.lock().expect("initial lock");
        panic!("test poison");
    })
    .join();

    *value.lock_recover() += 1;
    assert_eq!(*value.lock_recover(), 42);
}

fn library_lifecycle_frame(
    owner: &mut SessionRuntime,
    connection: u64,
    kind: u16,
    payload: &[u8],
    now: u64,
) {
    let ticket = owner
        .capture_observation_with_ingress(
            connection,
            Some(quadcortex::device::transport::IngressTime {
                first_report_at_ms: now,
                last_report_at_ms: now,
            }),
        )
        .unwrap();
    assert!(owner.observe_lifecycle(&ticket, kind, payload, now));
}

pub(super) fn boot_library_owner(
    owner: &mut SessionRuntime,
    connection: u64,
    session: &str,
    now: u64,
    finish: bool,
) {
    let reset = pa::ResetCommsBuffersMessage {
        session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
            session.into(),
        )),
        ..Default::default()
    }
    .encode_to_vec();
    let version = pa::VersionMessage {
        action: 1,
        device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
            pa::version_message::DeviceType::Qc as i32,
        )),
        cortex_control_version_valid: Some(
            pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
        ),
        ..Default::default()
    }
    .encode_to_vec();
    let models = pa::ModelRepoMessage {
        action: 1,
        model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
            br#"<Models><Category name="Drive"><Model id="1" name="Test" /></Category></Models>"#
                .to_vec(),
        )),
        ..Default::default()
    }
    .encode_to_vec();
    let stats = pa::ModuleStatsMessage {
        action: 1,
        stats: vec![pa::ModuleStatsItem::default()],
        ..Default::default()
    }
    .encode_to_vec();
    for (kind, payload) in [(52, reset), (10, version), (51, models), (35, stats)] {
        let driver = owner.lifecycle.as_mut().unwrap();
        while let Some(batch) = driver.next_batch(now, now) {
            assert!(driver.dispatched(
                &batch.ticket,
                quadcortex::device::session::DispatchOutcome::Accepted
            ));
        }
        library_lifecycle_frame(owner, connection, kind, &payload, now);
    }
    assert_eq!(
        owner.lifecycle.as_ref().unwrap().phase(),
        quadcortex::device::initialization::DeviceStartupPhase::Booting
    );
    let driver = owner.lifecycle.as_mut().unwrap();
    while let Some(batch) = driver.next_batch(now, now) {
        assert!(driver.dispatched(
            &batch.ticket,
            quadcortex::device::session::DispatchOutcome::Accepted
        ));
    }
    if finish {
        library_lifecycle_frame(owner, connection, 60, &[8, 1], now);
    }
}

#[test]
fn library_publication_wait_rejects_replacement_and_expired_cached_reads() {
    use prost::Message;
    use quadcortex::proto::cortex_protobuf_v2 as pa;
    for (kind, root, wire_type) in [
        (
            quadcortex::device::native_library::LibraryKind::Capture,
            "local_nc_root",
            2,
        ),
        (
            quadcortex::device::native_library::LibraryKind::Ir,
            "local_ir_root",
            1,
        ),
    ] {
        let (controller, commands) = refresh_controller();
        let controller = Arc::new(controller);
        let waiting = Arc::clone(&controller);
        let (done, result) = mpsc::channel();
        let waiter = FixtureThread::spawn(move || {
            done.send(waiting.wait_for_library_folder(kind, root, Duration::from_secs(2)))
                .unwrap();
        });
        let started = Instant::now();
        while controller.raw_events.subscriber_count() == 0 {
            assert!(started.elapsed() < Duration::from_secs(1));
            thread::yield_now();
        }
        {
            let mut owner = controller.session_owner.lock_recover();
            let connection = owner.opened(1).unwrap();
            owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
                8,
                "replacement".into(),
                1,
            ));
            let observed_ms = (controller.session_clock.elapsed().as_millis() as u64).max(1);
            boot_library_owner(&mut owner, connection, "replacement", observed_ms, true);
            let ticket = owner
                .capture_observation_with_ingress(
                    connection,
                    Some(quadcortex::device::transport::IngressTime {
                        first_report_at_ms: observed_ms,
                        last_report_at_ms: observed_ms,
                    }),
                )
                .unwrap();
            let payload = pa::FileMessage {
                action: pa::message_action::Enum::Create as i32,
                r#type: Some(pa::file_message::Type::Type(wire_type)),
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(root.into())),
                    files: vec![pa::ProductData {
                        key: Some(pa::product_data::Key::Key("new-session-only".into())),
                        name: Some(pa::product_data::Name::Name("New session".into())),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec();
            assert!(owner.observe_lifecycle(&ticket, 4, &payload, observed_ms));
            owner.observe_state(&ticket, 4, &payload).unwrap();
        }
        assert!(result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err()
            .contains("session changed"));
        waiter.join();
        assert!(controller
            .wait_for_library_folder(kind, root, Duration::ZERO)
            .unwrap()
            .is_none());
        let fresh_call = controller
            .wait_for_library_folder(kind, root, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(fresh_call.entries[0].key, "new-session-only");
        assert!(commands.try_recv().is_err());
    }
}

/// Stands in for `QcUsb`: dropping it is the orderly teardown write, and
/// like the real one it takes a moment (the HID writer join).
struct FakeUsb {
    wire: Arc<Mutex<Vec<&'static str>>>,
}

impl Drop for FakeUsb {
    fn drop(&mut self) {
        thread::sleep(Duration::from_millis(250));
        self.wire.lock_recover().push("connection(false)");
    }
}

/// A controller whose USB worker is `worker`, on its own named thread,
/// exactly where `start_for_device_with_lease` keeps the real one.
fn controller_with_worker(
    worker: impl FnOnce(mpsc::Receiver<Command>) + Send + 'static,
) -> DeviceController {
    DeviceController::with_worker_for_test(worker)
}

/// Mirrors `run`: the connection lives in the worker's frame and is
/// dropped when the worker returns on `Stop`.
fn fake_run(receiver: mpsc::Receiver<Command>, wire: Arc<Mutex<Vec<&'static str>>>) {
    let _connection = FakeUsb { wire };
    while let Ok(command) = receiver.recv() {
        if matches!(command, Command::Stop) {
            return;
        }
    }
}

#[test]
fn dropping_the_controller_returns_only_after_the_disconnect_write() {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&wire);
    let controller = controller_with_worker(move |receiver| fake_run(receiver, recorded));
    let shutdown = controller.shutdown_handle();
    finishes_within(WORKER_EXIT_BUDGET + Duration::from_secs(2), move || {
        drop(controller)
    });
    assert_eq!(
        *wire.lock_recover(),
        ["connection(false)"],
        "Drop must not return before the worker's connection teardown is written"
    );
    // Joined once; repeating it reports that outcome and waits for nothing.
    let started = Instant::now();
    assert_eq!(
        shutdown.stop_and_join(WORKER_EXIT_BUDGET),
        WorkerExit::Joined
    );
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn a_joined_worker_does_not_certify_an_unverified_native_teardown() {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&wire);
    let controller = controller_with_worker(move |receiver| fake_run(receiver, recorded));
    controller
        .worker
        .teardown_verified
        .store(false, Ordering::Release);
    assert_eq!(
        controller.stop_worker_for_exit(WORKER_EXIT_BUDGET),
        WorkerExit::TeardownUnverified,
    );
    assert_eq!(*wire.lock_recover(), ["connection(false)"]);
    assert!(!WorkerExit::TeardownUnverified.verified_clean_disconnect());
    assert_eq!(
        controller.stop_worker_for_exit(Duration::ZERO),
        WorkerExit::TeardownUnverified,
    );
}

#[test]
fn explicit_disconnect_propagates_native_teardown_evidence_without_claiming_release() {
    for verified in [true, false] {
        let (controller, commands) = refresh_controller();
        thread::scope(|scope| {
            let caller = scope.spawn(|| controller.disconnect());
            let Command::Disconnect { reply, .. } =
                commands.recv_timeout(Duration::from_secs(1)).unwrap()
            else {
                panic!("expected disconnect");
            };
            reply.send(native_disconnect_result(verified)).unwrap();
            let outcome = caller.join().unwrap();
            if verified {
                assert!(outcome.is_ok());
            } else {
                assert!(outcome.unwrap_err().contains("not verified"));
            }
        });
    }
}

#[test]
fn stop_worker_for_exit_writes_the_disconnect_and_is_idempotent() {
    let wire = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&wire);
    let controller = controller_with_worker(move |receiver| fake_run(receiver, recorded));
    assert_eq!(
        controller.stop_worker_for_exit(WORKER_EXIT_BUDGET),
        WorkerExit::Joined
    );
    assert_eq!(*wire.lock_recover(), ["connection(false)"]);
    let started = Instant::now();
    assert_eq!(
        controller.stop_worker_for_exit(WORKER_EXIT_BUDGET),
        WorkerExit::Joined,
        "a second stop reports the recorded outcome"
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(*wire.lock_recover(), ["connection(false)"]);
    // A controller with no worker thread at all is left alone.
    let (bare, commands) = refresh_controller();
    assert_eq!(
        bare.stop_worker_for_exit(WORKER_EXIT_BUDGET),
        WorkerExit::NoWorker
    );
    assert!(commands.try_recv().is_err(), "nothing was queued for it");
}

#[test]
fn a_stuck_worker_cannot_hold_drop_past_its_budget() {
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_unwind = ReleaseOnDrop(Arc::clone(&release));
    let stuck = Arc::clone(&release);
    let controller = controller_with_worker(move |_receiver| {
        // Never reads `Stop`: a worker wedged inside the OS.
        while !stuck.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(10));
        }
    });
    let started = Instant::now();
    finishes_within(WORKER_EXIT_BUDGET + Duration::from_secs(2), move || {
        drop(controller)
    });
    let elapsed = started.elapsed();
    release.store(true, Ordering::Release);
    assert!(
        elapsed >= WORKER_EXIT_BUDGET - Duration::from_millis(100),
        "Drop gave up early: {elapsed:?}"
    );
    assert!(
        elapsed < WORKER_EXIT_BUDGET + Duration::from_secs(1),
        "Drop overran its budget: {elapsed:?}"
    );
}

#[test]
fn a_timed_out_original_worker_is_later_joined_without_repeating_its_wait_budget() {
    struct RetireOnDrop {
        release: Arc<AtomicBool>,
        shutdown: WorkerShutdown,
    }
    impl Drop for RetireOnDrop {
        fn drop(&mut self) {
            self.release.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self
                .shutdown
                .stop_and_join(Duration::ZERO)
                .verified_clean_disconnect()
            {
                if Instant::now() >= deadline {
                    eprintln!(
                        "controlled worker cleanup not acknowledged; original remains retained"
                    );
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_unwind = ReleaseOnDrop(Arc::clone(&release));
    let (entered, ready) = mpsc::channel();
    let held = Arc::clone(&release);
    let controller = controller_with_worker(move |_receiver| {
        entered.send(()).unwrap();
        while !held.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
    });
    let shutdown = controller.shutdown_handle();
    let _retire_on_unwind = RetireOnDrop {
        release: Arc::clone(&release),
        shutdown: shutdown.clone(),
    };
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        shutdown.stop_and_join(Duration::from_millis(20)),
        WorkerExit::TimedOut
    );
    let repeated = Instant::now();
    assert_eq!(
        shutdown.stop_and_join(Duration::from_secs(5)),
        WorkerExit::TimedOut
    );
    assert!(repeated.elapsed() < Duration::from_millis(100));
    drop(controller);
    release.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match shutdown.stop_and_join(Duration::ZERO) {
            WorkerExit::Joined => break,
            WorkerExit::TimedOut => {
                assert!(
                    Instant::now() < deadline,
                    "exact original never acknowledged retirement"
                );
                thread::sleep(Duration::from_millis(1));
            }
            exit => panic!("unexpected original worker retirement: {exit:?}"),
        }
    }
    assert_eq!(shutdown.stop_and_join(Duration::ZERO), WorkerExit::Joined);
}

/// A worker wedged inside the OS: never reads `Stop`, returns only when
/// the test releases it.
fn stuck_worker(release: &Arc<AtomicBool>) -> DeviceController {
    DeviceController::with_stuck_worker_for_test(Arc::clone(release))
}

#[test]
fn several_stuck_workers_share_one_shutdown_deadline() {
    let budget = Duration::from_millis(600);
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_unwind = ReleaseOnDrop(Arc::clone(&release));
    let controllers: Vec<_> = (0..4).map(|_| stuck_worker(&release)).collect();
    let started = Instant::now();
    let report = stop_workers_within(controllers.iter(), budget);
    let elapsed = started.elapsed();
    assert!(
        elapsed < budget + Duration::from_millis(400),
        "four stuck workers took {elapsed:?}: one shared budget, not one each"
    );
    assert!(elapsed >= budget - Duration::from_millis(50), "{elapsed:?}");
    assert_eq!(report.outcomes.len(), 4);
    assert!(report
        .outcomes
        .iter()
        .all(|(_, exit)| *exit == WorkerExit::TimedOut));
    assert_eq!(report.unverified().len(), 4);
    assert!(!report.all_verified());
    assert!(
        report.summary().contains("NOT verified for 4"),
        "{}",
        report.summary()
    );
    // Already given up on: their own Drop must not add another budget.
    let dropping = Instant::now();
    drop(controllers);
    assert!(
        dropping.elapsed() < Duration::from_millis(200),
        "Drop waited again: {:?}",
        dropping.elapsed()
    );
    release.store(true, Ordering::Release);
}

#[test]
fn a_fast_and_a_stuck_worker_are_told_apart_within_one_budget() {
    let budget = Duration::from_millis(800);
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_unwind = ReleaseOnDrop(Arc::clone(&release));
    let wire = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&wire);
    let fast = controller_with_worker(move |receiver| fake_run(receiver, recorded));
    let stuck = stuck_worker(&release);
    let started = Instant::now();
    let report = stop_workers_within([&stuck, &fast], budget);
    let elapsed = started.elapsed();
    release.store(true, Ordering::Release);
    assert!(elapsed < budget + Duration::from_millis(400), "{elapsed:?}");
    assert_eq!(report.outcomes[0].1, WorkerExit::TimedOut);
    assert_eq!(report.outcomes[1].1, WorkerExit::Joined);
    assert!(report.outcomes[1].1.verified_clean_disconnect());
    assert!(!report.outcomes[0].1.verified_clean_disconnect());
    assert_eq!(*wire.lock_recover(), ["connection(false)"]);
    assert_eq!(report.unverified().len(), 1);
    assert!(
        report.summary().contains("NOT verified for 1"),
        "{}",
        report.summary()
    );
    assert!(
        report.summary().contains("verified for 1"),
        "{}",
        report.summary()
    );
}

#[test]
fn controllers_without_a_worker_get_nothing_queued_and_count_as_nothing_to_verify() {
    let (bare, commands) = refresh_controller();
    let report = stop_workers_within([&bare], Duration::from_millis(50));
    assert_eq!(report.outcomes[0].1, WorkerExit::NoWorker);
    assert!(report.all_verified());
    assert!(commands.try_recv().is_err());
}

#[test]
fn a_worker_that_stops_itself_is_not_waited_on() {
    let (shutdown_sender, shutdown_receiver) = mpsc::channel::<WorkerShutdown>();
    let (outcome_sender, outcome) = mpsc::channel();
    let controller = controller_with_worker(move |_receiver| {
        let shutdown = shutdown_receiver.recv().unwrap();
        let _ = outcome_sender.send(shutdown.stop_and_join(WORKER_EXIT_BUDGET));
    });
    let started = Instant::now();
    shutdown_sender.send(controller.shutdown_handle()).unwrap();
    assert_eq!(
        outcome.recv_timeout(Duration::from_secs(2)).unwrap(),
        WorkerExit::OnWorkerThread
    );
    assert!(started.elapsed() < WORKER_EXIT_BUDGET);
    drop(controller);
}

/// Reduce one complete 256-slot preset folder through the owner, with
/// ingress already finished at `observed_ms`.
fn publish_preset_folder(owner: &mut SessionRuntime, connection: u64, key: &str, observed_ms: u64) {
    let payload = pa::FileMessage {
        action: pa::message_action::Enum::Create as i32,
        folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
            key: Some(pa::folder_info::Key::Key(key.into())),
            files: (0..256)
                .map(|index| pa::ProductData {
                    index: Some(pa::product_data::Index::Index(index)),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })),
        ..Default::default()
    }
    .encode_to_vec();
    let ticket = owner
        .capture_observation_with_ingress(
            connection,
            Some(quadcortex::device::transport::IngressTime {
                first_report_at_ms: observed_ms,
                last_report_at_ms: observed_ms,
            }),
        )
        .unwrap();
    owner
        .observe_state(&ticket, profile::MESSAGE_TYPE_FILE, &payload)
        .unwrap();
}

/// `refresh_controller`, booted to `Connected` with `folders` retained.
pub(super) fn connected_catalog_controller(
    folders: &[&str],
) -> (DeviceController, mpsc::Receiver<Command>) {
    let (controller, commands) = refresh_controller();
    {
        let mut owner = controller.session_owner.lock_recover();
        boot_library_owner(&mut owner, 1, "original", 0, true);
        for folder in folders {
            publish_preset_folder(&mut owner, 1, folder, 0);
        }
    }
    (controller, commands)
}

/// The key Git Bash's MSYS path conversion made of
/// `/media/p4/Presets/My Presets` (2026-10-06).
const MANGLED_SETLIST_KEY: &str = "C:/Program Files/Git/media/p4/Presets/My Presets";

#[test]
fn an_unknown_setlist_key_in_a_complete_directory_fails_without_waiting() {
    let (controller, _commands) =
        connected_catalog_controller(&["/media/p4/Presets/Other", "/media/p4/Presets/Live"]);
    let timeout = Duration::from_secs(5);
    let started = Instant::now();
    let unpublished = controller
        .wait_for_preset_catalog("device.listPresets", Some(MANGLED_SETLIST_KEY), timeout)
        .unwrap()
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "an unknown key must not wait out the {timeout:?} budget: {elapsed:?}"
    );
    assert!(unpublished.setlist_key_unknown);
    assert_eq!(
        unpublished.setlist_key.as_deref(),
        Some(MANGLED_SETLIST_KEY)
    );
    assert_eq!(
        unpublished.absence.reason,
        quadcortex::device::owner::PresetCatalogAbsenceReason::SetlistKeyMissing
    );
    assert_eq!(unpublished.absence.known_setlists, Some(2));
    assert!(unpublished.waited_ms < 500, "{}", unpublished.waited_ms);
    let json = serde_json::to_value(&*unpublished).unwrap();
    assert_eq!(json["setlistKey"], MANGLED_SETLIST_KEY);
    assert_eq!(json["setlistKeyUnknown"], true);
    assert_eq!(json["knownSetlists"], 2);
    assert_eq!(json["reason"], "setlistKeyMissing");
    for name in ["Other", "Live"] {
        assert!(!json.to_string().contains(name), "{name} leaked: {json}");
    }
}

#[test]
fn a_known_setlist_key_in_a_complete_directory_is_still_served() {
    let (controller, _commands) = connected_catalog_controller(&["/media/p4/Presets/Other"]);
    let value = controller
        .wait_for_preset_catalog(
            "device.listPresets",
            Some("/media/p4/Presets/Other"),
            Duration::from_secs(5),
        )
        .unwrap()
        .unwrap();
    assert_eq!(value["setlistKey"], "/media/p4/Presets/Other");
    assert_eq!(value["presets"].as_array().unwrap().len(), 256);
}

#[test]
fn a_catalog_that_is_not_yet_complete_still_waits_for_the_key() {
    // Mid-Booting, with part of the directory retained: the key may yet
    // arrive, so the read waits out its budget exactly as before.
    let (controller, _commands) = refresh_controller();
    publish_preset_folder(
        &mut controller.session_owner.lock_recover(),
        1,
        "/media/p4/Presets/Other",
        0,
    );
    // Connected, but nothing retained: no directory proves the key wrong.
    let (empty, _empty_commands) = connected_catalog_controller(&[]);
    for (controller, reason) in [
        (
            &controller,
            quadcortex::device::owner::PresetCatalogAbsenceReason::SetlistKeyMissing,
        ),
        (
            &empty,
            quadcortex::device::owner::PresetCatalogAbsenceReason::NoCatalogFrame,
        ),
    ] {
        let timeout = Duration::from_millis(150);
        let unpublished = controller
            .wait_for_preset_catalog(
                "device.listPresets",
                Some("/media/p4/Presets/My Presets"),
                timeout,
            )
            .unwrap()
            .unwrap_err();
        assert!(unpublished.waited_ms >= 150, "{}", unpublished.waited_ms);
        assert!(!unpublished.setlist_key_unknown);
        assert_eq!(unpublished.absence.reason, reason);
        assert_eq!(
            unpublished.setlist_key.as_deref(),
            Some("/media/p4/Presets/My Presets")
        );
    }
}

#[test]
fn an_unpublished_catalog_says_why_with_this_connections_counters() {
    let (controller, _commands) = refresh_controller();
    {
        let mut status = controller.state.lock_recover();
        status.messages_received_by_type = HashMap::from([
            (profile::MESSAGE_TYPE_FILE, 585),
            (profile::MESSAGE_TYPE_CONNECTION, 2),
        ]);
        status.messages_sent_by_type = HashMap::from([(profile::MESSAGE_TYPE_FILE, 1)]);
        status.session_epoch = 3;
    }
    let timeout = Duration::from_millis(120);
    let unpublished = controller
        .wait_for_preset_catalog(
            "device.listPresets",
            Some("/media/p4/Presets/My Presets"),
            timeout,
        )
        .unwrap()
        .unwrap_err();
    assert_eq!(unpublished.method, "device.listPresets");
    assert_eq!(
        unpublished.absence.reason,
        quadcortex::device::owner::PresetCatalogAbsenceReason::NoCatalogFrame
    );
    assert_eq!(unpublished.file_frames_received, 585);
    assert_eq!(unpublished.file_messages_sent, 1);
    assert_eq!(unpublished.connection_frames_received, 2);
    assert_eq!(unpublished.session_epoch, 3);
    assert_eq!(unpublished.timeout_ms, 120);
    assert!(unpublished.waited_ms >= 120, "{}", unpublished.waited_ms);
    let json = serde_json::to_value(&*unpublished).unwrap();
    assert_eq!(json["reason"], "noCatalogFrame");
    assert_eq!(json["fileFramesReceived"], 585);
    assert_eq!(json["fileFramesObserved"], 0);
    assert_eq!(json["sessionRevision"], 0);
    assert_eq!(json["waitedMs"].as_u64(), Some(unpublished.waited_ms));
}

#[test]
fn slot_publication_waits_for_coverage_without_usb_and_rejects_replacement() {
    for replace_session in [false, true] {
        let (controller, commands) = refresh_controller();
        let controller = Arc::new(controller);
        let key = "/media/p4/Presets/Test";
        let publish = |owner: &mut SessionRuntime, connection, count| {
            use prost::Message;
            use quadcortex::proto::cortex_protobuf_v2 as pa;
            let payload = pa::FileMessage {
                action: pa::message_action::Enum::Create as i32,
                folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                    key: Some(pa::folder_info::Key::Key(key.into())),
                    files: (0..count)
                        .map(|index| pa::ProductData {
                            index: Some(pa::product_data::Index::Index(index)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                })),
                ..Default::default()
            }
            .encode_to_vec();
            let ticket = owner
                .capture_observation_with_ingress(
                    connection,
                    Some(quadcortex::device::transport::IngressTime {
                        first_report_at_ms: 0,
                        last_report_at_ms: 0,
                    }),
                )
                .unwrap();
            owner.observe_state(&ticket, 4, &payload).unwrap();
        };
        {
            let mut owner = controller.session_owner.lock_recover();
            use prost::Message;
            use quadcortex::proto::cortex_protobuf_v2 as pa;
            let ticket = owner.capture_observation(1).unwrap();
            let preset = pa::RecallPresetMessage {
                action: pa::message_action::Enum::Update as i32,
                preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
                ..Default::default()
            }
            .encode_to_vec();
            owner.observe_state(&ticket, 15, &preset).unwrap();
            owner.state.snapshot_mut().setlist_key = key.into();
            owner.state.snapshot_mut().has_preset = true;
            publish(&mut owner, 1, 1);
        }
        let waiting = Arc::clone(&controller);
        let (done, result) = mpsc::channel();
        let waiter = FixtureThread::spawn(move || {
            done.send(waiting.wait_for_preset_catalog(
                "device.listPresetSlots",
                None,
                Duration::from_secs(2),
            ))
            .unwrap();
        });
        let started = Instant::now();
        while controller.raw_events.subscriber_count() == 0 {
            assert!(started.elapsed() < Duration::from_secs(1));
            thread::yield_now();
        }
        // Subscription is installed after capture of the original context.
        assert!(result.try_recv().is_err());
        assert!(commands.try_recv().is_err());
        {
            let mut owner = controller.session_owner.lock_recover();
            let connection = if replace_session {
                let connection = owner.opened(1).unwrap();
                owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
                    8,
                    "replacement".into(),
                    1,
                ));
                owner.state.snapshot_mut().setlist_key = key.into();
                connection
            } else {
                1
            };
            publish(&mut owner, connection, 256);
        }
        let value = result.recv_timeout(Duration::from_secs(2)).unwrap();
        if replace_session {
            assert!(value.unwrap_err().contains("session changed"));
        } else {
            assert_eq!(
                value.unwrap().unwrap()["slots"].as_array().unwrap().len(),
                256
            );
        }
        assert!(commands.try_recv().is_err());
        waiter.join();
    }
}

#[test]
fn authoritative_saved_catalog_entry_updates_the_active_snapshot_name() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    {
        use prost::Message;
        use quadcortex::proto::cortex_protobuf_v2 as pa;
        let mut owner = controller.session_owner.lock_recover();
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "test".into(),
            0,
        ));
        let ticket = owner.capture_observation(connection).unwrap();
        let payload = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(
                quadcortex::proto::BinaryPreset::default(),
            )),
            ..Default::default()
        }
        .encode_to_vec();
        owner.observe_state(&ticket, 15, &payload).unwrap();
        let snapshot = owner.state.snapshot_mut();
        snapshot.setlist_key = "/media/p4/Presets/My Presets".into();
        snapshot.preset_position = 15;
        snapshot.preset_name = "Unsaved".into();
        let payload = pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(
                    "/media/p4/Presets/My Presets".into(),
                )),
                files: vec![pa::ProductData {
                    index: Some(pa::product_data::Index::Index(15)),
                    key: Some(pa::product_data::Key::Key("renamed-key".into())),
                    name: Some(pa::product_data::Name::Name("Renamed".into())),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec();
        let ticket = owner.capture_observation(connection).unwrap();
        owner
            .observe_state(&ticket, profile::MESSAGE_TYPE_FILE, &payload)
            .unwrap();
    }
    assert_eq!(
        controller.gateway_snapshot().unwrap().preset_name,
        "Renamed"
    );
}

#[test]
fn terminal_session_write_failures_clear_ready_status_before_reconnect() {
    for incident in [
        "keepalive-write-failure",
        "backup-keepalive-write-failure",
        "updater-relay-write-failure",
    ] {
        let (controller, _commands) = refresh_controller();
        {
            let mut status = controller.state.lock_recover();
            status.phase = "ready".into();
            status.connected = true;
            status.synchronized = true;
            status.device_name = Some("Previously connected device".into());
        }
        teardown_after_session_failure(
            "controlled write failure",
            100,
            incident,
            &controller.state,
            &controller.latest_messages,
            &mut Vec::new(),
            &mut None,
            &controller.session_owner,
            &mut None,
        );
        let status = controller.status();
        assert_eq!(status.phase, "searching");
        assert!(!status.connected);
        assert!(!status.synchronized);
        assert!(status.device_name.is_none());
        assert!(status.detail.contains("controlled write failure"));
        assert!(!controller
            .session_owner
            .lock_recover()
            .transport
            .is_connected());
    }
}

#[test]
fn dropped_state_subscriptions_release_membership_without_device_traffic() {
    let (controller, _commands) = refresh_controller();
    let baseline = controller.state_subscribers.lock_recover().len();
    let retained = controller.subscribe_state_events();
    for _ in 0..512 {
        let subscription = controller.subscribe_state_events();
        assert_eq!(
            controller.state_subscribers.lock_recover().len(),
            baseline + 2
        );
        drop(subscription);
        assert_eq!(
            controller.state_subscribers.lock_recover().len(),
            baseline + 1
        );
    }
    drop(retained);
    assert_eq!(controller.state_subscribers.lock_recover().len(), baseline);
    assert_eq!(controller.state_event_sequence.load(Ordering::Acquire), 0);
}

#[test]
fn stalled_state_subscribers_are_bounded_and_do_not_block_publication() {
    let event_log = Arc::new(Mutex::new(StateEventJournal::default()));
    let latest_sequence = Arc::new(AtomicU64::new(0));
    let (subscriber, receiver) = budgeted_queue(
        QueueKind::State,
        1,
        STATE_SUBSCRIBER_QUEUE_MAX_BYTES,
        Arc::default(),
    );
    let subscribers = Arc::new(Mutex::new(vec![StateSubscriber {
        identity: Arc::new(()),
        sender: subscriber,
    }]));
    let frame = DecodedStateFrame {
        stream_id: "test-stream".into(),
        sequence: 1,
        observed_at: 0,
        states: Vec::new(),
        tempo_clock: None,
    };

    publish_state_frame(&event_log, &latest_sequence, &subscribers, frame.clone());
    publish_state_frame(
        &event_log,
        &latest_sequence,
        &subscribers,
        DecodedStateFrame {
            stream_id: "test-stream".into(),
            sequence: 2,
            ..frame
        },
    );

    assert_eq!(receiver.try_recv().expect("first queued frame").sequence, 1);
    assert!(receiver.try_recv().is_err());
    assert_eq!(event_log.lock_recover().len(), 2);
    assert_eq!(latest_sequence.load(Ordering::Acquire), 2);
}

#[test]
fn state_event_history_is_bounded_without_rewinding_its_cursor() {
    let event_log = Arc::new(Mutex::new(StateEventJournal::default()));
    let latest_sequence = Arc::new(AtomicU64::new(0));
    let subscribers = Arc::new(Mutex::new(Vec::new()));
    for sequence in 1..=(STATE_EVENT_MAXIMUM_LIMIT as u64 + 2) {
        publish_state_frame(
            &event_log,
            &latest_sequence,
            &subscribers,
            DecodedStateFrame {
                stream_id: "test-stream".into(),
                sequence,
                observed_at: 0,
                states: Vec::new(),
                tempo_clock: None,
            },
        );
    }
    let mut log = event_log.lock_recover();
    assert_eq!(log.len(), STATE_EVENT_MAXIMUM_LIMIT);
    assert_eq!(log.front().expect("oldest retained frame").sequence, 3);
    assert_eq!(log.back().expect("newest retained frame").sequence, 4098);
    log.clear();
    drop(log);
    assert_eq!(latest_sequence.load(Ordering::Acquire), 4098);
}

#[test]
fn state_event_pages_are_ordered_strictly_after_the_cursor_and_limited() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    controller
        .state_event_log
        .lock_recover()
        .extend((7..=11).map(|sequence| DecodedStateFrame {
            stream_id: controller.state_stream_id.clone(),
            sequence,
            observed_at: u128::from(sequence),
            states: Vec::new(),
            tempo_clock: None,
        }));
    controller.state_event_sequence.store(11, Ordering::Release);
    let page = controller.state_event_page(8, 2);
    assert_eq!(
        page.frames
            .iter()
            .map(|frame| frame.sequence)
            .collect::<Vec<_>>(),
        vec![9, 10]
    );
    assert_eq!(page.latest_sequence, 11);
    assert!(!page.stream_id.is_empty());
    assert!(page
        .frames
        .iter()
        .all(|frame| frame.stream_id == page.stream_id));
    controller.state_event_log.lock_recover().clear();
    let cleared = controller.state_event_page(11, 2);
    assert_eq!(cleared.stream_id, page.stream_id);
    assert_eq!(cleared.latest_sequence, 11);
    assert!(cleared.frames.is_empty());
    let replacement = DeviceController::start_disconnected().unwrap();
    let fresh = replacement.state_event_page(11, 2);
    assert_ne!(fresh.stream_id, page.stream_id);
    assert_eq!(fresh.latest_sequence, 0);
}

#[test]
fn oversized_final_state_frame_advances_tail_without_filling_subscribers() {
    let controller = DeviceController::start_disconnected().unwrap();
    let subscriber = controller.subscribe_state_events();
    publish_state_frame(
        &controller.state_event_log,
        &controller.state_event_sequence,
        &controller.state_subscribers,
        DecodedStateFrame {
            stream_id: controller.state_stream_id.clone(),
            sequence: 1,
            observed_at: 0,
            states: vec![serde_json::from_value(serde_json::json!({
                "kind": "preset",
                "presetName": "x".repeat(quadcortex::domain::STATE_EVENT_PAGE_MAXIMUM_BYTES)
            }))
            .unwrap()],
            tempo_clock: None,
        },
    );
    let page = controller.state_event_page(0, 4096);
    assert_eq!(page.latest_sequence, 1);
    assert!(page.frames.is_empty());
    assert!(subscriber.try_recv().is_err());
    assert!(
        serde_json::to_vec(&page).unwrap().len()
            < quadcortex::domain::STATE_EVENT_PAGE_MAXIMUM_BYTES
    );
}

#[test]
fn tempo_clock_cache_is_distinct_from_the_latest_type_33_envelope() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    *controller.latest_tempo_clock.lock_recover() = Some(DecodedStateFrame {
        stream_id: "test-stream".into(),
        sequence: 7,
        observed_at: 123_456,
        states: Vec::new(),
        tempo_clock: Some(TempoClockFrame {
            enabled: true,
            preroll_enabled: false,
            current_beat: 2,
            current_bar: 3,
            current_tick: 4,
            count_in_beats_remaining: -1,
            count_in_bars_remaining: 0,
        }),
    });
    controller.latest_messages.lock_recover().insert(
        quadcortex::profile::MESSAGE_TYPE_GLOBAL_TEMPO,
        IncomingMessage {
            observation: None,
            sequence: 8,
            message_type: quadcortex::profile::MESSAGE_TYPE_GLOBAL_TEMPO,
            payload: vec![0x08, 0x01],
            received_at_unix_ms: 123_457,
        },
    );
    let cached = controller.latest_tempo_clock().expect("clock cached");
    assert_eq!(cached.sequence, 7);
    assert_eq!(cached.tempo_clock.unwrap().current_tick, 4);
    assert!(controller
        .latest_message(quadcortex::profile::MESSAGE_TYPE_GLOBAL_TEMPO)
        .is_some());
}

#[test]
fn master_volume_cache_is_distinct_from_the_bounded_state_journal() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    let mut state = StateUpdate::empty("master");
    state.master_volume = Some(0.57);
    *controller.latest_master_volume.lock_recover() = Some(DecodedStateFrame {
        stream_id: "test-stream".into(),
        sequence: 7,
        observed_at: 123_456,
        states: vec![state],
        tempo_clock: None,
    });
    controller.state_event_log.lock_recover().clear();

    let cached = controller.latest_master_volume().expect("volume cached");
    assert_eq!(cached.sequence, 7);
    assert_eq!(cached.observed_at, 123_456);
    assert_eq!(cached.states[0].master_volume, Some(0.57));
}

#[test]
fn default_status_distinguishes_searching_from_ready() {
    let status = BrokerStatus::default();
    assert_eq!(status.phase, "searching");
    assert!(!status.connected);
    assert!(!status.synchronized);
}

#[test]
fn published_settings_readback_requires_its_original_observation_ticket() {
    let controller = DeviceController::start_disconnected().unwrap();
    let (scope, old_ticket, ticket) = {
        let mut owner = controller.session_owner.lock_recover();
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "original".into(),
            0,
        ));
        let old = owner.capture_observation(connection).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            8,
            "replacement".into(),
            1,
        ));
        (
            owner.begin_gateway_read_scope(0, 30_000).unwrap(),
            old,
            owner.capture_observation(connection).unwrap(),
        )
    };
    let mut message = IncomingMessage {
        observation: Some(old_ticket),
        sequence: 1,
        message_type: profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        payload: pa::GeneralSettingsMessage {
            action: pa::message_action::Enum::Update as i32,
            ..Default::default()
        }
        .encode_to_vec(),
        received_at_unix_ms: 0,
    };
    controller
        .latest_messages
        .lock_recover()
        .insert(message.message_type, message.clone());
    assert!(controller
        .published_gateway_read_in_scope(
            &scope,
            message.message_type,
            runtime_request::GatewayResponseProjection::GeneralSettings
        )
        .unwrap_err()
        .contains("original write session"));
    message.observation = Some(ticket);
    controller
        .latest_messages
        .lock_recover()
        .insert(message.message_type, message.clone());
    assert!(controller
        .published_gateway_read_in_scope(
            &scope,
            message.message_type,
            runtime_request::GatewayResponseProjection::GeneralSettings
        )
        .is_ok());
    controller.session_owner.lock_recover().reset_state();
    assert!(controller
        .published_gateway_read_in_scope(
            &scope,
            message.message_type,
            runtime_request::GatewayResponseProjection::GeneralSettings
        )
        .is_err());
}

#[test]
fn logical_read_scope_rejects_stale_or_expired_followups_before_reserving_an_id() {
    let mut controller = DeviceController::start_disconnected().unwrap();
    let (commands, queued) = mpsc::sync_channel(2);
    let original_commands = std::mem::replace(&mut controller.commands, commands);
    let (stale, expired) = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "original".into(),
            0,
        ));
        let stale = owner.begin_gateway_read_scope(0, 30_000).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            8,
            "replacement".into(),
            1,
        ));
        (stale, owner.begin_gateway_read_scope(0, 0).unwrap())
    };
    assert!(controller
        .gateway_read_in_scope(
            &stale,
            "device.presetTempoSettings",
            &serde_json::Value::Null
        )
        .unwrap_err()
        .contains("session changed"));
    assert!(controller
        .gateway_read_in_scope(
            &expired,
            "device.presetTempoSettings",
            &serde_json::Value::Null
        )
        .unwrap_err()
        .contains("budget expired"));
    assert!(matches!(queued.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert!(controller.raw_events.subscriber_count() == 0);
    controller.commands = original_commands;
}

#[test]
fn write_verification_adapter_rejects_replacement_state_and_cancels_on_drop() {
    let controller = DeviceController::start_disconnected().unwrap();
    let context = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "original".into(),
            0,
        ));
        owner.session_context().unwrap()
    };
    let policy = runtime_request::gateway_verification_policy(100);
    let wait = controller
        .begin_gateway_verification(
            &context,
            GatewayVerification::Scene { scene: 1 },
            5,
            &policy,
        )
        .unwrap();
    let id = wait.id;
    {
        let mut owner = controller.session_owner.lock_recover();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            8,
            "replacement".into(),
            1,
        ));
        owner.state.snapshot_mut().active_scene = 1;
    }
    assert!(wait.advance(6).err().unwrap().contains("session changed"));
    drop(wait);
    assert!(controller
        .session_owner
        .lock_recover()
        .advance_gateway_verification(id, 6, 1)
        .is_err());
    assert!(controller
        .begin_gateway_verification(
            &context,
            GatewayVerification::Scene { scene: 1 },
            5,
            &policy
        )
        .is_err());
    let current = controller
        .session_owner
        .lock_recover()
        .session_context()
        .unwrap();
    let wait = controller
        .begin_gateway_verification(
            &current,
            GatewayVerification::Scene { scene: 2 },
            6,
            &policy,
        )
        .unwrap();
    let id = wait.id;
    drop(wait);
    assert!(controller
        .session_owner
        .lock_recover()
        .advance_gateway_verification(id, 7, 1)
        .is_err());
}

#[test]
fn write_verification_adapter_returns_the_verified_snapshot_and_honors_timeout() {
    let controller = DeviceController::start_disconnected().unwrap();
    let context = {
        let mut owner = controller.session_owner.lock_recover();
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "original".into(),
            0,
        ));
        owner.state.snapshot_mut().active_scene = 1;
        owner.session_context().unwrap()
    };
    let wait = controller
        .begin_gateway_verification(
            &context,
            GatewayVerification::Scene { scene: 1 },
            5,
            &runtime_request::gateway_verification_policy(100),
        )
        .unwrap();
    assert_ne!(
        wait.advance(5).unwrap().action,
        GatewayVerificationAction::Verified
    );
    let verified = wait.advance(6).unwrap();
    assert_eq!(verified.action, GatewayVerificationAction::Verified);
    controller
        .session_owner
        .lock_recover()
        .state
        .snapshot_mut()
        .active_scene = 2;
    assert_eq!(verified.snapshot.unwrap().active_scene, 1);
    let expired = controller
        .begin_gateway_verification(
            &context,
            GatewayVerification::Scene { scene: 2 },
            6,
            &runtime_request::gateway_verification_policy(0),
        )
        .unwrap();
    let step = expired.advance(7).unwrap();
    assert_eq!(step.action, GatewayVerificationAction::TimedOut);
    assert!(step.snapshot.is_none());
}

#[test]
fn verification_id_reservation_never_blocks_on_a_full_queue_or_restarts_its_budget() {
    let (commands, queued) = mpsc::sync_channel(1);
    assert!(reserve_request_id_before(&commands, Instant::now())
        .unwrap_err()
        .contains("expired before"));
    assert!(matches!(queued.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let error = reserve_request_id_before(&commands, Instant::now() + Duration::from_millis(10))
        .unwrap_err();
    assert!(error.contains("expired while"));
    // The sole queued reservation fills this bounded queue; there is no retry.
    assert!(
        reserve_request_id_before(&commands, Instant::now() + Duration::from_secs(5))
            .unwrap_err()
            .contains("cannot admit")
    );
    assert!(matches!(
        queued.try_recv(),
        Ok(Command::ReserveRequestId { .. })
    ));
    assert!(matches!(queued.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn preset_observation_does_not_bypass_shared_readiness() {
    let state = Arc::new(Mutex::new(BrokerStatus {
        phase: "syncing".into(),
        detail: "Waiting for authoritative state seed".into(),
        connected: true,
        ..BrokerStatus::default()
    }));
    let mut latest = HashMap::new();
    update_message(
        &state,
        &mut latest,
        IncomingMessage {
            observation: None,
            sequence: 1,
            message_type: quadcortex::profile::MESSAGE_TYPE_RECALL_PRESET,
            payload: Vec::new(),
            received_at_unix_ms: 0,
        },
    );

    let status = state.lock_recover();
    assert_eq!(status.phase, "syncing");
    assert_eq!(status.detail, "Waiting for authoritative state seed");
    assert!(status.connected);
    assert!(!status.synchronized);
}

#[test]
fn realtime_queue_rejects_a_disconnected_session_before_acceptance() {
    let controller = DeviceController::start_disconnected().expect("controller starts");
    let error = controller
        .send_realtime_command(commands::select_scene(1))
        .expect_err("disconnected realtime send must fail");
    assert!(error.contains("not connected"));
}

#[test]
fn verified_detail_projection_requires_the_complete_original_native_stamp() {
    use quadcortex::generated_payloads::NativeStateVersion;
    let mut current = GatewaySnapshot {
        state_version: Some(NativeStateVersion {
            session: "owner-session:epoch-1".into(),
            revision: 7,
        }),
        ..Default::default()
    };
    let result = serde_json::json!({"snapshot":{"stateVersion":current.state_version}});
    assert!(validate_verified_projection_version(&current, &result).is_ok());
    for (session, revision) in [
        ("replacement-session:epoch-1", 7),
        ("owner-session:epoch-2", 7),
        ("owner-session:epoch-1", 8),
    ] {
        current.state_version = Some(NativeStateVersion {
            session: session.into(),
            revision,
        });
        assert!(validate_verified_projection_version(&current, &result).is_err());
    }
    current.state_version = None;
    assert!(validate_verified_projection_version(&current, &result).is_err());
    current.state_version = Some(NativeStateVersion {
        session: "owner-session:epoch-1".into(),
        revision: 7,
    });
    for missing in [
        serde_json::json!({}),
        serde_json::json!({"snapshot":{"stateVersion":null}}),
        serde_json::json!({"snapshot":{"stateVersion":{"session":"owner-session:epoch-1"}}}),
    ] {
        assert!(validate_verified_projection_version(&current, &missing).is_err());
    }
}

#[test]
fn no_op_copy_returns_only_the_preset_from_its_captured_owner_context() {
    use prost::Message;
    use quadcortex::proto::cortex_protobuf_v2 as pa;
    let (controller, _commands) = refresh_controller();
    let seed = |owner: &mut SessionRuntime, connection: u64, name: &str| {
        let ticket = owner.capture_observation(connection).unwrap();
        let bytes = pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(Default::default())),
            ..Default::default()
        }
        .encode_to_vec();
        owner.observe_state(&ticket, 15, &bytes).unwrap();
        owner.state.snapshot_mut().preset_name = name.into();
        owner.state.snapshot_mut().has_preset = true;
    };
    let original = {
        let mut owner = controller.session_owner.lock_recover();
        seed(&mut owner, 1, "Original no-op preset");
        owner.session_context().unwrap()
    };
    assert_eq!(
        controller
            .gateway_snapshot_for_plan(&original)
            .unwrap()
            .preset_name,
        "Original no-op preset"
    );
    controller.session_owner.lock_recover().reset_state();
    assert!(controller.gateway_snapshot_for_plan(&original).is_err());
    let replacement = {
        let mut owner = controller.session_owner.lock_recover();
        let connection = owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            8,
            "replacement".into(),
            0,
        ));
        seed(&mut owner, connection, "Replacement preset");
        owner.session_context().unwrap()
    };
    assert!(controller.gateway_snapshot_for_plan(&original).is_err());
    assert_eq!(
        controller
            .gateway_snapshot_for_plan(&replacement)
            .unwrap()
            .preset_name,
        "Replacement preset"
    );
}

#[test]
fn gateway_snapshot_reduces_native_updates_without_python() {
    let mut snapshot = GatewaySnapshot::default();
    let mut position = StateUpdate::empty("position");
    position.position = Some(17);
    position.setlist_key = Some("/media/p4/Presets/Live/".into());
    snapshot.apply(&position);

    let mut preset = StateUpdate::empty("preset");
    preset.preset_name = Some("Direct Rust".into());
    preset.tempo = Some(96);
    preset.scenes = Some(
        (b'A'..=b'H')
            .map(|letter| format!("Scene {}", letter as char))
            .collect(),
    );
    preset.blocks = Some(Vec::new());
    preset.routes = Some(Vec::new());
    snapshot.apply(&preset);

    let mut volume = StateUpdate::empty("master");
    volume.master_volume = Some(0.57);
    snapshot.apply(&volume);

    assert!(snapshot.has_preset);
    assert_eq!(snapshot.preset_name, "Direct Rust");
    assert_eq!(snapshot.preset_location, "3B");
    assert_eq!(snapshot.setlist_name, "Live");
    assert_eq!(snapshot.tempo, 96);
    assert_eq!(snapshot.master_volume, 57);
}

#[test]
fn normal_reconnect_does_not_restart_an_active_or_connecting_session() {
    for phase in ["connecting", "handshaking", "syncing", "ready"] {
        assert_eq!(
            reconnect_outcome(false, phase == "ready", phase),
            ReconnectOutcome::AlreadyConnected
        );
    }
    assert_eq!(
        reconnect_outcome(false, false, "searching"),
        ReconnectOutcome::Restarted
    );
    assert_eq!(
        reconnect_outcome(false, false, "disconnected"),
        ReconnectOutcome::Restarted
    );
}

#[test]
fn forced_reset_never_reuses_the_current_worker_epoch() {
    assert_eq!(
        reconnect_outcome(true, true, "ready"),
        ReconnectOutcome::Restarted
    );
    assert_eq!(
        reconnect_outcome(true, false, "connecting"),
        ReconnectOutcome::Restarted
    );
}

#[test]
fn a_reset_resume_restarts_the_session_and_reports_why() {
    let (controller, _commands) = refresh_controller();
    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_SUSPEND_CONNECTION,
        &[0x10, 0x01, 0x18, 0x01],
    );
    assert!(controller.status().device_suspended);
    controller.observe_device_frame_for_test(
        profile::MESSAGE_TYPE_SUSPEND_CONNECTION,
        &[0x10, 0x00, 0x18, 0x01],
    );
    assert!(!controller.status().device_suspended);
    let reason = controller
        .session_owner
        .lock_recover()
        .take_suspension_resync()
        .expect("a factory-reset resume asks for a resync");
    assert_eq!(reason, "FACTORY_RESET");

    let (state_messages, decoder) = mpsc::sync_channel(4);
    let mut connection = None;
    let mut pending = Vec::new();
    let mut backup = None;
    restart_session(
        &format!("Quad Cortex resumed after {reason}; resynchronizing the session"),
        &mut connection,
        &mut pending,
        &mut backup,
        &controller.state,
        &controller.latest_messages,
        &controller.session_owner,
        &controller.session_clock,
        &state_messages,
        &controller.updater_relay,
        &mut CloudForwardRelay::default(),
    );
    let status = controller.status();
    assert_eq!(status.phase, "searching");
    assert!(status.detail.contains("FACTORY_RESET"), "{}", status.detail);
    assert!(!controller.session_owner.lock_recover().is_connected());
    assert!(matches!(decoder.try_recv(), Ok(StateDecoderCommand::Reset)));
}

/// A fetcher that never answers until released: a request that stays running.
struct StalledFetcher(Arc<AtomicBool>);

impl HttpFetcher for StalledFetcher {
    fn get(&self, _request: &FetchRequest<'_>) -> Result<FetchedHop, FetchFailure> {
        while !self.0.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        Err(FetchFailure::Io)
    }
}

/// A type-61 CHECK request as the unit sends it.
fn relay_check_frame(updater_request_id: u64) -> Vec<u8> {
    pa::UpdaterForwardMessage {
        action: pa::message_action::Enum::Update as i32,
        updater_request_id,
        request_id: None,
        request: Some(pa::updater_forward_message::Request::Request(
            pa::UpdaterForwardRequest {
                r#type: pa::updater_forward_request_type::Enum::Check as i32,
                url: "https://cloud-api.neuraldsp.com/firmware/v1/latest".into(),
                timeout: 60_000,
                headers: Vec::new(),
                payload: None,
            },
        )),
        response: None,
    }
    .encode_to_vec()
}

#[test]
fn a_restarted_session_ends_the_update_relay_request_it_was_carrying() {
    use crate::updater_relay::Disposition;
    // The restart makes the reconnect due at once, so the loop's no-session
    // branch, which used to be the only place the relay was cancelled, never
    // runs: the old request kept streaming into the new session.
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_unwind = ReleaseOnDrop(Arc::clone(&release));
    let relay = UpdaterRelay::with_parts(
        Arc::new(StalledFetcher(Arc::clone(&release))),
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(
        relay.on_forward_frame(&relay_check_frame(41), Instant::now()),
        Disposition::Started
    );
    assert!(relay.is_active());

    let (controller, _commands) = refresh_controller();
    let (state_messages, _decoder) = mpsc::sync_channel(4);
    let mut connection = None;
    let mut pending = Vec::new();
    let mut backup = None;
    restart_session(
        "Quad Cortex resumed after FACTORY_RESET; resynchronizing the session",
        &mut connection,
        &mut pending,
        &mut backup,
        &controller.state,
        &controller.latest_messages,
        &controller.session_owner,
        &controller.session_clock,
        &state_messages,
        &relay,
        &mut CloudForwardRelay::default(),
    );
    assert!(!relay.is_active(), "the old request outlived its session");
    assert!(
        relay.poll_outbound().is_none(),
        "and nothing of it is left to send"
    );
    release.store(true, Ordering::Release);
}

#[test]
fn a_new_connection_epoch_clears_all_prior_transport_telemetry() {
    let state = Arc::new(Mutex::new(BrokerStatus {
        phase: "ready".into(),
        detail: "old session".into(),
        connected: true,
        synchronized: true,
        active_preset_name: Some("Old preset".into()),
        active_preset_location: Some("7F".into()),
        device_name: Some("Old device".into()),
        active_scene: Some(7),
        connected_at_unix_ms: Some(123),
        handshake_ms: Some(456),
        messages_received: 98,
        messages_sent: 76,
        write_delivery: quadcortex_hid::WriteDeliveryCounts {
            admitted: 80,
            started: 77,
            completed: 76,
            failed: 1,
            cancelled: 3,
        },
        messages_received_by_type: HashMap::from([(49, 12), (51, 4)]),
        messages_sent_by_type: HashMap::from([(4, 9), (52, 1)]),
        inbound_telemetry: InboundTelemetrySnapshot {
            frames: 16,
            unacted_frames: 1,
            no_state_handler_frames: 1,
            undeclared_types: BTreeMap::from([(73, 1)]),
            no_state_handler_types: BTreeMap::from([(8, 1)]),
            last_unacted: Some("undeclared message type 73".into()),
            ..Default::default()
        },
        retention: Default::default(),
        writes_quarantined: true,
        write_quarantine_reason: Some("ambiguous write".into()),
        write_quarantine: None,
        device_suspended: true,
        suspension: Default::default(),
        expected_write_stalls: 5,
        last_hid_write_duration_ms: 6,
        max_hid_write_duration_ms: 7,
        last_hid_write_completed: true,
        reader_lock_stall_events: 2,
        max_reader_lock_stall_ms: 42,
        last_message_type: Some(49),
        session_epoch: 3,
        updater_relay: Default::default(),
    }));

    begin_connection_epoch(&state, "Opening native QC USB session");

    let status = state.lock_recover();
    assert_eq!(status.phase, "connecting");
    assert_eq!(status.detail, "Opening native QC USB session");
    assert!(!status.connected);
    assert!(!status.synchronized);
    assert_eq!(status.active_preset_name, None);
    assert_eq!(status.active_preset_location, None);
    assert_eq!(status.device_name, None);
    assert_eq!(status.active_scene, None);
    assert_eq!(status.connected_at_unix_ms, None);
    assert_eq!(status.handshake_ms, None);
    assert_eq!(status.messages_received, 0);
    assert_eq!(status.messages_sent, 0);
    assert_eq!(status.write_delivery.admitted, 0);
    assert_eq!(status.write_delivery.completed, 0);
    assert!(!status.device_suspended);
    assert!(status.writes_quarantined);
    assert_eq!(
        status.write_quarantine_reason.as_deref(),
        Some("ambiguous write")
    );
    assert!(status.messages_received_by_type.is_empty());
    assert!(status.messages_sent_by_type.is_empty());
    // What the host could not act on is a receive total like any other:
    // a status consumer must never read a prior connection's.
    assert_eq!(
        status.inbound_telemetry,
        InboundTelemetrySnapshot::default()
    );
    assert_eq!(status.expected_write_stalls, 0);
    assert_eq!(status.last_hid_write_duration_ms, 0);
    assert_eq!(status.max_hid_write_duration_ms, 0);
    assert!(!status.last_hid_write_completed);
    assert_eq!(status.reader_lock_stall_events, 0);
    assert_eq!(status.max_reader_lock_stall_ms, 0);
    assert_eq!(status.last_message_type, None);
}

fn synchronized_session() -> Arc<Mutex<BrokerStatus>> {
    let state = Arc::new(Mutex::new(BrokerStatus::default()));
    {
        let mut status = state.lock_recover();
        status.phase = "ready".into();
        status.detail = "Active preset synchronized".into();
        status.connected = true;
        status.synchronized = true;
        status.connected_at_unix_ms = Some(1_789_201_977_481);
    }
    state
}

#[test]
fn an_in_session_rebuild_advances_the_status_epoch() {
    // A rebuild keeps the transport timestamp but must invalidate queued
    // host admission tied to the previous synchronized state.
    let state = synchronized_session();
    let before = state.lock_recover().session_epoch;
    let stamped_at = state.lock_recover().connected_at_unix_ms;

    update_lifecycle_status(&state, true, false);

    let status = state.lock_recover();
    assert_eq!(
        status.connected_at_unix_ms, stamped_at,
        "one transport session: lastSync must not move on a rebuild"
    );
    assert!(status.connected, "a rebuild is not a disconnect");
    assert_eq!(status.phase, "syncing");
    assert_ne!(
        status.session_epoch, before,
        "the rebuild must retire the previous admission epoch"
    );
}

#[test]
fn opening_a_transport_still_advances_the_status_epoch() {
    // Whatever else changes, the original invalidation point must survive.
    let state = synchronized_session();
    let before = state.lock_recover().session_epoch;
    begin_connection_epoch(&state, "Opening native QC USB session");
    assert_ne!(state.lock_recover().session_epoch, before);
}

/// A running worker whose retry finds nothing to open: the selected id matches
/// no unit HID can enumerate, so no hardware attached to the machine can make
/// these tests open a device. The status is seeded with a finished connection's
/// counters before the first retry is armed, so nothing races the seed.
fn controller_searching_for_an_absent_qc(tag: &str) -> (DeviceController, WorkerRootGuard, String) {
    let permit = FixtureAdmissions::global()
        .reserve()
        .expect("fixture root admission");
    let root = crate::device_lease::testing::fresh_root(&format!("absent-{tag}"));
    let path = root.0.join("device-owner.json");
    let discoveries = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&discoveries);
    let controller = DeviceController::start_for_device_with_test_discovery(
        Some("no-such-device".into()),
        false,
        path.clone(),
        move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(vec![quadcortex_hid::usb::QcUsbDevice {
                id: "other-test-device".into(),
                name: "Different test device".into(),
            }])
        },
    )
    .expect("controller starts");
    let root_guard = WorkerRootGuard {
        shutdown: controller.shutdown_handle(),
        root,
        permit: Some(permit),
    };
    {
        let mut status = controller.hold_status_lock_for_test();
        status.handshake_ms = Some(456);
        status.messages_received = 98;
        status.messages_sent = 76;
        status.session_epoch = 3;
    }
    assert!(matches!(
        controller.reconnect(),
        Ok(ReconnectOutcome::Restarted)
    ));
    let expected = crate::device_lease::busy_detail(&path);
    wait_for_status(&controller, "the no-device search", |status| {
        status.detail == expected
    });
    assert!(discoveries.load(Ordering::SeqCst) > 0);
    (controller, root_guard, expected)
}

#[test]
fn a_retry_that_finds_no_qc_stays_searching_and_opens_no_connection_epoch() {
    let (controller, _root_guard, expected) = controller_searching_for_an_absent_qc("phase");
    // Several retry cadences (RECONNECT_INTERVAL_MS is 750 ms) pass; the worker
    // enumerates on each and finds nothing, so it never publishes `connecting`
    // and never starts a connection epoch.
    thread::sleep(Duration::from_millis(2_000));
    let status = controller.status();
    assert_eq!(status.phase, "searching");
    assert_eq!(status.detail, expected);
    assert!(!status.connected);
    assert!(!status.synchronized);
    assert_eq!(
        status.session_epoch, 3,
        "only begin_connection_epoch bumps the epoch, and it now runs after a QC is found"
    );
    assert_eq!(
        controller.device_id().as_deref(),
        Some("no-such-device"),
        "a selected unit that is missing is never replaced by another one"
    );
    drop(controller);
}

/// Characterization of what 2badc68 changed as a side effect: with no QC
/// enumerated the retry no longer reaches `begin_connection_epoch`, the one
/// place that zeroes the transport counters, so the previous connection's
/// totals stay in the status while the phase reads `searching`. If the
/// maintainers decide those totals must clear on an unplug, this is the
/// assertion to turn around.
#[test]
fn a_retry_that_finds_no_qc_leaves_the_previous_connections_counters_in_place() {
    let (controller, _root_guard, _) = controller_searching_for_an_absent_qc("counters");
    thread::sleep(Duration::from_millis(2_000));
    let status = controller.status();
    assert_eq!(status.phase, "searching");
    assert_eq!(status.handshake_ms, Some(456));
    assert_eq!(status.messages_received, 98);
    assert_eq!(status.messages_sent, 76);
    drop(controller);
}

#[test]
fn installed_navigation_coordinates_cross_real_rpc_as_one_drag_frame() {
    use quadcortex::device::delivery::ReportOutcome;
    let cases = [
        (
            100,
            440,
            "08011a160d0000c843150000c8421804250000c8432d0000dc43",
        ),
        (0, 479, "08011a110d0000c8431804250000c8432d0080ef43"),
    ];
    assert_eq!(cases.len(), 2);
    let methods = [
        ("device.swipeScreen", false),
        ("device.dragScreen", false),
        ("device.dragScreen", true),
    ];
    assert_eq!(methods.len(), 3);
    for (y, to_y, hex) in cases {
        let expected: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        for (method, options) in methods {
            // This fixture constructs an owner and command channel only. It
            // never spawns a broker/worker or enumerates/opens a HID device.
            let (controller, receiver) = refresh_controller();
            let mut params = serde_json::json!({"x":400,"y":y,"toX":400,"toY":to_y});
            if options {
                params["steps"] = serde_json::json!(30);
                params["holdMs"] = serde_json::json!(1450);
            }
            thread::scope(|scope| {
                let caller = scope.spawn(|| {
                    crate::rpc::dispatch_gesture_regression_request(&controller, method, params)
                });
                let deadline = Instant::now() + Duration::from_secs(2);
                let (message_type, payload, operation, reply) = loop {
                    let command = receiver
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .expect("RPC must submit one native frame before its deadline");
                    match command {
                        Command::RecordFlight { .. } => continue,
                        Command::Send(kind, bytes, operation, reply) => break (kind, bytes, operation, reply),
                        _ => panic!("gesture dispatched a read, lifecycle action, paced sequence or other unexpected command"),
                    }
                };
                assert_eq!(message_type, 72);
                // Independent literal protobuf includes UPDATE=1, DRAG=4,
                // exact endpoints and no outer request id. Any PRESS/RELEASE
                // or builder/mapping substitution changes this assertion.
                assert_eq!(
                    payload, expected,
                    "{method} ({y}->{to_y}), options={options}"
                );
                {
                    let mut owner = controller.session_owner.lock_recover();
                    let now = controller.session_clock.elapsed().as_millis() as u64;
                    let dispatch = owner.prepare_dispatch(Some(&operation), None, now).unwrap();
                    let delivery = owner
                        .prepare_delivery(&dispatch, message_type, 1, now)
                        .unwrap();
                    owner.delivery_admitted(&delivery).unwrap();
                    owner.begin_native_report(&delivery, 0, now).unwrap();
                    owner
                        .finish_native_report(&delivery, 0, ReportOutcome::Delivered, now)
                        .unwrap();
                }
                // Queue admission alone is insufficient: the real dispatcher
                // waits for the owner's completed native-delivery journal.
                reply.send(Ok(())).unwrap();
                let response = caller.join().expect("RPC thread must not panic");
                assert!(response.get("error").is_none(), "{response}");
                assert_eq!(response["result"]["accepted"], true);
                assert_eq!(response["result"]["verified"], false);
                assert_eq!(response["result"]["verification"], "accepted_unverified");
                assert!(controller.touch_guard.lock_recover().held().is_none());
                while let Ok(command) = receiver.try_recv() {
                    assert!(matches!(command, Command::RecordFlight { .. }),
                        "no follow-up read, release, retry or second native write may follow the single DRAG");
                }
            });
        }
    }
}

#[test]
fn retained_general_gateway_rejects_replacement_during_decode_and_accepts_fresh_ticket() {
    use prost::Message;
    let (controller, _commands) = DeviceController::live_session_for_test();
    let payload = pa::GeneralSettingsMessage {
        action: pa::message_action::Enum::Update as i32,
        ..Default::default()
    }
    .encode_to_vec();
    let publish = || {
        let ticket = {
            let mut owner = controller.session_owner.lock_recover();
            let connection = serde_json::to_value(owner.session_fence().unwrap()).unwrap()
                ["connection"]
                .as_u64()
                .unwrap();
            owner.capture_observation(connection).unwrap()
        };
        controller.latest_messages.lock_recover().insert(
            profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            IncomingMessage {
                observation: Some(ticket),
                sequence: 1,
                message_type: profile::MESSAGE_TYPE_GENERAL_SETTINGS,
                payload: payload.clone(),
                received_at_unix_ms: 0,
            },
        );
    };
    publish();
    let expected = runtime_request::GatewayResponseProjection::GeneralSettings
        .decode(&payload)
        .unwrap();
    assert_eq!(
        crate::rpc::gateway_general_settings_with(&controller, |bytes| {
            runtime_request::GatewayResponseProjection::GeneralSettings.decode(bytes)
        })
        .unwrap(),
        expected
    );
    let result = crate::rpc::gateway_general_settings_with(&controller, |bytes| {
        let decoded = runtime_request::GatewayResponseProjection::GeneralSettings.decode(bytes)?;
        let mut owner = controller.session_owner.lock_recover();
        owner.disconnect(0, false);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            8,
            "replacement-general-session".into(),
            0,
        ));
        drop(owner);
        publish();
        Ok(decoded)
    });
    assert!(
        result.is_err(),
        "a replacement's fresh publication cannot certify the old decoded result"
    );
    assert_eq!(
        crate::rpc::gateway_general_settings_with(&controller, |bytes| {
            runtime_request::GatewayResponseProjection::GeneralSettings.decode(bytes)
        })
        .unwrap(),
        expected
    );
}

#[test]
fn retained_general_gateway_rejects_ancillary_old_ticket_and_invalidated_owner() {
    use prost::Message;
    let (controller, _commands) = DeviceController::live_session_for_test();
    let (connection, ticket) = {
        let mut owner = controller.session_owner.lock_recover();
        let connection = serde_json::to_value(owner.session_fence().unwrap()).unwrap()
            ["connection"]
            .as_u64()
            .unwrap();
        (connection, owner.capture_observation(connection).unwrap())
    };
    controller.latest_messages.lock_recover().insert(
        profile::MESSAGE_TYPE_GENERAL_SETTINGS,
        IncomingMessage {
            observation: Some(ticket),
            sequence: 1,
            message_type: profile::MESSAGE_TYPE_GENERAL_SETTINGS,
            payload: pa::GeneralSettingsMessage {
                action: pa::message_action::Enum::Update as i32,
                ..Default::default()
            }
            .encode_to_vec(),
            received_at_unix_ms: 0,
        },
    );
    {
        let mut owner = controller.session_owner.lock_recover();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            9,
            "replacement-with-stale-cache".into(),
            0,
        ));
        assert!(owner.capture_observation(connection).is_ok());
    }
    assert!(
        controller
            .latest_message(profile::MESSAGE_TYPE_GENERAL_SETTINGS)
            .is_some(),
        "old ancillary bytes remain deliberately present"
    );
    assert!(
        crate::rpc::gateway_general_settings_with(&controller, |bytes| {
            runtime_request::GatewayResponseProjection::GeneralSettings.decode(bytes)
        })
        .is_err(),
        "new scope cannot certify old observation ticket"
    );
    controller.session_owner.lock_recover().reset_state();
    let mut decoded = false;
    let result = crate::rpc::gateway_general_settings_with(&controller, |_| {
        decoded = true;
        Ok(serde_json::Value::Null)
    });
    assert!(result.is_err());
    assert!(
        !decoded,
        "invalidated owner refuses read admission before decoding the stale cache"
    );
}

#[test]
fn retained_io_gateway_rejects_replacement_during_decode_and_accepts_fresh_reads() {
    use prost::Message;
    use quadcortex::proto::cortex_protobuf_v2 as pa;
    let (controller, _commands) = DeviceController::live_session_for_test();
    let payload = pa::IoSettingsMessage {
        action: 1,
        settings: Some(pa::io_settings_message::Settings::Settings(
            pa::PortSettings {
                in_port: vec![pa::InputPortSettings {
                    input_port_id: 1,
                    plugged: Some(pa::input_port_settings::Plugged::Plugged(true)),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )),
        ..Default::default()
    }
    .encode_to_vec();
    controller.observe_device_frame_for_test(profile::MESSAGE_TYPE_IO_SETTINGS, &payload);
    let expected = runtime_request::GatewayResponseProjection::IoSettings
        .decode(&payload)
        .unwrap();
    assert_eq!(
        crate::rpc::gateway_io_settings_with(&controller, |bytes| {
            runtime_request::GatewayResponseProjection::IoSettings.decode(bytes)
        })
        .unwrap(),
        expected
    );
    let result = crate::rpc::gateway_io_settings_with(&controller, |bytes| {
        let decoded = runtime_request::GatewayResponseProjection::IoSettings.decode(bytes)?;
        let mut owner = controller.session_owner.lock_recover();
        owner.disconnect(0, false);
        owner.opened(0).unwrap();
        owner.lifecycle = Some(quadcortex::device::session::SessionDriver::start(
            7,
            "replacement-io-session".into(),
            0,
        ));
        drop(owner);
        controller.observe_device_frame_for_test(profile::MESSAGE_TYPE_IO_SETTINGS, &payload);
        Ok(decoded)
    });
    assert!(result.unwrap_err().contains("session changed"));
    assert_eq!(
        crate::rpc::gateway_io_settings_with(&controller, |bytes| {
            runtime_request::GatewayResponseProjection::IoSettings.decode(bytes)
        })
        .unwrap(),
        expected
    );
}

#[test]
fn retained_io_gateway_refuses_invalidated_owner_even_when_latest_cache_has_old_bytes() {
    use prost::Message;
    use quadcortex::proto::cortex_protobuf_v2 as pa;
    let (controller, _commands) = DeviceController::live_session_for_test();
    let payload = |count: usize| {
        pa::IoSettingsMessage {
            action: 1,
            settings: Some(pa::io_settings_message::Settings::Settings(
                pa::PortSettings {
                    in_port: (0..count)
                        .map(|id| pa::InputPortSettings {
                            input_port_id: id as u32,
                            plugged: Some(pa::input_port_settings::Plugged::Plugged(true)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }
        .encode_to_vec()
    };
    controller.observe_device_frame_for_test(profile::MESSAGE_TYPE_IO_SETTINGS, &payload(1));
    let mut cached = raw_frame(1, 0);
    cached.message_type = profile::MESSAGE_TYPE_IO_SETTINGS;
    cached.payload = payload(1);
    assert_eq!(
        cache_latest_message(&mut controller.latest_messages.lock_recover(), cached),
        None
    );
    assert!(controller
        .latest_message(profile::MESSAGE_TYPE_IO_SETTINGS)
        .is_some());
    assert!(crate::rpc::gateway_io_settings_with(&controller, |bytes| {
        runtime_request::GatewayResponseProjection::IoSettings.decode(bytes)
    })
    .is_ok());
    {
        let mut owner = controller.session_owner.lock_recover();
        let connection = serde_json::to_value(owner.session_fence().unwrap()).unwrap()
            ["connection"]
            .as_u64()
            .unwrap();
        let ticket = owner.capture_observation(connection).unwrap();
        assert!(owner
            .observe_state(
                &ticket,
                profile::MESSAGE_TYPE_IO_SETTINGS,
                &payload(quadcortex::responses::MAX_RETAINED_IO_RECORDS + 1),
            )
            .is_err());
    }
    assert!(
        controller
            .latest_message(profile::MESSAGE_TYPE_IO_SETTINGS)
            .is_some(),
        "fixture deliberately retains stale ancillary cache evidence"
    );
    let mut decoded = false;
    let result = crate::rpc::gateway_io_settings_with(&controller, |_| {
        decoded = true;
        Ok(serde_json::Value::Null)
    });
    assert!(result.unwrap_err().contains("software observation budget"));
    assert!(
        !decoded,
        "unavailable retained IO must be refused before decoding cached bytes"
    );
}
