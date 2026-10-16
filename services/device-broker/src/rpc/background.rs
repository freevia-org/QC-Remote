//! Software headroom, not device/firmware limits. Every admitted dispatcher
//! retains its original operations, including stalled operations after stop.
use super::*;
use std::sync::{OnceLock, Weak};

const MAX_MANAGERS: usize = 32;
const MAX_KEYS: usize = 64;
const MAX_JOBS: usize = 4;
const POLL: Duration = Duration::from_millis(10);
pub(super) type Key = [u64; 3];
static MANAGERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        MANAGERS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Original {
    handle: thread::JoinHandle<()>,
    _permit: Arc<Permit>,
}
fn retained() -> &'static Mutex<Vec<Original>> {
    static OWNERS: OnceLock<Mutex<Vec<Original>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(Vec::new()))
}
fn signalled(handle: &thread::JoinHandle<()>) -> bool {
    crate::platform::process::thread_signalled(handle)
}
fn reap_retained() {
    reap_batches();
    let complete = {
        let mut owners = retained().lock_recover();
        let mut complete = Vec::new();
        let mut index = 0;
        while index < owners.len() {
            if signalled(&owners[index].handle) {
                complete.push(owners.swap_remove(index));
            } else {
                index += 1;
            }
        }
        complete
    };
    for original in complete {
        let _ = original.handle.join();
        drop(original._permit);
    }
}
#[allow(
    deprecated,
    reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
)]
fn permit() -> Result<Arc<Permit>, String> {
    MANAGERS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| (count < MAX_MANAGERS).then_some(count + 1))
        .map_err(|_| "Broker background manager capacity exhausted; original unfinished owners remain retained".to_string())?;
    Ok(Arc::new(Permit))
}
pub(super) struct Work {
    run: Box<dyn FnOnce(Ticket) + Send>,
    // This completion/guard is retained BESIDE the original operation handle.
    // It runs only after join, or when a queued operation never starts.
    retired: Option<Box<dyn FnOnce(bool) + Send>>,
}
impl Work {
    pub(super) fn new(run: impl FnOnce(Ticket) + Send + 'static) -> Self {
        Self {
            run: Box::new(run),
            retired: None,
        }
    }
    pub(super) fn on_retired(mut self, retired: impl FnOnce(bool) + Send + 'static) -> Self {
        self.retired = Some(Box::new(retired));
        self
    }
    fn cancel(mut self) {
        retire_callback(self.retired.take(), false);
    }
}
struct Entry {
    generation: u64,
    deadline: Instant,
    queued: Option<Work>,
    running: bool,
    requesters: Vec<u64>,
    requesters_closed: bool,
}
enum Scheduling {
    Replace,
    Latest,
    Requested(u64),
    Current(Ticket),
}
#[derive(Default)]
struct State {
    stopped: bool,
    started: bool,
    generation: u64,
    entries: BTreeMap<Key, Entry>,
    #[cfg(test)]
    panic_before_jobs: bool,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Clone)]
pub(super) struct Ticket {
    shared: Weak<Shared>,
    key: Key,
    generation: u64,
}
impl Ticket {
    /// Rearm an active original's key without starting another dispatcher or
    /// capturing its Manager in queued work. Superseded/stopped work cannot
    /// overwrite a newer generation; the existing owner retains the timer.
    #[cfg(target_os = "linux")]
    pub(super) fn retry_after(&self, deadline: Instant, work: Work) -> Result<bool, String> {
        let mut pending = Some(work);
        let mut replaced = None;
        let shared = self.shared.upgrade();
        let result = (|| {
            let Some(shared) = &shared else {
                return Ok(false);
            };
            let mut state = shared.state.lock_recover();
            if state.stopped
                || !state
                    .entries
                    .get(&self.key)
                    .is_some_and(|entry| entry.running && entry.generation == self.generation)
            {
                return Ok(false);
            }
            let generation = state
                .generation
                .checked_add(1)
                .ok_or_else(|| "Broker background generation exhausted".to_string())?;
            state.generation = generation;
            let entry = state
                .entries
                .get_mut(&self.key)
                .expect("current active key");
            entry.generation = generation;
            entry.deadline = deadline;
            entry.requesters_closed = false;
            replaced = entry
                .queued
                .replace(pending.take().expect("retry work is owned"));
            Ok(true)
        })();
        if let Some(shared) = shared {
            shared.changed.notify_all();
        }
        if let Some(work) = pending {
            work.cancel();
        }
        if let Some(work) = replaced {
            work.cancel();
        }
        result
    }

    pub(super) fn current(&self) -> bool {
        self.with_current(|| ()).is_some()
    }
    /// Acquire request_lock BEFORE this call. Only short accounting admission
    /// runs inside; never native waits, queue producers or blocking teardown.
    pub(super) fn with_current<T>(&self, admit: impl FnOnce() -> T) -> Option<T> {
        self.with_requesters(|_| admit())
    }
    pub(super) fn with_requesters<T>(&self, admit: impl FnOnce(&[u64]) -> T) -> Option<T> {
        let shared = self.shared.upgrade()?;
        let state = shared.state.lock_recover();
        if state.stopped
            || !state
                .entries
                .get(&self.key)
                .is_some_and(|entry| entry.generation == self.generation)
        {
            return None;
        }
        Some(admit(&state.entries.get(&self.key)?.requesters))
    }
    /// Final requester admission is sealed atomically with its predicate.
    /// A later request must queue a fresh operation, not join an evaluation
    /// which already refused or dispatched before that request existed.
    pub(super) fn admit_requesters<T>(&self, admit: impl FnOnce(&[u64]) -> T) -> Option<T> {
        let shared = self.shared.upgrade()?;
        let mut state = shared.state.lock_recover();
        if state.stopped {
            return None;
        }
        let entry = state.entries.get_mut(&self.key)?;
        if entry.generation != self.generation {
            return None;
        }
        entry.requesters_closed = true;
        Some(admit(&entry.requesters))
    }
}
pub(super) struct Manager {
    shared: Arc<Shared>,
    original: Mutex<Option<Original>>,
    retirement: Mutex<Weak<Permit>>,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
            original: Mutex::new(None),
            retirement: Mutex::new(Weak::new()),
        }
    }
}
impl Manager {
    pub(super) fn schedule(&self, key: Key, deadline: Instant, work: Work) -> Result<(), String> {
        self.schedule_with(key, deadline, work, |builder, body| builder.spawn(body))
    }
    pub(super) fn requested(&self, key: Key, requester: u64, work: Work) -> Result<(), String> {
        self.enqueue_with(
            key,
            Instant::now(),
            work,
            Scheduling::Requested(requester),
            |builder, body| builder.spawn(body),
        )
    }
    pub(super) fn latest(&self, key: Key, deadline: Instant, work: Work) -> Result<(), String> {
        self.enqueue_with(key, deadline, work, Scheduling::Latest, |builder, body| {
            builder.spawn(body)
        })
    }
    pub(super) fn reschedule(
        &self,
        ticket: &Ticket,
        deadline: Instant,
        work: Work,
    ) -> Result<(), String> {
        self.enqueue_with(
            ticket.key,
            deadline,
            work,
            Scheduling::Current(ticket.clone()),
            |builder, body| builder.spawn(body),
        )
    }
    fn schedule_with<F>(
        &self,
        key: Key,
        deadline: Instant,
        work: Work,
        spawn: F,
    ) -> Result<(), String>
    where
        F: FnOnce(thread::Builder, Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>>,
    {
        self.enqueue_with(key, deadline, work, Scheduling::Replace, spawn)
    }
    fn enqueue_with<F>(
        &self,
        key: Key,
        deadline: Instant,
        work: Work,
        mode: Scheduling,
        spawn: F,
    ) -> Result<(), String>
    where
        F: FnOnce(thread::Builder, Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>>,
    {
        // Reapers can invoke retirement callbacks; do so before ANY owner lock.
        reap_retained();
        // Serializes first-spawn/publication, not worker dispatch/native IO.
        let mut original = self.original.lock_recover();
        let mut work = Some(work);
        if original
            .as_ref()
            .is_some_and(|owner| signalled(&owner.handle))
        {
            drop(original);
            self.cancel_pending();
            work.take().unwrap().cancel();
            return Err("Broker background dispatcher ended; new work was not admitted".into());
        }
        let accepted = (|| -> Result<(bool, Option<Work>), String> {
            let mut state = self.shared.state.lock_recover();
            if state.stopped {
                return Err("Broker background manager is stopped".into());
            }
            if let Scheduling::Current(ticket) = &mode {
                if !Weak::ptr_eq(&ticket.shared, &Arc::downgrade(&self.shared))
                    || !state
                        .entries
                        .get(&key)
                        .is_some_and(|entry| entry.generation == ticket.generation)
                {
                    return Ok((false, None));
                }
            }
            if let Scheduling::Requested(requester) = &mode {
                if let Some(entry) = state.entries.get_mut(&key) {
                    if !entry.requesters_closed {
                        if !entry.requesters.contains(requester) {
                            if entry.requesters.len() >= BROKER_MAX_CONNECTION_OWNERS {
                                return Err("Broker reconnect requester capacity exhausted".into());
                            }
                            entry.requesters.push(*requester);
                        }
                        return Ok((false, None));
                    }
                }
            }
            if !state.entries.contains_key(&key) && state.entries.len() >= MAX_KEYS {
                return Err("Broker background logical-key capacity exhausted".into());
            }
            let Some(generation) = state.generation.checked_add(1) else {
                return Err("Broker background generation exhausted".into());
            };
            state.generation = generation;
            if let Some(entry) = state.entries.get_mut(&key) {
                entry.generation = generation;
                entry.deadline = if matches!(mode, Scheduling::Latest) {
                    entry.deadline.max(deadline)
                } else {
                    deadline
                };
                if let Scheduling::Requested(requester) = &mode {
                    entry.requesters = vec![*requester];
                }
                entry.requesters_closed = false;
                Ok((true, entry.queued.replace(work.take().unwrap())))
            } else {
                let requesters = if let Scheduling::Requested(requester) = mode {
                    vec![requester]
                } else {
                    Vec::new()
                };
                state.entries.insert(
                    key,
                    Entry {
                        generation,
                        deadline,
                        queued: work.take(),
                        running: false,
                        requesters,
                        requesters_closed: false,
                    },
                );
                Ok((true, None))
            }
        })();
        let replaced = match accepted {
            Ok((true, replaced)) => replaced,
            Ok((false, _)) => {
                drop(original);
                work.take().unwrap().cancel();
                return Ok(());
            }
            Err(error) => {
                drop(original);
                work.take().unwrap().cancel();
                return Err(error);
            }
        };
        if original.is_none() {
            let admitted = match permit() {
                Ok(admitted) => admitted,
                Err(error) => {
                    drop(original);
                    self.cancel_pending();
                    if let Some(work) = replaced {
                        work.cancel();
                    }
                    return Err(error);
                }
            };
            let shared = Arc::clone(&self.shared);
            let held_permit = Arc::clone(&admitted);
            let body: Box<dyn FnOnce() + Send> = Box::new(move || dispatch(shared, held_permit));
            match spawn(
                thread::Builder::new().name("qc-owned-background-dispatch".into()),
                body,
            ) {
                Ok(handle) => {
                    *self.retirement.lock_recover() = Arc::downgrade(&admitted);
                    *original = Some(Original {
                        handle,
                        _permit: admitted,
                    });
                    // Original owner is stored before the first possible dispatch.
                    self.shared.state.lock_recover().started = true;
                }
                Err(error) => {
                    drop(original);
                    self.cancel_pending();
                    if let Some(work) = replaced {
                        work.cancel();
                    }
                    return Err(format!(
                        "Could not start broker background dispatcher: {error}"
                    ));
                }
            }
        }
        drop(original);
        self.shared.changed.notify_all();
        // No queue OR original-owner mutex is held across guard/callback drop.
        if let Some(work) = replaced {
            work.cancel();
        }
        Ok(())
    }
    fn cancel_pending(&self) {
        let cancelled = {
            let mut state = self.shared.state.lock_recover();
            state.stopped = true;
            let cancelled = state
                .entries
                .values_mut()
                .filter_map(|entry| entry.queued.take())
                .collect::<Vec<_>>();
            state.entries.retain(|_, entry| entry.running);
            cancelled
        };
        self.shared.changed.notify_all();
        for work in cancelled {
            work.cancel();
        }
    }
    pub(super) fn stop(&self) {
        self.cancel_pending();
    }
    #[cfg(test)]
    pub(super) fn pending_keys(&self) -> usize {
        self.shared.state.lock_recover().entries.len()
    }
    #[cfg(test)]
    pub(super) fn pending_key(&self, key: Key) -> bool {
        self.shared.state.lock_recover().entries.contains_key(&key)
    }
    /// Rebase a not-yet-admitted fixture timer when its software clock is
    /// explicitly overridden. Production Latest remains monotonic; a running
    /// callback is never rewound or misrepresented as cancelled.
    #[cfg(test)]
    pub(super) fn rebase_pending_deadline_for_test(&self, key: Key, deadline: Instant) -> bool {
        let mut state = self.shared.state.lock_recover();
        let Some(entry) = state.entries.get_mut(&key) else {
            return false;
        };
        if entry.running || entry.queued.is_none() {
            return false;
        }
        entry.deadline = deadline;
        true
    }
    #[cfg(test)]
    pub(super) fn cancel_key(&self, key: Key) -> Result<(), String> {
        let cancelled = {
            let mut state = self.shared.state.lock_recover();
            if !state.entries.contains_key(&key) {
                return Ok(());
            }
            let Some(generation) = state.generation.checked_add(1) else {
                drop(state);
                self.stop();
                return Err("Broker background generation exhausted".into());
            };
            state.generation = generation;
            let entry = state.entries.get_mut(&key).unwrap();
            entry.generation = generation;
            let cancelled = entry.queued.take();
            if !entry.running {
                state.entries.remove(&key);
            }
            cancelled
        };
        self.shared.changed.notify_all();
        if let Some(work) = cancelled {
            work.cancel();
        }
        Ok(())
    }
    pub(super) fn acknowledged(&self) -> bool {
        reap_batches();
        let mut original = self.original.lock_recover();
        if original
            .as_ref()
            .is_some_and(|owner| signalled(&owner.handle))
        {
            let owner = original.take().unwrap();
            drop(original);
            let _ = owner.handle.join();
            drop(owner._permit);
            return self.retirement.lock_recover().upgrade().is_none();
        }
        original.is_none() && self.retirement.lock_recover().upgrade().is_none()
    }
    #[cfg(test)]
    pub(super) fn finish_before(&self, deadline: Instant) -> Result<(), String> {
        self.stop();
        loop {
            if self.acknowledged() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("Broker background retirement unacknowledged; exact dispatcher/jobs/guards remain owned".into());
            }
            thread::sleep(POLL);
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.stop();
        let original = self
            .original
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(original) = original {
            let same_thread = original.handle.thread().id() == thread::current().id();
            let deadline = Instant::now() + crate::worker::WORKER_EXIT_BUDGET;
            while !same_thread && !signalled(&original.handle) && Instant::now() < deadline {
                thread::sleep(POLL);
            }
            if signalled(&original.handle) {
                let _ = original.handle.join();
                drop(original._permit);
            } else {
                retained().lock_recover().push(original);
            }
        }
    }
}
struct Job {
    key: Key,
    generation: u64,
    original: thread::JoinHandle<()>,
    retired: Option<Box<dyn FnOnce(bool) + Send>>,
}
struct Batch {
    jobs: Vec<Job>,
    shared: Arc<Shared>,
    _permit: Arc<Permit>,
}
fn batches() -> &'static Mutex<Vec<Batch>> {
    static OWNERS: OnceLock<Mutex<Vec<Batch>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(Vec::new()))
}
fn retire_callback(retired: Option<Box<dyn FnOnce(bool) + Send>>, successful: bool) {
    if let Some(retired) = retired {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| retired(successful)));
    }
}
fn reap_batch(batch: &mut Batch) {
    let mut index = 0;
    while index < batch.jobs.len() {
        if signalled(&batch.jobs[index].original) {
            let mut job = batch.jobs.swap_remove(index);
            let successful = job.original.join().is_ok();
            complete(&batch.shared, job.key, job.generation);
            retire_callback(job.retired.take(), successful);
        } else {
            index += 1;
        }
    }
}
fn reap_batches() {
    let pending = std::mem::take(&mut *batches().lock_recover());
    let mut unfinished = Vec::new();
    for mut batch in pending {
        reap_batch(&mut batch);
        if !batch.jobs.is_empty() {
            unfinished.push(batch);
        }
    }
    batches().lock_recover().extend(unfinished);
}
struct JobsOwner(Batch);
impl Drop for JobsOwner {
    fn drop(&mut self) {
        let cancelled = {
            let mut state = self.0.shared.state.lock_recover();
            state.stopped = true;
            let cancelled = state
                .entries
                .values_mut()
                .filter_map(|entry| entry.queued.take())
                .collect::<Vec<_>>();
            state.entries.retain(|_, entry| entry.running);
            cancelled
        };
        self.0.shared.changed.notify_all();
        for work in cancelled {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work.cancel()));
        }
        if self.0.jobs.is_empty() {
            return;
        }
        batches().lock_recover().push(Batch {
            jobs: std::mem::take(&mut self.0.jobs),
            shared: Arc::clone(&self.0.shared),
            _permit: Arc::clone(&self.0._permit),
        });
    }
}
fn complete(shared: &Shared, key: Key, _generation: u64) {
    let mut state = shared.state.lock_recover();
    if let Some(entry) = state.entries.get_mut(&key) {
        entry.running = false;
        // At most one original runs per key. A superseding queued operation
        // keeps the entry; cancellation without replacement frees it on join.
        if entry.queued.is_none() {
            state.entries.remove(&key);
        }
    }
    shared.changed.notify_all();
}
fn dispatch(shared: Arc<Shared>, permit: Arc<Permit>) {
    let mut owner = JobsOwner(Batch {
        jobs: Vec::new(),
        shared: Arc::clone(&shared),
        _permit: permit,
    });
    #[cfg(test)]
    {
        let panic_before_jobs = shared.state.lock_recover().panic_before_jobs;
        if panic_before_jobs {
            panic!("injected empty dispatcher unwind");
        }
    }
    loop {
        reap_batch(&mut owner.0);
        let next = {
            let mut state = shared.state.lock_recover();
            while !state.started {
                state = shared
                    .changed
                    .wait_timeout(state, POLL)
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
            if state.stopped {
                if owner.0.jobs.is_empty() {
                    return;
                }
                None
            } else if owner.0.jobs.len() < MAX_JOBS {
                let now = Instant::now();
                let key = state
                    .entries
                    .iter()
                    .find(|(_, entry)| {
                        !entry.running && entry.queued.is_some() && entry.deadline <= now
                    })
                    .map(|(key, _)| *key);
                key.map(|key| {
                    let entry = state.entries.get_mut(&key).unwrap();
                    entry.running = true;
                    (key, entry.generation, entry.queued.take().unwrap())
                })
            } else {
                None
            }
        };
        if let Some((key, generation, mut work)) = next {
            let ticket = Ticket {
                shared: Arc::downgrade(&shared),
                key,
                generation,
            };
            let run = work.run;
            let admitted = Arc::new(AtomicBool::new(false));
            let start = Arc::clone(&admitted);
            match thread::Builder::new()
                .name("qc-owned-background-operation".into())
                .spawn(move || {
                    while !start.load(Ordering::Acquire) {
                        thread::sleep(POLL);
                    }
                    if ticket.current() {
                        run(ticket);
                    }
                }) {
                Ok(original) => {
                    owner.0.jobs.push(Job {
                        key,
                        generation,
                        original,
                        retired: work.retired.take(),
                    });
                    admitted.store(true, Ordering::Release);
                }
                Err(_) => {
                    complete(&shared, key, generation);
                    retire_callback(work.retired.take(), false);
                }
            }
            continue;
        }
        let state = shared.state.lock_recover();
        let _ = shared
            .changed
            .wait_timeout(state, POLL)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static FIXTURES: Mutex<()> = Mutex::new(());
    #[derive(Clone)]
    struct Gate(Arc<(Mutex<bool>, Condvar)>);
    impl Gate {
        fn new() -> Self {
            Self(Arc::new((Mutex::new(false), Condvar::new())))
        }
        fn release(&self) {
            *self.0 .0.lock_recover() = true;
            self.0 .1.notify_all();
        }
        fn wait(&self) {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut released = self.0 .0.lock_recover();
            while !*released {
                assert!(
                    Instant::now() < deadline,
                    "owned test gate was not independently released"
                );
                released = self
                    .0
                     .1
                    .wait_timeout(released, POLL)
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
        }
    }
    struct Release(Gate);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    fn finish(manager: &Manager) {
        manager
            .finish_before(Instant::now() + Duration::from_secs(2))
            .unwrap();
    }
    struct FixtureThread(Option<Original>);
    impl FixtureThread {
        fn spawn(run: impl FnOnce() + Send + 'static) -> Self {
            reap_retained();
            let admitted = permit().unwrap();
            let handle = thread::spawn(run);
            Self(Some(Original {
                handle,
                _permit: admitted,
            }))
        }
    }
    impl Drop for FixtureThread {
        fn drop(&mut self) {
            let Some(original) = self.0.take() else {
                return;
            };
            let deadline = Instant::now() + Duration::from_secs(2);
            while !signalled(&original.handle) && Instant::now() < deadline {
                thread::sleep(POLL);
            }
            if signalled(&original.handle) {
                let _ = original.handle.join();
                drop(original._permit);
            } else {
                retained().lock_recover().push(original);
            }
        }
    }
    #[test]
    fn repeated_timer_key_replaces_the_deadline_and_old_callback_before_dispatch() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let gate = Gate::new();
        let _release = Release(gate.clone());
        let (sent, answer) = mpsc::channel();
        let old = sent.clone();
        manager
            .schedule_with(
                [1, 2, 0],
                Instant::now(),
                Work::new(move |_| {
                    old.send(("old", Instant::now())).unwrap();
                }),
                move |builder, body| {
                    builder.spawn(move || {
                        gate.wait();
                        body();
                    })
                },
            )
            .unwrap();
        let latest = Instant::now() + Duration::from_millis(120);
        manager
            .schedule(
                [1, 2, 0],
                latest,
                Work::new(move |_| {
                    sent.send(("latest", Instant::now())).unwrap();
                }),
            )
            .unwrap();
        assert_eq!(manager.shared.state.lock_recover().entries.len(), 1);
        _release.0.release();
        let (which, dispatched) = answer.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(which, "latest");
        assert!(
            dispatched >= latest,
            "coalescing must use the newest deadline, never the original expired one"
        );
        finish(&manager);
        assert!(answer.try_recv().is_err());
    }
    #[test]
    fn distinct_logical_key_capacity_refuses_without_growing_the_queue() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let deadline = Instant::now() + Duration::from_secs(60);
        for index in 0..MAX_KEYS {
            manager
                .schedule(
                    [0, index as u64, 0],
                    deadline,
                    Work::new(|_| panic!("future work must cancel")),
                )
                .unwrap();
        }
        assert!(manager
            .schedule(
                [0, MAX_KEYS as u64, 0],
                deadline,
                Work::new(|_| panic!("over-cap work must never start"))
            )
            .unwrap_err()
            .contains("logical-key"));
        assert_eq!(manager.shared.state.lock_recover().entries.len(), MAX_KEYS);
        manager
            .schedule(
                [0, 0, 0],
                deadline,
                Work::new(|_| panic!("replacement must cancel")),
            )
            .unwrap();
        assert_eq!(manager.shared.state.lock_recover().entries.len(), MAX_KEYS);
        finish(&manager);
        assert!(manager
            .shared
            .state
            .lock_recover()
            .entries
            .values()
            .all(|entry| entry.queued.is_none()));
    }
    #[test]
    fn four_original_jobs_bound_execution_and_queue_the_fifth_until_join() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let gate = Gate::new();
        let _release = Release(gate.clone());
        let (entered, entries) = mpsc::channel();
        let (retired, retirements) = mpsc::channel();
        for index in 0..MAX_JOBS + 1 {
            let gate = gate.clone();
            let entered = entered.clone();
            let retired = retired.clone();
            manager
                .schedule(
                    [2, index as u64, 0],
                    Instant::now(),
                    Work::new(move |_| {
                        entered.send(index).unwrap();
                        gate.wait();
                    })
                    .on_retired(move |successful| {
                        retired.send((index, successful)).unwrap();
                    }),
                )
                .unwrap();
        }
        for _ in 0..MAX_JOBS {
            entries.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        assert!(
            entries.try_recv().is_err(),
            "fifth job cannot start while four actual originals are live"
        );
        assert_eq!(
            manager
                .shared
                .state
                .lock_recover()
                .entries
                .values()
                .filter(|entry| entry.running)
                .count(),
            MAX_JOBS
        );
        _release.0.release();
        entries.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..MAX_JOBS + 1 {
            assert!(retirements.recv_timeout(Duration::from_secs(2)).unwrap().1);
        }
        finish(&manager);
    }
    #[test]
    fn a_superseded_expired_operation_refuses_at_its_late_admission_boundary() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let gate = Gate::new();
        let _release = Release(gate.clone());
        let (entered, entry) = mpsc::channel();
        let (dispatched, dispatches) = mpsc::channel();
        let old_gate = gate.clone();
        let old_output = dispatched.clone();
        manager
            .schedule(
                [3, 7, 0],
                Instant::now(),
                Work::new(move |ticket| {
                    entered.send(ticket.clone()).unwrap();
                    old_gate.wait();
                    ticket.with_current(|| old_output.send("old native mutation").unwrap());
                }),
            )
            .unwrap();
        let old_ticket = entry.recv_timeout(Duration::from_secs(2)).unwrap();
        manager
            .schedule(
                [3, 7, 0],
                Instant::now(),
                Work::new(move |ticket| {
                    ticket.with_current(|| dispatched.send("new").unwrap());
                }),
            )
            .unwrap();
        assert!(!old_ticket.current());
        _release.0.release();
        assert_eq!(
            dispatches.recv_timeout(Duration::from_secs(2)).unwrap(),
            "new"
        );
        finish(&manager);
        assert!(dispatches.try_recv().is_err());
        // Removing/reusing the same key must not resurrect a prior ticket.
        assert!(!old_ticket.current());
    }
    #[test]
    fn original_job_retirement_guard_survives_stop_and_body_completion_until_join() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let gate = Gate::new();
        let _release = Release(gate.clone());
        let (entered, entry) = mpsc::channel();
        let (retired, retirement) = mpsc::channel();
        manager
            .schedule(
                [4, 0, 0],
                Instant::now(),
                Work::new(move |_| {
                    entered.send(()).unwrap();
                    gate.wait();
                })
                .on_retired(move |successful| {
                    retired.send(successful).unwrap();
                }),
            )
            .unwrap();
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(manager
            .finish_before(Instant::now() + Duration::from_millis(20))
            .unwrap_err()
            .contains("unacknowledged"));
        assert!(
            retirement.try_recv().is_err(),
            "stopped does not certify original retirement or release accounting guard"
        );
        assert!(!manager.acknowledged());
        _release.0.release();
        assert!(retirement.recv_timeout(Duration::from_secs(2)).unwrap());
        finish(&manager);
    }
    #[test]
    fn failed_dispatcher_spawn_reports_and_retires_never_admitted_work_once() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let (retired, retirement) = mpsc::channel();
        let error = manager
            .schedule_with(
                [5, 0, 0],
                Instant::now(),
                Work::new(|_| panic!("failed spawn cannot dispatch")).on_retired(
                    move |successful| {
                        retired.send(successful).unwrap();
                    },
                ),
                |_builder, _body| Err(io::Error::other("injected spawn failure")),
            )
            .unwrap_err();
        assert!(error.contains("spawn failure"));
        assert!(!retirement.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(retirement.try_recv().is_err());
        assert!(manager.acknowledged());
        assert!(manager.shared.state.lock_recover().stopped);
    }
    #[test]
    fn dispatcher_unwind_retains_original_jobs_and_permit_until_real_join() {
        let _fixture = FIXTURES.lock_recover();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let gate = Gate::new();
        let release = Release(gate.clone());
        let permit = permit().unwrap();
        let held = Arc::downgrade(&permit);
        let (entered, entry) = mpsc::channel();
        let (retired, retirement) = mpsc::channel();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut owned = JobsOwner(Batch {
                jobs: Vec::new(),
                shared,
                _permit: permit,
            });
            let original = thread::spawn(move || {
                entered.send(()).unwrap();
                gate.wait();
            });
            owned.0.jobs.push(Job {
                key: [6, 0, 0],
                generation: 1,
                original,
                retired: Some(Box::new(move |successful| {
                    retired.send(successful).unwrap();
                })),
            });
            entry.recv_timeout(Duration::from_secs(1)).unwrap();
            panic!("injected dispatcher unwind");
        }));
        assert!(failure.is_err());
        assert!(held.upgrade().is_some());
        assert!(retirement.try_recv().is_err());
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            reap_batches();
            if held.upgrade().is_none() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        }
        assert!(retirement.recv_timeout(Duration::from_secs(1)).unwrap());
    }
    #[test]
    fn replacement_cancellation_can_reenter_the_same_manager_without_owner_locks() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Arc::new(Manager::default());
        let weak = Arc::downgrade(&manager);
        let deadline = Instant::now() + Duration::from_secs(60);
        let (reentered, reentry) = mpsc::channel();
        manager
            .schedule(
                [7, 0, 0],
                deadline,
                Work::new(|_| panic!("future work must cancel")).on_retired(move |_| {
                    let manager = weak.upgrade().unwrap();
                    let result = manager.schedule(
                        [7, 1, 0],
                        deadline,
                        Work::new(|_| panic!("future nested work must cancel")),
                    );
                    reentered.send(result).unwrap();
                }),
            )
            .unwrap();
        let caller = Arc::clone(&manager);
        let (finished, completion) = mpsc::channel();
        let _owner = FixtureThread::spawn(move || {
            let result = caller.schedule(
                [7, 0, 0],
                deadline,
                Work::new(|_| panic!("replacement must cancel")),
            );
            let _ = finished.send(result);
        });
        completion
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        reentry
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(manager.shared.state.lock_recover().entries.len(), 2);
        finish(&manager);
    }
    #[test]
    fn empty_dispatcher_unwind_retires_queued_work_and_refuses_later_admission() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        manager.shared.state.lock_recover().panic_before_jobs = true;
        let (retired, retirement) = mpsc::channel();
        manager
            .schedule(
                [8, 0, 0],
                Instant::now(),
                Work::new(|_| panic!("empty dispatcher never admitted operation")).on_retired(
                    move |successful| {
                        retired.send(successful).unwrap();
                    },
                ),
            )
            .unwrap();
        assert!(!retirement.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(manager.shared.state.lock_recover().stopped);
        assert!(manager
            .schedule(
                [8, 1, 0],
                Instant::now(),
                Work::new(|_| panic!("finished dispatcher must refuse"))
            )
            .is_err());
        assert!(retirement.try_recv().is_err());
        finish(&manager);
    }
    #[test]
    fn a_panicking_cancel_callback_does_not_skip_other_original_guard_retirement() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let deadline = Instant::now() + Duration::from_secs(60);
        manager
            .schedule(
                [9, 0, 0],
                deadline,
                Work::new(|_| panic!("cancelled future operation"))
                    .on_retired(|_| panic!("injected guard callback panic")),
            )
            .unwrap();
        let (retired, retirement) = mpsc::channel();
        manager
            .schedule(
                [9, 1, 0],
                deadline,
                Work::new(|_| panic!("cancelled second future operation")).on_retired(
                    move |successful| {
                        retired.send(successful).unwrap();
                    },
                ),
            )
            .unwrap();
        finish(&manager);
        assert!(!retirement.recv_timeout(Duration::from_secs(1)).unwrap());
        assert!(retirement.try_recv().is_err());
        assert!(manager.acknowledged());
    }
    #[test]
    fn coalesced_reconnect_keeps_bounded_requesters_and_one_original_operation() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let gate = Gate::new();
        let release = Release(gate.clone());
        let (sent, answer) = mpsc::channel();
        manager
            .enqueue_with(
                [3, 99, 0],
                Instant::now(),
                Work::new(move |ticket| {
                    ticket.with_requesters(|ids| sent.send(ids.to_vec()).unwrap());
                }),
                Scheduling::Requested(0),
                move |builder, body| {
                    builder.spawn(move || {
                        gate.wait();
                        body();
                    })
                },
            )
            .unwrap();
        for id in 1..BROKER_MAX_CONNECTION_OWNERS as u64 {
            manager
                .requested(
                    [3, 99, 0],
                    id,
                    Work::new(|_| panic!("merged reconnect must not allocate another operation")),
                )
                .unwrap();
        }
        assert!(manager
            .requested(
                [3, 99, 0],
                BROKER_MAX_CONNECTION_OWNERS as u64,
                Work::new(|_| panic!("over-cap requester must never run"))
            )
            .unwrap_err()
            .contains("requester capacity"));
        assert_eq!(manager.shared.state.lock_recover().entries.len(), 1);
        drop(release);
        assert_eq!(
            answer.recv_timeout(Duration::from_secs(2)).unwrap(),
            (0..BROKER_MAX_CONNECTION_OWNERS as u64).collect::<Vec<_>>()
        );
        finish(&manager);
        assert!(answer.try_recv().is_err());
    }
    #[test]
    fn an_old_callback_cannot_rearm_over_a_newer_timer_deadline() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let gate = Gate::new();
        let release = Release(gate.clone());
        let (sent, answer) = mpsc::channel();
        manager
            .schedule(
                [10, 0, 0],
                Instant::now(),
                Work::new(move |ticket| {
                    sent.send(ticket).unwrap();
                    gate.wait();
                }),
            )
            .unwrap();
        let old = answer.recv_timeout(Duration::from_secs(1)).unwrap();
        let newest = Instant::now() + Duration::from_secs(60);
        manager
            .latest(
                [10, 0, 0],
                newest,
                Work::new(|_| panic!("future replacement cancelled at finish")),
            )
            .unwrap();
        manager
            .reschedule(
                &old,
                Instant::now(),
                Work::new(|_| panic!("stale rearm must never publish")),
            )
            .unwrap();
        assert_eq!(
            manager
                .shared
                .state
                .lock_recover()
                .entries
                .get(&[10, 0, 0])
                .unwrap()
                .deadline,
            newest
        );
        // Out-of-order callers cannot bring a latest-deadline grace backward.
        manager
            .latest(
                [10, 0, 0],
                Instant::now(),
                Work::new(|_| panic!("early replacement still uses newest deadline")),
            )
            .unwrap();
        assert_eq!(
            manager
                .shared
                .state
                .lock_recover()
                .entries
                .get(&[10, 0, 0])
                .unwrap()
                .deadline,
            newest
        );
        drop(release);
        finish(&manager);
    }
    #[test]
    fn cancelled_key_reuse_does_not_resurrect_its_old_ticket_or_retain_empty_keys() {
        let _fixture = FIXTURES.lock_recover();
        let manager = Manager::default();
        let key = [11, 0, 0];
        manager
            .schedule(
                key,
                Instant::now() + Duration::from_secs(60),
                Work::new(|_| panic!("old queued key must cancel")),
            )
            .unwrap();
        let old = Ticket {
            shared: Arc::downgrade(&manager.shared),
            key,
            generation: manager
                .shared
                .state
                .lock_recover()
                .entries
                .get(&key)
                .unwrap()
                .generation,
        };
        manager.cancel_key(key).unwrap();
        assert!(!manager
            .shared
            .state
            .lock_recover()
            .entries
            .contains_key(&key));
        let gate = Gate::new();
        let release = Release(gate.clone());
        let (sent, answer) = mpsc::channel();
        manager
            .schedule(
                key,
                Instant::now(),
                Work::new(move |ticket| {
                    sent.send(ticket).unwrap();
                    gate.wait();
                }),
            )
            .unwrap();
        let fresh = answer.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(fresh.current());
        assert!(!old.current());
        manager.cancel_key(key).unwrap();
        assert!(!fresh.current());
        drop(release);
        finish(&manager);
        assert!(!manager
            .shared
            .state
            .lock_recover()
            .entries
            .contains_key(&key));
    }
}
