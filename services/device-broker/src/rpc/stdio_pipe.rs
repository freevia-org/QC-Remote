//! Windows anonymous STDIO pipes are synchronous. Only dedicated owned IO
//! threads use these duplicated handles; cancellation targets those ORIGINAL
//! thread objects, never a PID/TID lookup or a reusable thread-pool worker.
use super::*;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use windows_sys::Win32::Foundation::{
    DuplicateHandle, DUPLICATE_SAME_ACCESS, ERROR_NOT_FOUND, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, WaitForSingleObject};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

const POLL: Duration = Duration::from_millis(10);
const DELIVERY: Duration = Duration::from_secs(5);
const RETIREMENT: Duration = Duration::from_secs(3);
static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static RETAINED: Mutex<Vec<RetainedRun>> = Mutex::new(Vec::new());
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        RUNS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct RetainedRun {
    jobs: Vec<thread::JoinHandle<()>>,
    _permit: Arc<Permit>,
}
fn signalled(job: &thread::JoinHandle<()>) -> Result<bool, String> {
    match unsafe { WaitForSingleObject(job.as_raw_handle() as _, 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        other => Err(format!(
            "STDIO original thread wait failed ({other}): {}",
            io::Error::last_os_error()
        )),
    }
}
#[allow(
    deprecated,
    reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
)]
fn acquire() -> Result<Arc<Permit>, String> {
    let finished = {
        let mut retained = RETAINED.lock_recover();
        let mut finished = Vec::new();
        let mut index = 0;
        while index < retained.len() {
            if retained[index]
                .jobs
                .iter()
                .all(|job| signalled(job).unwrap_or(false))
            {
                finished.push(retained.swap_remove(index));
            } else {
                index += 1;
            }
        }
        finished
    };
    for run in finished {
        for job in run.jobs {
            let _ = job.join();
        }
        drop(run._permit);
    }
    RUNS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        (count < 4).then_some(count + 1)
    })
    .map_err(|_| {
        "STDIO run capacity exhausted; unfinished original owners remain retained".to_string()
    })?;
    Ok(Arc::new(Permit))
}
struct OwnedRun {
    stop: Arc<AtomicBool>,
    jobs: Vec<thread::JoinHandle<()>>,
    essential: Vec<usize>,
    permit: Arc<Permit>,
}
impl OwnedRun {
    fn new() -> Result<Self, String> {
        Ok(Self {
            stop: Arc::new(AtomicBool::new(false)),
            jobs: Vec::new(),
            essential: Vec::new(),
            permit: acquire()?,
        })
    }
    fn spawn_essential(
        &mut self,
        name: &str,
        run: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> Result<(), String> {
        self.spawn(name, run)?;
        self.essential.push(self.jobs.len() - 1);
        Ok(())
    }
    fn spawn(
        &mut self,
        name: &str,
        run: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> Result<(), String> {
        self.spawn_with(name, run, |builder, task| builder.spawn(task))
    }
    fn spawn_with<F>(
        &mut self,
        name: &str,
        run: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
        spawn: F,
    ) -> Result<(), String>
    where
        F: FnOnce(thread::Builder, Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>>,
    {
        let stop = Arc::clone(&self.stop);
        // Preserve the original JoinHandle before any caller waits/dispatches.
        let job = spawn(
            thread::Builder::new().name(name.into()),
            Box::new(move || run(stop)),
        )
        .map_err(|error| format!("Could not start owned STDIO worker: {error}"))?;
        self.jobs.push(job);
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        self.finish_within(RETIREMENT)
    }
    fn finish_within(&mut self, budget: Duration) -> Result<(), String> {
        self.finish_within_using(budget, |job| {
            if unsafe { CancelSynchronousIo(job.as_raw_handle() as _) } != 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Ok(())
            } else {
                Err(error.to_string())
            }
        })
    }
    fn finish_within_using(
        &mut self,
        budget: Duration,
        mut cancel: impl FnMut(&thread::JoinHandle<()>) -> Result<(), String>,
    ) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        let deadline = Instant::now() + budget;
        let mut problems = Vec::new();
        loop {
            let mut index = 0;
            while index < self.jobs.len() {
                match signalled(&self.jobs[index]) {
                    Ok(true) => {
                        if self.jobs.swap_remove(index).join().is_err() {
                            problems.push("STDIO worker panicked".to_string());
                        }
                        continue;
                    }
                    Err(error) => {
                        if !problems.contains(&error) {
                            problems.push(error);
                        }
                    }
                    Ok(false) => {}
                }
                // Cancel is an ATTEMPT, not acknowledgment. Repeat to cover
                // an IO start racing a previous ERROR_NOT_FOUND cancellation.
                if let Err(error) = cancel(&self.jobs[index]) {
                    let message = format!("STDIO original thread cancellation failed: {error}");
                    if !problems.contains(&message) {
                        problems.push(message);
                    }
                }
                index += 1;
            }
            if self.jobs.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                RETAINED.lock_recover().push(RetainedRun {
                    jobs: std::mem::take(&mut self.jobs),
                    _permit: Arc::clone(&self.permit),
                });
                problems.push("STDIO retirement unacknowledged; exact original threads/handles/resources and permit retained".into());
                break;
            }
            thread::sleep(POLL);
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }
}
// Explicit serve completion returns cleanup diagnostics. Drop must not write
// to potentially congested STDERR during unwind; unacknowledged owners remain
// in the bounded registry even when the caller cannot receive an error.
impl Drop for OwnedRun {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
fn duplicate(handle: &impl AsRawHandle) -> Result<File, String> {
    let mut owned = std::ptr::null_mut();
    let process = unsafe { GetCurrentProcess() };
    if unsafe {
        DuplicateHandle(
            process,
            handle.as_raw_handle() as _,
            process,
            &mut owned,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(format!(
            "Could not duplicate owned STDIO pipe: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(unsafe { File::from_raw_handle(owned as _) })
}
struct StopFile {
    file: File,
    stop: Arc<AtomicBool>,
}
impl Read for StopFile {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::Acquire) {
            return Err(io::Error::other("STDIO stopped"));
        }
        self.file.read(bytes)
    }
}
impl Write for StopFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::Acquire) {
            return Err(io::Error::other("STDIO stopped"));
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Frame {
    body: Vec<u8>,
    ack: mpsc::SyncSender<Result<(), String>>,
}
struct Limited(Vec<u8>);
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > domain::IPC_MAX_FRAME_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other(
                "Native broker response exceeds the IPC frame limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn enqueue<T>(
    sender: &mpsc::SyncSender<T>,
    mut item: T,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), String> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("STDIO session has stopped".into());
        }
        match sender.try_send(item) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("STDIO queue owner ended".into())
            }
            Err(mpsc::TrySendError::Full(returned)) => item = returned,
        }
        if Instant::now() >= deadline {
            return Err("STDIO queue admission deadline expired".into());
        }
        thread::sleep(POLL);
    }
}
fn deliver(
    sender: &mpsc::SyncSender<Frame>,
    value: &Value,
    stop: &AtomicBool,
) -> Result<(), String> {
    let deadline = Instant::now() + DELIVERY;
    let mut body = Limited(Vec::new());
    serde_json::to_writer(&mut body, value).map_err(|error| error.to_string())?;
    let (ack, answer) = mpsc::sync_channel(1);
    enqueue(sender, Frame { body: body.0, ack }, stop, deadline)?;
    answer
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| {
            "STDIO output completion deadline expired; transport must retire without replay"
                .to_string()
        })?
}
pub(super) fn serve(
    controller: Arc<DeviceController>,
    phases: bool,
    clock: Arc<crate::orphan_guard::RequestClock>,
) -> Result<(), String> {
    // Reserve and own the run before duplicating handles or starting workers.
    let mut owned = OwnedRun::new()?;
    let input = duplicate(&io::stdin())?;
    let output = duplicate(&io::stdout())?;
    let shutdown = controller.shutdown_handle();
    let watched_clock = Arc::clone(&clock);
    owned.spawn("qc-owned-stdio-parent-watch", move |stop| {
        crate::orphan_guard::watch_until(
            crate::orphan_guard::parent_process_id(),
            watched_clock,
            move || {
                let _ = shutdown.stop_and_join(crate::worker::WORKER_EXIT_BUDGET);
            },
            &stop,
        );
    })?;
    let result = serve_files(&mut owned, controller, phases, clock, input, output);
    let retired = owned.finish();
    match (result, retired) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
    }
}
fn serve_files(
    owned: &mut OwnedRun,
    controller: Arc<DeviceController>,
    phases: bool,
    clock: Arc<crate::orphan_guard::RequestClock>,
    input: File,
    output: File,
) -> Result<(), String> {
    let (requests, incoming) = mpsc::sync_channel(4);
    let (frames, outgoing) = mpsc::sync_channel::<Frame>(4);
    owned.spawn("qc-owned-stdio-input", move |stop| {
        let mut input = StopFile {
            file: input,
            stop: Arc::clone(&stop),
        };
        loop {
            let next = read_request(&mut input);
            let ended = !matches!(&next, Ok(Some(_)));
            if enqueue(&requests, next, &stop, Instant::now() + DELIVERY).is_err() {
                stop.store(true, Ordering::Release);
                return;
            }
            if ended {
                return;
            }
        }
    })?;
    owned.spawn_essential("qc-owned-stdio-output", move |stop| {
        let mut output = StopFile {
            file: output,
            stop: Arc::clone(&stop),
        };
        while !stop.load(Ordering::Acquire) {
            let frame = match outgoing.recv_timeout(POLL) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return,
            };
            let result = qc_broker_framing::write_frame(
                &mut output,
                &frame.body,
                domain::IPC_MAX_FRAME_BYTES,
            )
            .map_err(|error| error.to_string());
            let failed = result.is_err();
            let _ = frame.ack.try_send(result);
            if failed {
                stop.store(true, Ordering::Release);
                return;
            }
        }
    })?;
    let events = controller.subscribe_state_events();
    let observed_controller = Arc::clone(&controller);
    let notifications = frames.clone();
    owned.spawn_essential("qc-owned-stdio-events", move |stop| {
        let mut published = None;
        while !stop.load(Ordering::Acquire) {
            match events.recv_timeout(PHASE_EVENT_POLL) {
                Ok(frame) => {
                    let notification =
                        json!({"jsonrpc":"2.0","method":"device.stateFrame","params":frame});
                    if deliver(&notifications, &notification, &stop).is_err() {
                        stop.store(true, Ordering::Release);
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
            if phases {
                let status = observed_controller.status();
                let next = (
                    status.phase.clone(),
                    status.detail.clone(),
                    status.connected,
                    status.synchronized,
                );
                if published.as_ref() != Some(&next) {
                    if deliver(&notifications, &phase_notification(&status), &stop).is_err() {
                        stop.store(true, Ordering::Release);
                        return;
                    }
                    published = Some(next);
                }
            }
        }
    })?;
    let midi = PerformanceMidiWorkers::default();
    loop {
        if owned.stop.load(Ordering::Acquire) {
            return Err("STDIO pipe owner stopped; transport outcome must not be replayed".into());
        }
        for index in &owned.essential {
            if signalled(&owned.jobs[*index])? {
                return Err(
                    "STDIO output/event owner ended unexpectedly; transport must retire".into(),
                );
            }
        }
        let request = match incoming.recv_timeout(POLL) {
            Ok(request) => request?,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return Err("STDIO input owner ended unexpectedly".into()),
        };
        let Some(request) = request else {
            return Ok(());
        };
        // Recheck at the dispatch boundary; reader admission is not authority
        // to execute a queued mutation after output/session cancellation.
        if owned.stop.load(Ordering::Acquire) {
            return Err("STDIO stopped before request dispatch".into());
        }
        if !clock.start_request() {
            return Err("STDIO orphan retirement committed before request dispatch".into());
        }
        let response = serve_stdio_request(&controller, &midi, request);
        clock.finish_request();
        deliver(&frames, &response, &owned.stop)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::GetThreadIOPendingFlag;
    static PIPE_FIXTURE_LOCK: Mutex<()> = Mutex::new(());
    fn pipe() -> (File, File) {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        assert_ne!(
            unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 4096) },
            0
        );
        (unsafe { File::from_raw_handle(read as _) }, unsafe {
            File::from_raw_handle(write as _)
        })
    }
    fn wait_pending(job: &thread::JoinHandle<()>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut pending = 0;
            assert_ne!(
                unsafe { GetThreadIOPendingFlag(job.as_raw_handle() as _, &mut pending) },
                0
            );
            if pending != 0 {
                return;
            }
            assert!(
                Instant::now() < deadline && !signalled(job).unwrap(),
                "fixture never entered actual blocked synchronous pipe IO"
            );
            thread::sleep(POLL);
        }
    }
    #[test]
    fn silent_owned_input_pipe_is_cancelled_and_original_thread_joined_while_writer_lives() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (input, _live_writer) = pipe();
        let mut owned = OwnedRun::new().unwrap();
        owned
            .spawn("fixture-stdio-read", move |stop| {
                let mut input = StopFile { file: input, stop };
                let _ = input.read(&mut [0; 1]);
            })
            .unwrap();
        wait_pending(&owned.jobs[0]);
        owned
            .finish()
            .expect("actual native signal and original join, not a stopflag");
        assert!(owned.jobs.is_empty());
    }
    #[test]
    fn the_original_parent_watchdog_retires_with_the_same_stdio_run() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let mut owned = OwnedRun::new().unwrap();
        let permit = Arc::downgrade(&owned.permit);
        let (entered, ready) = mpsc::channel();
        owned
            .spawn("fixture-owned-parent-watch", move |stop| {
                entered.send(()).unwrap();
                crate::orphan_guard::watch_until(
                    Some(std::process::id()),
                    Arc::new(crate::orphan_guard::RequestClock::started_now()),
                    || panic!("live fixture parent must not be reaped"),
                    &stop,
                );
            })
            .unwrap();
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        owned.finish().unwrap();
        assert!(
            owned.jobs.is_empty(),
            "actual original watchdog native signal and join, not detached polling"
        );
        drop(owned);
        assert!(permit.upgrade().is_none());
    }
    #[test]
    fn full_owned_output_pipe_is_cancelled_and_joined_while_reader_is_nonreading() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (_nonreading_peer, output) = pipe();
        let mut owned = OwnedRun::new().unwrap();
        owned
            .spawn("fixture-stdio-write", move |stop| {
                let mut output = StopFile { file: output, stop };
                let _ = output.write_all(&[0; 64 * 1024]);
            })
            .unwrap();
        wait_pending(&owned.jobs[0]);
        owned
            .finish()
            .expect("actual full-pipe WriteFile retirement");
        assert!(owned.jobs.is_empty());
    }
    #[test]
    fn partial_stdio_startup_unwind_retires_the_first_actual_pipe_owner() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (input, _live_writer) = pipe();
        let mut permit = None;
        let finished = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&finished);
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut owned = OwnedRun::new().unwrap();
            permit = Some(Arc::downgrade(&owned.permit));
            owned
                .spawn("fixture-stdio-startup", move |stop| {
                    let mut input = StopFile { file: input, stop };
                    let _ = input.read(&mut [0; 1]);
                    done.store(true, Ordering::Release);
                })
                .unwrap();
            wait_pending(&owned.jobs[0]);
            let error = owned
                .spawn_with(
                    "fixture-rejected-second-worker",
                    |_| panic!("must never start"),
                    |_builder, _task| Err(io::Error::other("injected native thread spawn refusal")),
                )
                .unwrap_err();
            assert!(error.contains("spawn refusal"));
            panic!("caller unwinds after actual owned startup seam returned failure");
        }));
        assert!(failure.is_err());
        assert!(finished.load(Ordering::Acquire));
        assert!(
            permit.unwrap().upgrade().is_none(),
            "this exact original run was joined/released, independent of concurrent fixtures"
        );
    }
    #[test]
    fn a_cancel_before_io_start_retains_unknown_owner_then_reaps_after_actual_exit() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (input, _live_writer) = pipe();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        struct Release(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0 .0.lock_recover() = true;
                self.0 .1.notify_all();
            }
        }
        let mut owned = OwnedRun::new().unwrap();
        let permit = Arc::downgrade(&owned.permit);
        let worker_gate = Arc::clone(&gate);
        let (entered, ready) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&finished);
        owned
            .spawn("fixture-stdio-cancel-race", move |stop| {
                entered.send(()).unwrap();
                let mut released = worker_gate.0.lock_recover();
                while !*released {
                    released = worker_gate
                        .1
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                drop(released);
                let mut input = StopFile { file: input, stop };
                assert!(
                    input.read(&mut [0; 1]).is_err(),
                    "stop check forbids starting IO after cancellation"
                );
                done.store(true, Ordering::Release);
            })
            .unwrap();
        let release = Release(gate);
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(owned
            .finish_within(Duration::from_millis(20))
            .unwrap_err()
            .contains("unacknowledged"));
        drop(owned);
        assert!(
            permit.upgrade().is_some(),
            "exact retained original still consumes admission"
        );
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !finished.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        }
        loop {
            let next = acquire().unwrap();
            drop(next);
            if permit.upgrade().is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "actual original thread signal/join must release retained permit"
            );
            thread::sleep(POLL);
        }
    }
    #[test]
    fn owned_stdio_runtime_returns_a_real_framed_status_and_retires_on_input_eof() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (input, mut requests) = pipe();
        let (responses, output) = pipe();
        let controller = Arc::new(DeviceController::start_disconnected().unwrap());
        let clock = Arc::new(crate::orphan_guard::RequestClock::started_now());
        let stop = Arc::new(AtomicBool::new(false));
        struct Stop(Arc<AtomicBool>);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let (completed, completion) = mpsc::channel();
        let (answered, answer) = mpsc::channel();
        let mut fixture = OwnedRun::new().unwrap();
        // This guard is established before spawn/waits and drops before the
        // outer jobs. It also wakes the server's receive loop on assertion
        // unwind instead of hoping stdin EOF or native cancellation does so.
        let _stop_on_unwind = Stop(Arc::clone(&stop));
        fixture
            .spawn("fixture-real-stdio-runtime", move |_| {
                let mut owned = OwnedRun::new().unwrap();
                owned.stop = stop;
                owned
                    .spawn("fixture-unavailable-parent-watch", |stop| {
                        crate::orphan_guard::watch_until(
                            None,
                            Arc::new(crate::orphan_guard::RequestClock::started_now()),
                            || panic!("unknown parent is conservative"),
                            &stop,
                        );
                    })
                    .unwrap();
                let result = serve_files(&mut owned, controller, false, clock, input, output);
                let cleanup = owned.finish();
                let _ = completed.send((result, cleanup));
            })
            .unwrap();
        fixture
            .spawn("fixture-real-stdio-response", move |stop| {
                let mut input = StopFile {
                    file: responses,
                    stop,
                };
                let response = read_frame_bytes(&mut input).and_then(|body| {
                    serde_json::from_slice::<Value>(&body.ok_or("missing status frame")?)
                        .map_err(|error| error.to_string())
                });
                let _ = answered.send(response);
            })
            .unwrap();
        let request = json!({"jsonrpc":"2.0","id":7,"method":"system.status","params":{}});
        write_response(&mut requests, &request).unwrap();
        let response = answer
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(
            response["result"]["gatewayApiVersion"],
            generated_gateway::API_VERSION
        );
        assert_eq!(response["result"]["usbDiagnostics"]["connected"], false);
        drop(requests);
        let (served, retired) = completion.recv_timeout(Duration::from_secs(2)).unwrap();
        served.unwrap();
        retired.unwrap();
        fixture.finish().unwrap();
        assert!(fixture.jobs.is_empty());
    }
    #[test]
    fn failed_native_cancellation_retains_exact_pending_io_until_peer_eof_and_reap() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (input, writer) = pipe();
        let mut owned = OwnedRun::new().unwrap();
        let permit = Arc::downgrade(&owned.permit);
        owned
            .spawn("fixture-cancel-error", move |_stop| {
                // Unlike the separate cancel-before-start fixture, this fixture
                // deliberately requires a read which CANNOT short-circuit on stop.
                // A transient pending-IO observation during thread startup must
                // not turn an injected cancellation failure into early success.
                let mut input = input;
                let _ = input.read(&mut [0; 1]);
            })
            .unwrap();
        // Peer ownership is established before assertions and retires before
        // the run on unwind. No unrelated handle/process is closed.
        struct Peer(Option<File>);
        let peer = Peer(Some(writer));
        wait_pending(&owned.jobs[0]);
        let error = owned
            .finish_within_using(Duration::from_millis(20), |_| {
                Err("injected access denial".into())
            })
            .unwrap_err();
        assert!(
            error.contains("access denial") && error.contains("unacknowledged"),
            "unexpected retirement diagnostic: {error}"
        );
        assert!(permit.upgrade().is_some());
        drop(owned);
        drop(peer.0);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            drop(acquire().unwrap());
            if permit.upgrade().is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "original pending read must signal/join after its own peer closes"
            );
            thread::sleep(POLL);
        }
    }
    #[test]
    fn a_full_producer_queue_wakes_on_stop_without_dispatching_its_pending_item() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (sender, receiver) = mpsc::sync_channel(4);
        for value in 0..4 {
            sender.try_send(value).unwrap();
        }
        let mut owned = OwnedRun::new().unwrap();
        let (completed, completion) = mpsc::channel();
        owned
            .spawn("fixture-full-stdio-queue", move |stop| {
                let result = enqueue(&sender, 99, &stop, Instant::now() + DELIVERY);
                let _ = completed.send(result);
            })
            .unwrap();
        owned.finish().unwrap();
        assert!(completion
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err()
            .contains("stopped"));
        assert_eq!(receiver.try_iter().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert!(owned.jobs.is_empty());
    }
    #[test]
    fn owned_stdio_phase_events_share_framing_with_replies_and_retire_on_eof() {
        let _fixture = PIPE_FIXTURE_LOCK.lock_recover();
        let (input, mut requests) = pipe();
        let (responses, output) = pipe();
        let controller = Arc::new(DeviceController::start_disconnected().unwrap());
        let expected = controller.status();
        let clock = Arc::new(crate::orphan_guard::RequestClock::started_now());
        let stop = Arc::new(AtomicBool::new(false));
        struct Stop(Arc<AtomicBool>);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let (completed, completion) = mpsc::channel();
        let (answered, answer) = mpsc::channel();
        let mut fixture = OwnedRun::new().unwrap();
        let _stop_on_unwind = Stop(Arc::clone(&stop));
        fixture
            .spawn("fixture-owned-stdio-phase-runtime", move |_| {
                let mut owned = OwnedRun::new().unwrap();
                owned.stop = stop;
                let result = serve_files(&mut owned, controller, true, clock, input, output);
                let cleanup = owned.finish();
                let _ = completed.send((result, cleanup));
            })
            .unwrap();
        fixture
            .spawn("fixture-owned-stdio-phase-reader", move |stop| {
                let mut input = StopFile {
                    file: responses,
                    stop,
                };
                let result = (|| -> Result<(Value, Value), String> {
                    let mut phase = None;
                    let mut reply = None;
                    // Main waits one whole bounded observation below; this worker
                    // is still an original cancellable pipe owner on failure.
                    // Frame count also bounds unexpected well-formed notifications.
                    for _ in 0..8 {
                        let body = read_frame_bytes(&mut input)?.ok_or("missing phase/reply")?;
                        let value: Value =
                            serde_json::from_slice(&body).map_err(|error| error.to_string())?;
                        if value["method"] == "device.phase" {
                            phase = Some(value);
                        } else if value["id"] == 9 {
                            reply = Some(value);
                        }
                        if let (Some(phase), Some(reply)) = (phase.as_ref(), reply.as_ref()) {
                            return Ok((phase.clone(), reply.clone()));
                        }
                    }
                    Err("phase/reply not found within bounded frame count".into())
                })();
                let _ = answered.send(result);
            })
            .unwrap();
        write_response(
            &mut requests,
            &json!({"jsonrpc":"2.0","id":9,"method":"system.status","params":{}}),
        )
        .unwrap();
        let (phase, reply) = answer
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(phase["jsonrpc"], "2.0");
        assert_eq!(phase["params"]["phase"], expected.phase);
        assert_eq!(phase["params"]["detail"], expected.detail);
        assert_eq!(phase["params"]["connected"], false);
        assert_eq!(phase["params"]["synchronized"], false);
        assert_eq!(reply["id"], 9);
        assert_eq!(
            reply["result"]["gatewayApiVersion"],
            generated_gateway::API_VERSION
        );
        assert_eq!(reply["result"]["usbDiagnostics"]["connected"], false);
        drop(requests);
        let (served, retired) = completion.recv_timeout(Duration::from_secs(2)).unwrap();
        served.unwrap();
        retired.unwrap();
        fixture.finish().unwrap();
        assert!(fixture.jobs.is_empty());
    }
}
