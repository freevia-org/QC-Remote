//! Short-lived tools are owned processes, not provider application launches.
//! Start suspended, contain before resume, and pump pipes without reader threads.
use std::{
    io::{self, Read},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Pipes::PeekNamedPipe, Threading::*},
};

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
struct Handle(HANDLE);
// SAFETY: these owned kernel handles have no thread affinity. Transfer never
// duplicates ownership; each handle is closed once by its retained owner.
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
// One family owns healthy and unknown originals. This software limit includes
// active tools and retained cleanup; it is not an OS process limit.
struct ToolLimit(std::sync::atomic::AtomicUsize, usize);
struct ToolPermit(std::sync::Arc<ToolLimit>);
impl Drop for ToolPermit {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
fn tool_limit() -> &'static std::sync::Arc<ToolLimit> {
    static LIMIT: std::sync::OnceLock<std::sync::Arc<ToolLimit>> = std::sync::OnceLock::new();
    LIMIT.get_or_init(|| std::sync::Arc::new(ToolLimit(std::sync::atomic::AtomicUsize::new(0), 16)))
}
struct Tool {
    ownership: Option<ToolOwnership>,
}
struct ToolOwnership {
    child: Child,
    job: Option<Handle>,
    members: Vec<Handle>,
    cleanup_attempted: bool,
    cleanup_confirmed: bool,
    _permit: ToolPermit,
    #[cfg(test)]
    cleanup_failure: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
impl std::ops::Deref for Tool {
    type Target = ToolOwnership;
    fn deref(&self) -> &Self::Target {
        self.ownership.as_ref().unwrap()
    }
}
impl std::ops::DerefMut for Tool {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ownership.as_mut().unwrap()
    }
}
fn retired_tools() -> &'static std::sync::Mutex<Vec<Tool>> {
    static RETIRED: std::sync::Mutex<Vec<Tool>> = std::sync::Mutex::new(Vec::new());
    &RETIRED
}
impl Drop for Tool {
    fn drop(&mut self) {
        if self.ownership.is_none() {
            return;
        }
        if !self.cleanup_attempted {
            let _ = self.cleanup();
        }
        if !self.cleanup_confirmed {
            // Move the same Child, job, member handles and permit, never just a PID.
            let original = Tool {
                ownership: self.ownership.take(),
            };
            retired_tools()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(original);
            use std::io::Write;
            let _ = writeln!(
                io::stderr(),
                "Status tool cleanup unacknowledged; exact original and admission retained"
            );
        }
    }
}
fn reap_retired_tools() {
    // OS calls, diagnostics and acknowledged destructors stay outside the mutex.
    let originals = std::mem::take(&mut *retired_tools().lock().unwrap_or_else(|p| p.into_inner()));
    let mut pending = Vec::new();
    // One small cooperative sweep, not a per-record wait budget. OS calls are
    // not forcibly cancellable; no WaitForSingleObject below may block here.
    let deadline = Instant::now() + Duration::from_millis(10);
    for mut original in originals {
        if Instant::now() >= deadline || original.cleanup_until(deadline, true).is_err() {
            pending.push(original);
        }
    }
    retired_tools()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .extend(pending);
}
impl Tool {
    fn spawn(command: &mut Command, deadline: Instant) -> io::Result<Self> {
        Self::spawn_with_limit(command, deadline, tool_limit().clone())
    }
    #[allow(
        deprecated,
        reason = "Retain fetch_update for stable toolchains predating Atomic::try_update"
    )]
    fn spawn_with_limit(
        command: &mut Command,
        deadline: Instant,
        limit: std::sync::Arc<ToolLimit>,
    ) -> io::Result<Self> {
        reap_retired_tools();
        limit
            .0
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |count| {
                    if count < limit.1 {
                        Some(count + 1)
                    } else {
                        None
                    }
                },
            )
            .map_err(|_| io::Error::other("Previous status tools still own cleanup admission"))?;
        let permit = ToolPermit(limit);
        if Instant::now() >= deadline {
            return Err(deadline_error());
        }
        // Callers are short-lived inspections or scoped system ACL setup; user
        // requested provider launches deliberately do not use this wrapper.
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn()?;
        // Child itself retains the exact process handle; no fallible duplicate
        // is needed between successful spawn and ownership.
        let mut tool = Self {
            ownership: Some(ToolOwnership {
                child,
                job: None,
                members: Vec::new(),
                cleanup_attempted: false,
                cleanup_confirmed: false,
                _permit: permit,
                #[cfg(test)]
                cleanup_failure: None,
            }),
        };
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        tool.job = Some(Handle(raw));
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(raw, tool.child.as_raw_handle()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if Instant::now() >= deadline {
            return Err(deadline_error());
        }
        resume_initial_thread(tool.child.id(), tool.child.as_raw_handle(), deadline)?;
        Ok(tool)
    }
    fn stop_descendants(&mut self, deadline: Instant) -> io::Result<()> {
        self.capture_members(deadline)?;
        if unsafe { TerminateJobObject(self.job.as_ref().unwrap().0, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn capture_members(&mut self, deadline: Instant) -> io::Result<()> {
        let Some(job) = self.job.as_ref().map(|job| job.0) else {
            return Ok(());
        };
        const CAPACITY: usize = 4096;
        let offset = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);
        let bytes = offset + CAPACITY * std::mem::size_of::<usize>();
        let mut buffer = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "status-tool member inventory deadline expired",
            ));
        }
        let list = buffer
            .as_mut_ptr()
            .cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
        // usize storage is aligned for this variable-length Win32 structure.
        if unsafe {
            QueryInformationJobObject(
                job,
                JobObjectBasicProcessIdList,
                list.cast(),
                bytes as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::other(format!(
                "status-tool job member query failed: {}",
                io::Error::last_os_error()
            )));
        }
        let count = unsafe { (*list).NumberOfProcessIdsInList as usize };
        if count > CAPACITY || unsafe { (*list).NumberOfAssignedProcesses as usize } > CAPACITY {
            return Err(io::Error::other(
                "status-tool job member inventory exceeded 4096 processes",
            ));
        }
        let pids = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<u8>().add(offset).cast::<usize>(),
                count,
            )
        };
        for &pid in pids {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "status-tool member inventory deadline expired",
                ));
            }
            let pid =
                u32::try_from(pid).map_err(|_| io::Error::other("invalid status-tool job PID"))?;
            if pid == self.child.id()
                || self
                    .members
                    .iter()
                    .any(|member| unsafe { GetProcessId(member.0) } == pid)
            {
                continue;
            }
            if self.members.len() >= CAPACITY {
                return Err(io::Error::other(
                    "status-tool retained member limit exceeded",
                ));
            }
            let raw = unsafe {
                OpenProcess(
                    PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    pid,
                )
            };
            if raw.is_null() {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                    continue;
                }
                return Err(io::Error::other(format!(
                    "status-tool member {pid} open failed: {error}"
                )));
            }
            let handle = Handle(raw);
            let mut owned = 0;
            if unsafe { IsProcessInJob(handle.0, job, &mut owned) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if owned != 0 {
                self.members.push(handle);
            }
            // A PID reused outside this job is never retained, waited or killed.
        }
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        self.cleanup_with_wait(false)
    }
    fn cleanup_with_wait(&mut self, nonblocking: bool) -> io::Result<()> {
        self.cleanup_until(Instant::now() + Duration::from_secs(1), nonblocking)
    }
    fn cleanup_until(&mut self, deadline: Instant, nonblocking: bool) -> io::Result<()> {
        self.cleanup_attempted = true;
        #[cfg(test)]
        if self
            .cleanup_failure
            .as_ref()
            .is_some_and(|failure| failure.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(io::Error::other(
                "injected cleanup observation failure before termination",
            ));
        }
        let mut errors = Vec::new();
        if let Err(error) = self.capture_members(deadline) {
            errors.push(error.to_string());
        }
        let mut terminated_members = self.members.len();
        if let Some(job) = &self.job {
            if unsafe { TerminateJobObject(job.0, 1) } == 0 {
                errors.push(format!(
                    "terminate status-tool job failed: {}",
                    io::Error::last_os_error()
                ));
            }
        }
        if let Err(error) = self.child.kill() {
            if unsafe { WaitForSingleObject(self.child.as_raw_handle(), 0) } != WAIT_OBJECT_0 {
                errors.push(format!("terminate status-tool primary failed: {error}"));
            }
        }
        // Refresh after termination to cover a child admitted between the
        // pre-termination snapshot and termination. Retained handles pin identity.
        if let Err(error) = self.capture_members(deadline) {
            errors.push(error.to_string());
        }
        if self.members.len() > terminated_members {
            if let Some(job) = &self.job {
                if unsafe { TerminateJobObject(job.0, 1) } == 0 {
                    errors.push(format!(
                        "terminate newly admitted status-tool members failed: {}",
                        io::Error::last_os_error()
                    ));
                }
            }
            terminated_members = self.members.len();
        }
        let mut confirmed = 0;
        loop {
            let handles: Vec<HANDLE> = std::iter::once(self.child.as_raw_handle())
                .chain(self.members.iter().map(|member| member.0))
                .collect();
            if let Err(error) =
                wait_member_signals(&handles[confirmed..], deadline, |handle, millis| unsafe {
                    WaitForSingleObject(handle, if nonblocking { 0 } else { millis })
                })
            {
                errors.push(error.to_string());
                break;
            }
            confirmed = handles.len();
            if let Err(error) = self.capture_members(deadline) {
                errors.push(error.to_string());
                break;
            }
            if self.members.len() + 1 == confirmed {
                break;
            }
            if self.members.len() > terminated_members {
                if let Some(job) = &self.job {
                    if unsafe { TerminateJobObject(job.0, 1) } == 0 {
                        errors.push(format!(
                            "terminate newly admitted status-tool members failed: {}",
                            io::Error::last_os_error()
                        ));
                    }
                }
                terminated_members = self.members.len();
            }
        }
        if errors.is_empty() {
            self.cleanup_confirmed = true;
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
}

fn wait_member_signals(
    handles: &[HANDLE],
    deadline: Instant,
    mut wait: impl FnMut(HANDLE, u32) -> u32,
) -> io::Result<()> {
    for &handle in handles {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(1000) as u32;
        match wait(handle, remaining) {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                "status-tool owned process did not signal within cleanup deadline (WAIT_TIMEOUT=258)",
                ))
            }
            code => {
                return Err(io::Error::other(format!(
                    "status-tool owned process wait failed ({code}): {}",
                    io::Error::last_os_error()
                )))
            }
        }
    }
    Ok(())
}

fn resume_initial_thread(pid: u32, process: HANDLE, deadline: Instant) -> io::Result<()> {
    // CREATE_SUSPENDED holds the primary thread before user code. Only accept
    // the sole thread of that held process, never an arbitrary matching thread.
    // Std's primary-thread accessor/job attributes are not stable on our toolchain.
    if unsafe { GetProcessId(process) } != pid {
        return Err(io::Error::other("status-tool process identity changed"));
    }
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = Handle(raw);
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of_val(&entry) as u32;
    let mut threads = Vec::new();
    let mut present = unsafe { Thread32First(snapshot.0, &mut entry) } != 0;
    while present {
        if entry.th32OwnerProcessID == pid {
            threads.push(entry.th32ThreadID);
        }
        present = unsafe { Thread32Next(snapshot.0, &mut entry) } != 0;
    }
    if threads.len() != 1 {
        return Err(io::Error::other(
            "could not identify the suspended status-tool primary thread",
        ));
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(io::Error::last_os_error());
    }
    let raw = unsafe {
        OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
            0,
            threads[0],
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let thread = Handle(raw);
    if unsafe { GetProcessIdOfThread(thread.0) } != pid || unsafe { GetProcessId(process) } != pid {
        return Err(io::Error::other("status-tool thread identity changed"));
    }
    if Instant::now() >= deadline {
        return Err(deadline_error());
    }
    if unsafe { ResumeThread(thread.0) } != 1 {
        return Err(io::Error::other(
            "status-tool primary thread was not initially suspended",
        ));
    }
    Ok(())
}

fn pump(pipe: &mut Option<impl Read + AsRawHandle>, bytes: &mut Vec<u8>) -> io::Result<bool> {
    let Some(pipe) = pipe.as_mut() else {
        return Ok(true);
    };
    let mut available = 0;
    if unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
            return Ok(true);
        }
        return Err(error);
    }
    if available > 0 {
        let count = (available as usize).min(64 * 1024);
        if bytes.len() + count > OUTPUT_LIMIT {
            return Err(io::Error::other("status-tool output exceeded 8 MiB"));
        }
        let mut buffer = vec![0; count];
        let read = pipe.read(&mut buffer)?;
        bytes.extend_from_slice(&buffer[..read]);
        if read == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn run(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    run_observing(command, timeout, Child::try_wait)
}
fn deadline_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "status tool or inherited output pipes exceeded the operation deadline",
    )
}
fn run_observing(
    command: &mut Command,
    timeout: Duration,
    observe: impl FnMut(&mut Child) -> io::Result<Option<std::process::ExitStatus>>,
) -> io::Result<Output> {
    let started = Instant::now();
    let deadline = started
        .checked_add(timeout)
        .ok_or_else(|| io::Error::other("invalid status-tool deadline"))?;
    if Instant::now() >= deadline {
        return Err(deadline_error());
    }
    let tool = Tool::spawn(command, deadline)?;
    observe_admitted(tool, deadline, observe)
}
fn observe_admitted(
    mut tool: Tool,
    deadline: Instant,
    mut observe: impl FnMut(&mut Child) -> io::Result<Option<std::process::ExitStatus>>,
) -> io::Result<Output> {
    let mut stdout = tool.child.stdout.take();
    let mut stderr = tool.child.stderr.take();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut status = None;
    let outcome = (|| loop {
        if Instant::now() >= deadline {
            return Err(deadline_error());
        }
        tool.capture_members(deadline)?;
        let out_done = pump(&mut stdout, &mut out)?;
        let err_done = pump(&mut stderr, &mut err)?;
        if status.is_none() {
            if let Some(exited) = observe(&mut tool.child)? {
                status = Some(exited);
                tool.stop_descendants(deadline)?;
            }
        }
        if let Some(status) = status {
            if out_done && err_done {
                if Instant::now() >= deadline {
                    return Err(deadline_error());
                }
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
        }
        std::thread::sleep(
            Duration::from_millis(2).min(deadline.saturating_duration_since(Instant::now())),
        );
    })();
    match (outcome, tool.cleanup()) {
        (Ok(output), Ok(())) => Ok(output),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(io::Error::new(
            error.kind(),
            format!("{error}; cleanup incomplete: {cleanup}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    const FIXTURE: &str = "owned_tool::tests::owned_status_tool_fixture";

    struct ReleaseCleanupFailure {
        failure: std::sync::Arc<std::sync::atomic::AtomicBool>,
        original: Option<std::sync::Arc<std::os::windows::io::OwnedHandle>>,
        identities: Vec<(u32, u64)>,
    }
    impl Drop for ReleaseCleanupFailure {
        fn drop(&mut self) {
            self.failure
                .store(false, std::sync::atomic::Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                reap_retired_tools();
                if (self.original.as_ref().is_none_or(|original| unsafe { WaitForSingleObject(original.as_raw_handle(), 0) } == WAIT_OBJECT_0)
                    && self.identities.iter().all(|(pid, created)| original_process_stopped(crate::os_process_is_alive(*pid), crate::os_process_created_at(*pid), *created)))
                    || Instant::now() >= deadline { break; }
                // Original resources remain retained if the OS still cannot acknowledge.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn an_unacknowledged_cleanup_retains_the_actual_tool_until_later_acknowledgement() {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        let fixture = Fixture::new();
        let failure = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut release = ReleaseCleanupFailure {
            failure: failure.clone(),
            original: None,
            identities: Vec::new(),
        };
        let setup_deadline = Instant::now() + Duration::from_secs(20);
        let mut tool = Tool::spawn(&mut command(&fixture.0, "timeout"), setup_deadline).unwrap();
        release.identities = fixture.wait_ready(2, setup_deadline);
        tool.cleanup_failure = Some(failure);
        let mut duplicate = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    tool.child.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let original = std::sync::Arc::new(unsafe { OwnedHandle::from_raw_handle(duplicate) });
        release.original = Some(original.clone());
        assert!(
            tool.cleanup().is_err(),
            "injected unknown must not report successful cleanup"
        );
        drop(tool);
        assert_eq!(
            unsafe { WaitForSingleObject(original.as_raw_handle(), 500) },
            WAIT_TIMEOUT,
            "an observation failure must retain the actual live original, not destroy its job"
        );
        drop(release);
        assert_eq!(
            unsafe { WaitForSingleObject(original.as_raw_handle(), 0) },
            WAIT_OBJECT_0,
            "later real cleanup must acknowledge the same original"
        );
        fixture.assert_stopped("retained-observation");
    }

    #[test]
    fn unknown_tool_cleanup_holds_its_same_admission_until_original_acknowledgement() {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        let fixture = Fixture::new();
        let limit = std::sync::Arc::new(ToolLimit(std::sync::atomic::AtomicUsize::new(0), 1));
        let failure = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut release = ReleaseCleanupFailure {
            failure: failure.clone(),
            original: None,
            identities: Vec::new(),
        };
        let setup_deadline = Instant::now() + Duration::from_secs(20);
        let mut tool = Tool::spawn_with_limit(
            &mut command(&fixture.0, "timeout"),
            setup_deadline,
            limit.clone(),
        )
        .unwrap();
        release.identities = fixture.wait_ready(2, setup_deadline);
        let mut duplicate = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    tool.child.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let original = std::sync::Arc::new(unsafe { OwnedHandle::from_raw_handle(duplicate) });
        release.original = Some(original.clone());
        tool.cleanup_failure = Some(failure);
        assert!(tool.cleanup().is_err());
        drop(tool);
        assert_eq!(limit.0.load(std::sync::atomic::Ordering::Acquire), 1);
        for _ in 0..8 {
            let refusal = Tool::spawn_with_limit(
                &mut Command::new("must-not-launch-status-tool"),
                Instant::now() + Duration::from_secs(1),
                limit.clone(),
            )
            .err()
            .expect("capacity must refuse before spawn");
            assert_eq!(
                refusal.to_string(),
                "Previous status tools still own cleanup admission"
            );
        }
        assert_eq!(
            unsafe { WaitForSingleObject(original.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        drop(release);
        assert_eq!(
            unsafe { WaitForSingleObject(original.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        let retired_before = Instant::now() + Duration::from_secs(3);
        while limit.0.load(std::sync::atomic::Ordering::Acquire) != 0 {
            reap_retired_tools();
            assert!(
                Instant::now() < retired_before,
                "same tool admission did not retire after release"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(limit.0.load(std::sync::atomic::Ordering::Acquire), 0);
        fixture.assert_stopped("retained-capacity");
        let mut invalid = Command::new("must-not-exist-after-tool-admission");
        assert!(Tool::spawn_with_limit(
            &mut invalid,
            Instant::now() + Duration::from_secs(1),
            limit.clone()
        )
        .is_err());
        assert_eq!(
            limit.0.load(std::sync::atomic::Ordering::Acquire),
            0,
            "failed spawn after readmission releases only its new permit"
        );
    }

    #[test]
    fn member_signal_confirmation_rejects_primary_only_exit_and_unknown_wait_results() {
        let handles = [1usize as HANDLE, 2usize as HANDLE];
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut visited = Vec::new();
        let error = wait_member_signals(&handles, deadline, |handle, millis| {
            visited.push(handle as usize);
            assert!(millis <= 1000, "every member shares one cleanup budget");
            if handle == handles[0] {
                WAIT_OBJECT_0
            } else {
                WAIT_TIMEOUT
            }
        })
        .unwrap_err();
        assert_eq!(
            visited,
            vec![1, 2],
            "primary exit does not skip descendant signal confirmation"
        );
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(wait_member_signals(&handles, deadline, |_, _| WAIT_FAILED).is_err());
        assert!(wait_member_signals(&handles, deadline, |_, _| WAIT_OBJECT_0).is_ok());
    }
    fn command(path: &Path, kind: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", FIXTURE, "--ignored", "--nocapture"])
            .env("QC_OWNED_TOOL_FIXTURE", path)
            .env("QC_OWNED_TOOL_KIND", kind)
            .stderr(Stdio::piped());
        command
    }
    struct Fixture(PathBuf, PathBuf);
    fn original_process_stopped(alive: bool, observed: Option<u64>, expected: u64) -> bool {
        !alive || observed.is_some_and(|observed| observed != expected)
    }
    impl Fixture {
        fn new() -> Self {
            for _ in 0..64 {
                let root = std::env::temp_dir()
                    .join(format!("qc-owned-tool-{:032x}", rand::random::<u128>()));
                match std::fs::create_dir(&root) {
                    Ok(()) => return Self(root.join("identities.json"), root),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("reserve owned-tool fixture root: {error}"),
                }
            }
            panic!("could not reserve owned-tool fixture after 64 collisions")
        }
        fn identities(&self) -> io::Result<Vec<(u32, u64)>> {
            match std::fs::read(&self.0) {
                Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(error) => Err(error),
            }
        }
        fn wait_ready(&self, expected: usize, deadline: Instant) -> Vec<(u32, u64)> {
            loop {
                assert!(
                    Instant::now() < deadline,
                    "controlled fixture readiness deadline expired"
                );
                let observation = self.identities();
                assert!(
                    Instant::now() < deadline,
                    "controlled fixture readiness deadline expired"
                );
                match observation {
                    Ok(identities) if identities.len() == expected => return identities,
                    Ok(identities) if identities.is_empty() => {}
                    Ok(identities) => panic!(
                        "controlled receipt had {} originals, expected {expected}",
                        identities.len()
                    ),
                    // std::fs::write can expose an incomplete JSON prefix;
                    // no malformed complete receipt or unrelated IO is ignored.
                    Err(error)
                        if error
                            .get_ref()
                            .and_then(|cause| cause.downcast_ref::<serde_json::Error>())
                            .is_some_and(|cause| cause.is_eof()) => {}
                    Err(error) => panic!("controlled fixture receipt failed: {error}"),
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn assert_stopped(&self, kind: &str) {
            let identities = self
                .identities()
                .expect("read complete child identity receipt");
            assert!(
                !identities.is_empty(),
                "fixture must execute before cleanup test"
            );
            for (index, (pid, created)) in identities.into_iter().enumerate() {
                assert!(
                    original_process_stopped(
                        crate::os_process_is_alive(pid),
                        crate::os_process_created_at(pid),
                        created,
                    ),
                    "owned fixture {pid} remained live (kind={kind}, role={}, created={created})",
                    if index == 0 { "primary" } else { "descendant" }
                );
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Independent cleanup still runs if a production cleanup assertion fails.
            let identities = match self.identities() {
                Ok(identities) => identities,
                Err(error) => {
                    eprintln!(
                        "owned-tool fixture identity receipt unreadable ({error}); preserving {}",
                        self.1.display()
                    );
                    return;
                }
            };
            let mut acknowledged = true;
            for (pid, created) in identities {
                if !crate::os_process_is_alive(pid) {
                    continue;
                }
                if crate::os_process_created_at(pid) == Some(created) {
                    let raw = unsafe {
                        OpenProcess(
                            PROCESS_TERMINATE
                                | PROCESS_SYNCHRONIZE
                                | PROCESS_QUERY_LIMITED_INFORMATION,
                            0,
                            pid,
                        )
                    };
                    if !raw.is_null() {
                        let handle = Handle(raw);
                        if crate::os_process_created_at(pid) == Some(created) {
                            let result = unsafe {
                                TerminateProcess(handle.0, 1);
                                WaitForSingleObject(handle.0, 1000)
                            };
                            acknowledged &= result == WAIT_OBJECT_0;
                        } else if unsafe { WaitForSingleObject(handle.0, 0) } != WAIT_OBJECT_0 {
                            // Do not kill when identity could not be reconfirmed;
                            // keep evidence until this exact retained handle exits.
                            acknowledged = false;
                        }
                    } else if crate::os_process_is_alive(pid) {
                        acknowledged = false;
                    }
                } else if crate::os_process_created_at(pid).is_none() {
                    // Unknown is not proof that the original child is gone.
                    acknowledged = false;
                }
            }
            if acknowledged {
                let _ = std::fs::remove_dir_all(&self.1);
            } else {
                eprintln!(
                    "owned-tool fixture cleanup unacknowledged; preserving {}",
                    self.1.display()
                );
            }
        }
    }
    #[test]
    fn owned_tool_cleanup_assertion_requires_known_exit_or_replaced_identity() {
        assert!(!original_process_stopped(true, None, 41));
        assert!(!original_process_stopped(true, Some(41), 41));
        assert!(original_process_stopped(true, Some(42), 41));
        assert!(original_process_stopped(false, None, 41));
        assert!(original_process_stopped(false, Some(41), 41));
    }
    #[test]
    fn owned_tool_fixture_retires_its_exclusive_root_on_setup_unwind() {
        let mut root = None;
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let fixture = Fixture::new();
            root = Some(fixture.1.clone());
            std::fs::write(&fixture.0, b"[]").unwrap();
            panic!("injected owned-tool fixture setup failure");
        }));
        assert!(unwind.is_err());
        assert!(!root.unwrap().exists());
    }
    #[test]
    fn owned_tool_fixture_preserves_a_partial_identity_receipt_on_unwind() {
        // This synthetic fixture starts no child. Its independent root guard
        // can therefore retire the bytes after inspecting failure evidence.
        struct NoChildRoot(PathBuf);
        impl Drop for NoChildRoot {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture::new();
        let root = NoChildRoot(fixture.1.clone());
        let receipt = fixture.0.clone();
        let unwind = std::panic::catch_unwind(move || {
            let _fixture = fixture;
            std::fs::write(&receipt, b"[[123,").unwrap();
            panic!("injected partial child receipt");
        });
        assert!(unwind.is_err());
        assert!(root.0.is_dir(), "unknown inventory must not claim cleanup");
        assert_eq!(
            std::fs::read(root.0.join("identities.json")).unwrap(),
            b"[[123,"
        );
    }
    #[test]
    #[ignore]
    fn owned_status_tool_fixture() {
        let Some(path) = std::env::var_os("QC_OWNED_TOOL_FIXTURE") else {
            return;
        };
        let kind = std::env::var("QC_OWNED_TOOL_KIND").unwrap();
        if kind == "descendant" {
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        let mut identities = vec![(
            std::process::id(),
            crate::os_process_created_at(std::process::id()).unwrap(),
        )];
        let mut nested = None;
        if kind == "inherited" || kind == "timeout" || kind == "error" || kind == "panic" {
            let mut descendant = command(Path::new(&path), "descendant");
            descendant
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .creation_flags(CREATE_NO_WINDOW);
            let child = descendant.spawn().unwrap();
            identities.push((
                child.id(),
                crate::os_process_created_at(child.id()).unwrap(),
            ));
            nested = Some(child);
        }
        std::fs::write(path, serde_json::to_vec(&identities).unwrap()).unwrap();
        if kind == "inherited" {
            std::process::exit(0);
        }
        if kind == "oversized" {
            use std::io::Write;
            std::io::stdout()
                .write_all(&vec![b'x'; OUTPUT_LIMIT + 1])
                .unwrap();
        }
        std::thread::sleep(Duration::from_secs(60));
        drop(nested);
    }
    #[test]
    fn admitted_observation_keeps_expired_deadline_and_retires_originals() {
        let fixture = Fixture::new();
        let setup_deadline = Instant::now() + Duration::from_secs(20);
        let tool = Tool::spawn(&mut command(&fixture.0, "timeout"), setup_deadline).unwrap();
        fixture.wait_ready(2, setup_deadline);
        let expired = Instant::now();
        let mut observed = 0;
        let error = observe_admitted(tool, expired, |_| {
            observed += 1;
            Ok(None)
        })
        .unwrap_err();
        assert_eq!(
            observed, 0,
            "an expired absolute deadline cannot enter observation"
        );
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            error.to_string(),
            "status tool or inherited output pipes exceeded the operation deadline"
        );
        fixture.assert_stopped("expired-handoff");
    }
    #[test]
    fn owned_tool_bounds_descendant_pipes_timeout_output_error_and_unwind() {
        for kind in ["inherited", "timeout", "oversized", "error", "panic"] {
            let fixture = Fixture::new();
            let mut observation_started = Instant::now();
            // Only the timeout scenario asserts a short operation budget.
            // The other cases must reach their actual intended outcome even
            // when fixture startup is slow; a timeout is not their oracle.
            let budget = if kind == "timeout" {
                Duration::from_secs(3)
            } else {
                Duration::from_secs(20)
            };
            let mut command = command(&fixture.0, kind);
            let mut observe = |child: &mut Child| {
                if fixture.0.exists() {
                    if kind == "error" {
                        return Err(io::Error::other("injected wait failure"));
                    }
                    if kind == "panic" {
                        panic!("injected observer unwind");
                    }
                }
                child.try_wait()
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if kind == "timeout" {
                    // Exercise a real running child rather than accepting a
                    // timeout during cold process admission as coverage.
                    let setup_deadline = Instant::now() + Duration::from_secs(20);
                    let tool = Tool::spawn(&mut command, setup_deadline)?;
                    fixture.wait_ready(2, setup_deadline);
                    observation_started = Instant::now();
                    observe_admitted(tool, observation_started + budget, &mut observe)
                } else {
                    run_observing(&mut command, budget, &mut observe)
                }
            }));
            match &result {
                Ok(Ok(output)) => eprintln!(
                    "owned-tool fixture kind={kind} outcome=status {}",
                    output.status
                ),
                Ok(Err(error)) => {
                    eprintln!("owned-tool fixture kind={kind} outcome=error {error:?}")
                }
                Err(_) => eprintln!("owned-tool fixture kind={kind} outcome=observer unwind"),
            }
            match kind {
                "inherited" => assert!(result.unwrap().unwrap().status.success()),
                "panic" => assert!(result.is_err()),
                "timeout" => {
                    assert_eq!(result.unwrap().unwrap_err().kind(), io::ErrorKind::TimedOut)
                }
                "oversized" => assert_eq!(
                    result.unwrap().unwrap_err().to_string(),
                    "status-tool output exceeded 8 MiB"
                ),
                "error" => assert_eq!(
                    result.unwrap().unwrap_err().to_string(),
                    "injected wait failure"
                ),
                _ => unreachable!(),
            }
            assert!(
                observation_started.elapsed() < budget + Duration::from_secs(3),
                "operation exceeded deadline+cleanup (timeout phase begins after controlled admission)"
            );
            fixture.assert_stopped(kind);
        }
    }
}
