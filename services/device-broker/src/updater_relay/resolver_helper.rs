//! OS DNS stays in an isolated child: Windows DoH/NRPT is not reimplemented.
//! The child has no broker, host, election or USB initialization and spawns no
//! descendants. The parent assigns its Windows job before releasing stdin.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
#[cfg(not(windows))]
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) const FLAG: &str = "--resolve-host-only";
const MAX_HOST: usize = 253;
const MAX_ADDRESSES: usize = 256;
const MAX_OUTPUT: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    version: u8,
    addresses: Vec<IpAddr>,
}

fn valid_host(host: &str) -> bool {
    let labels = host.strip_suffix('.').unwrap_or(host);
    !labels.is_empty()
        && host.len() <= MAX_HOST
        && host.is_ascii()
        && labels.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub(crate) fn serve(reader: impl Read, writer: impl Write) -> io::Result<()> {
    serve_with(reader, writer, |host| {
        (host, 0).to_socket_addrs().map(|a| a.collect())
    })
}

fn serve_with(
    mut reader: impl Read,
    mut writer: impl Write,
    lookup: impl FnOnce(&str) -> io::Result<Vec<SocketAddr>>,
) -> io::Result<()> {
    // EOF is the admission gate. Parent death before job assignment closes the
    // pipe without a hostname, so this child never enters OS resolution.
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((MAX_HOST + 1) as u64)
        .read_to_end(&mut bytes)?;
    let host =
        std::str::from_utf8(&bytes).map_err(|_| io::Error::other("invalid resolver input"))?;
    if !valid_host(host) {
        return Err(io::Error::other("invalid resolver input"));
    }
    let found = lookup(host)?;
    if found.is_empty() || found.len() > MAX_ADDRESSES {
        return Err(io::Error::other("invalid resolver answer"));
    }
    let answer = Answer {
        version: 1,
        addresses: found.into_iter().map(|a| a.ip()).collect(),
    };
    let output = serde_json::to_vec(&answer)?;
    if output.len() as u64 > MAX_OUTPUT {
        return Err(io::Error::other("resolver answer too large"));
    }
    writer.write_all(&output)
}

fn decode(bytes: &[u8]) -> io::Result<Vec<SocketAddr>> {
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(io::Error::other("resolver answer too large"));
    }
    let answer: Answer = serde_json::from_slice(bytes)?;
    if answer.version != 1 || answer.addresses.is_empty() || answer.addresses.len() > MAX_ADDRESSES
    {
        return Err(io::Error::other("invalid resolver answer"));
    }
    Ok(answer
        .addresses
        .into_iter()
        .map(|ip| SocketAddr::new(ip, 0))
        .collect())
}

fn helper_path(current: &Path) -> io::Result<PathBuf> {
    // Engine can also be embedded in the GUI. Never re-execute a GUI or a
    // library/test harness with a broker flag: only the shipped sibling broker
    // (or the broker itself) has the exact resolve-only dispatch.
    if matches!(
        current.file_stem().and_then(|s| s.to_str()),
        Some(
            "qc-device-broker"
                | "qc-device-broker-x86_64-pc-windows-msvc"
                | "qc-device-broker-x86_64-pc-windows-gnu"
                | "qc-device-broker-x86_64-unknown-linux-gnu"
                | "qc-device-broker-x86_64-unknown-linux-musl"
                | "qc-device-broker-aarch64-unknown-linux-gnu"
                | "qc-device-broker-aarch64-unknown-linux-musl"
        )
    ) {
        return Ok(current.to_owned());
    }
    let name = if cfg!(windows) {
        "qc-device-broker.exe"
    } else {
        "qc-device-broker"
    };
    let parent = current
        .parent()
        .ok_or_else(|| io::Error::other("missing broker directory"))?;
    let ordinary = parent.join(name);
    if ordinary.is_file() {
        return Ok(ordinary);
    }
    #[cfg(windows)]
    for name in [
        "qc-device-broker-x86_64-pc-windows-msvc.exe",
        "qc-device-broker-x86_64-pc-windows-gnu.exe",
    ] {
        let candidate = parent.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    #[cfg(target_os = "linux")]
    for target in [
        "x86_64-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-gnu",
        "aarch64-unknown-linux-musl",
    ] {
        let candidate = parent.join(format!("qc-device-broker-{target}"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Ok(ordinary) // Missing packaged helper fails at spawn; never try the GUI.
}

pub(super) async fn resolve(host: &str) -> io::Result<Vec<SocketAddr>> {
    if !valid_host(host) {
        return Err(io::Error::other("invalid resolver input"));
    }
    static BUDGET: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    sweep_retired_resolvers();
    let permit = BUDGET
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| io::Error::other("resolver unavailable"))?;
    let executable = helper_path(&std::env::current_exe()?)?;
    let mut command = tokio::process::Command::new(executable);
    command.arg(FLAG);
    run_owned(command, host, None, Some(permit)).await
}

// Unknown retirement keeps BOTH the original object and its admission charge.
// There are at most four production objects, because each holds a budget permit.
// A later request rechecks the exact originals; no per-cancellation reaper task,
// thread, PID lookup or runtime-dependent cleanup is needed.
static RETIRING: OnceLock<Mutex<Vec<RetainedResolver>>> = OnceLock::new();

struct RetainedResolver {
    child: tokio::process::Child,
    #[cfg(windows)]
    job: Option<Job>,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl RetainedResolver {
    fn terminal(&mut self, wait: Duration) -> bool {
        // Tokio retains the successful wait result even after closing its handle.
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return true;
        }
        #[cfg(windows)]
        {
            let handle = self
                .job
                .as_ref()
                .map(|job| job.process)
                .filter(|handle| !handle.is_null())
                .or_else(|| self.child.raw_handle().map(|handle| handle as _));
            handle.is_some_and(|handle| unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(
                    handle,
                    wait.as_millis().min(u32::MAX as u128 - 1) as u32,
                ) == windows_sys::Win32::Foundation::WAIT_OBJECT_0
            })
        }
        #[cfg(not(windows))]
        {
            let started = Instant::now();
            while started.elapsed() < wait {
                std::thread::sleep(
                    Duration::from_millis(10).min(wait.saturating_sub(started.elapsed())),
                );
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return true;
                }
            }
            false
        }
    }

    fn terminate(&mut self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            if job.assigned {
                unsafe {
                    windows_sys::Win32::System::JobObjects::TerminateJobObject(job.handle, 1);
                }
            }
        }
        let _ = self.child.start_kill();
    }
}

fn sweep_retired_resolvers() {
    if let Some(retiring) = RETIRING.get() {
        retiring
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain_mut(|original| !original.terminal(Duration::ZERO));
    }
}

fn retain_unconfirmed(
    original: RetainedResolver,
    confirmed: bool,
    retiring: &mut Vec<RetainedResolver>,
) {
    if !confirmed {
        retiring.push(original);
    }
}

struct ResolverOwner(Option<RetainedResolver>);
impl Drop for ResolverOwner {
    fn drop(&mut self) {
        let Some(mut original) = self.0.take() else {
            return;
        };
        original.terminate();
        let confirmed = original.terminal(Duration::from_secs(1));
        if !confirmed {
            eprintln!("Resolver retirement unconfirmed; retaining original child {:?} and its admission capacity", original.child.id());
        }
        retain_unconfirmed(
            original,
            confirmed,
            &mut RETIRING
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }
}

#[cfg(test)]
async fn run(
    command: tokio::process::Command,
    host: &str,
    deadline: Duration,
) -> io::Result<Vec<SocketAddr>> {
    run_owned(command, host, Some(deadline), None).await
}

async fn run_owned(
    mut command: tokio::process::Command,
    host: &str,
    deadline: Option<Duration>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> io::Result<Vec<SocketAddr>> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut owner = ResolverOwner(Some(RetainedResolver {
        child: command.spawn()?,
        #[cfg(windows)]
        job: None,
        _permit: permit,
    }));
    let original = owner.0.as_mut().unwrap();
    // Retain the job through output processing AND process exit. Any error,
    // timeout, dropped lookup or runtime shutdown closes it and kills the child.
    #[cfg(windows)]
    {
        original.job = Some(
            original
                .child
                .raw_handle()
                .ok_or_else(|| io::Error::other("missing child handle"))
                .and_then(Job::assign)?,
        );
    }
    let child = &mut original.child;
    let operation = async {
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing resolver input"))?;
        input.write_all(host.as_bytes()).await?;
        drop(input);
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing resolver output"))?;
        let mut bytes = Vec::new();
        output.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).await?;
        if bytes.len() as u64 > MAX_OUTPUT {
            return Err(io::Error::other("resolver answer too large"));
        }
        let status = child.wait().await?;
        if !status.success() {
            return Err(io::Error::other("name did not resolve"));
        }
        decode(&bytes)
    };
    let result = match deadline {
        Some(deadline) => tokio::time::timeout(deadline, operation)
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "name did not resolve",
                ))
            }),
        None => operation.await,
    };
    if result.is_err() {
        let _ = child.start_kill();
        // A defective helper cannot turn cleanup into an infinite wait.
        tokio::time::timeout(Duration::from_secs(1), child.wait())
            .await
            .map_err(|_| io::Error::other("resolver cleanup timed out"))??;
    }
    result
}

#[cfg(windows)]
struct Job {
    handle: windows_sys::Win32::Foundation::HANDLE,
    process: windows_sys::Win32::Foundation::HANDLE,
    assigned: bool,
}
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
impl Job {
    fn assign(process: std::os::windows::io::RawHandle) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut job = Self {
                handle,
                process: std::ptr::null_mut(),
                assigned: false,
            };
            let current = windows_sys::Win32::System::Threading::GetCurrentProcess();
            if windows_sys::Win32::Foundation::DuplicateHandle(
                current,
                process,
                current,
                &mut job.process,
                0,
                0,
                windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, process) == 0
            {
                return Err(io::Error::last_os_error());
            }
            job.assigned = true;
            Ok(job)
        }
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
            // ResolverOwner, not a timeout in this destructor, owns terminal
            // acknowledgement. Unknown originals retain this exact handle.
            if !self.process.is_null() {
                windows_sys::Win32::Foundation::CloseHandle(self.process);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_protocol_is_bounded_and_never_accepts_urls_ports_or_commands() {
        for host in [
            "",
            "https://example.org",
            "example.org:443",
            "a\nb",
            "-bad.test",
            "bad..test",
        ] {
            assert!(serve_with(host.as_bytes(), Vec::new(), |_| panic!(
                "invalid input reached DNS"
            ))
            .is_err());
        }
        let too_long = "a".repeat(MAX_HOST + 1);
        assert!(serve_with(too_long.as_bytes(), Vec::new(), |_| panic!(
            "oversized input reached DNS"
        ))
        .is_err());
        let expected = vec!["8.8.8.8:0".parse().unwrap(), "[::1]:0".parse().unwrap()];
        let mut output = Vec::new();
        serve_with("example.org".as_bytes(), &mut output, |_| {
            Ok(expected.clone())
        })
        .unwrap();
        assert_eq!(
            decode(&output).unwrap(),
            expected,
            "helper preserves mixed answers for parent refusal"
        );
        for bytes in [
            br#"{"version":2,"addresses":["8.8.8.8"]}"#.as_slice(),
            br#"{"version":1,"addresses":[],"extra":true}"#.as_slice(),
            br#"{"version":1,"addresses":["8.8.8.8:443"]}"#.as_slice(),
        ] {
            assert!(decode(bytes).is_err());
        }
        assert!(decode(&vec![b' '; MAX_OUTPUT as usize + 1]).is_err());
        let oversized = vec!["8.8.8.8:0".parse().unwrap(); MAX_ADDRESSES + 1];
        assert!(serve_with("example.org".as_bytes(), Vec::new(), |_| Ok(oversized)).is_err());
    }
    #[test]
    fn embedded_engine_selects_broker_sibling_never_gui_or_test_harness() {
        let root = Path::new("trusted-directory");
        let broker = root.join(if cfg!(windows) {
            "qc-device-broker.exe"
        } else {
            "qc-device-broker"
        });
        assert_eq!(helper_path(&broker).unwrap(), broker);
        assert_eq!(helper_path(&root.join("qc-remote.exe")).unwrap(), broker);
        assert_eq!(helper_path(&root.join("unit-tests.exe")).unwrap(), broker);
        for name in [
            "qc-device-broker-x86_64-pc-windows-msvc.exe",
            "qc-device-broker-x86_64-pc-windows-gnu.exe",
            "qc-device-broker-x86_64-unknown-linux-gnu",
            "qc-device-broker-x86_64-unknown-linux-musl",
            "qc-device-broker-aarch64-unknown-linux-gnu",
            "qc-device-broker-aarch64-unknown-linux-musl",
        ] {
            let target_broker = root.join(name);
            assert_eq!(helper_path(&target_broker).unwrap(), target_broker);
        }
    }

    // Test-harness child only: no DNS, network, broker or device. The parent
    // deliberately kills a stalled stdin-gated owned child on every path.
    #[test]
    #[ignore]
    fn resolver_process_fixture() {
        let Some(path) = std::env::var_os("QC_RESOLVER_FIXTURE_PID") else {
            return;
        };
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        assert_eq!(input, "example.org");
        #[cfg(windows)]
        let identity = unsafe {
            serde_json::json!({"pid":std::process::id(), "created":creation_time(windows_sys::Win32::System::Threading::GetCurrentProcess()).unwrap()})
        };
        #[cfg(not(windows))]
        let identity = serde_json::json!({"pid":std::process::id()});
        std::fs::write(path, identity.to_string()).unwrap();
        match std::env::var("QC_RESOLVER_FIXTURE_KIND").unwrap().as_str() {
            "oversized" => {
                std::io::stdout()
                    .write_all(&vec![b' '; MAX_OUTPUT as usize + 1])
                    .unwrap();
            }
            "nonzero" => {
                std::io::stdout()
                    .write_all(br#"{"version":1,"addresses":["8.8.8.8"]}"#)
                    .unwrap();
                std::process::exit(2);
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_secs(60));
    }

    #[cfg(windows)]
    fn process_fixture(kind: &str, path: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "updater_relay::resolver_helper::tests::resolver_process_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("QC_RESOLVER_FIXTURE_PID", path)
            .env("QC_RESOLVER_FIXTURE_KIND", kind);
        command
    }

    #[cfg(windows)]
    unsafe fn creation_time(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
        let mut created: windows_sys::Win32::Foundation::FILETIME = std::mem::zeroed();
        let mut exit = created;
        let mut kernel = created;
        let mut user = created;
        if windows_sys::Win32::System::Threading::GetProcessTimes(
            handle,
            &mut created,
            &mut exit,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return None;
        }
        Some(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
    }

    #[cfg(windows)]
    struct FixtureCleanup(PathBuf);
    #[cfg(windows)]
    impl FixtureCleanup {
        fn handle(&self) -> Option<FixtureHandle> {
            let identity: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&self.0).ok()?).ok()?;
            let pid = u32::try_from(identity.get("pid")?.as_u64()?).ok()?;
            let created = identity.get("created")?.as_u64()?;
            unsafe {
                use windows_sys::Win32::System::Threading::*;
                let raw = OpenProcess(
                    PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    pid,
                );
                if raw.is_null() {
                    return None;
                }
                let handle = FixtureHandle(raw);
                if creation_time(raw) == Some(created) {
                    Some(handle)
                } else {
                    None
                }
            }
        }
    }
    #[cfg(windows)]
    struct FixtureHandle(windows_sys::Win32::Foundation::HANDLE);
    #[cfg(windows)]
    impl Drop for FixtureHandle {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
    #[cfg(windows)]
    impl Drop for FixtureCleanup {
        fn drop(&mut self) {
            // Independent cleanup even if a production cleanup regression
            // makes an assertion fail. PID + creation-time avoids PID reuse.
            if let Some(handle) = self.handle() {
                unsafe {
                    windows_sys::Win32::System::Threading::TerminateProcess(handle.0, 2);
                    windows_sys::Win32::System::Threading::WaitForSingleObject(handle.0, 1000);
                }
            }
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn unconfirmed_original_retirement_keeps_admission_until_exact_terminal_proof() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let budget = Arc::new(tokio::sync::Semaphore::new(1));
            let permit = budget.clone().acquire_owned().await.unwrap();
            // This helper never receives a hostname: no DNS, broker or hardware.
            let path = std::env::temp_dir().join(format!(
                "qc-resolver-no-admission-{}.pid",
                uuid::Uuid::new_v4()
            ));
            let mut command = process_fixture("timeout", &path);
            command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .creation_flags(0x0800_0000);
            let mut owner = ResolverOwner(Some(RetainedResolver {
                child: command.spawn().unwrap(),
                job: None,
                _permit: Some(permit),
            }));
            let original = owner.0.as_mut().unwrap();
            original.job = Some(Job::assign(original.child.raw_handle().unwrap()).unwrap());
            assert!(
                !original.terminal(Duration::ZERO),
                "stdin-gated original is alive"
            );
            let mut retained = Vec::new();
            // Inject a failed/timeout acknowledgement at the same retention
            // boundary used by Drop. It must not return the admission charge.
            retain_unconfirmed(owner.0.take().unwrap(), false, &mut retained);
            assert_eq!(retained.len(), 1);
            assert_eq!(budget.available_permits(), 0);
            assert!(budget.clone().try_acquire_owned().is_err());
            retained.retain_mut(|original| !original.terminal(Duration::ZERO));
            assert_eq!(retained.len(), 1, "a sweep cannot discard a live original");
            // Reinstall the actual owner before assertions that could unwind.
            owner.0 = retained.pop();
            let original = owner.0.as_mut().unwrap();
            original.terminate();
            assert!(
                original.terminal(Duration::from_secs(5)),
                "exact original must acknowledge termination"
            );
            retain_unconfirmed(owner.0.take().unwrap(), true, &mut retained);
            assert!(retained.is_empty());
            assert_eq!(budget.available_permits(), 1);
            assert!(
                !path.exists(),
                "the child never reached OS lookup admission"
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn resolver_owned_child_timeout_cancel_oversize_and_nonzero_leave_no_live_process() {
        use windows_sys::Win32::System::Threading::*;
        for kind in ["timeout", "cancel", "shutdown", "oversized", "nonzero"] {
            let path = std::env::temp_dir().join(format!(
                "qc-resolver-fixture-{}-{}.pid",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let cleanup = FixtureCleanup(path.clone());
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            if kind == "shutdown" {
                let command = process_fixture(kind, &path);
                runtime.spawn(
                    async move { run(command, "example.org", Duration::from_secs(10)).await },
                );
                runtime.block_on(async {
                    for _ in 0..200 {
                        if path.exists() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    panic!("fixture never started");
                });
                drop(runtime);
            } else {
                runtime.block_on(async {
                    if kind == "cancel" {
                        let command = process_fixture(kind, &path);
                        let task = tokio::spawn(async move {
                            run(command, "example.org", Duration::from_secs(10)).await
                        });
                        for _ in 0..200 {
                            if path.exists() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        assert!(path.exists(), "fixture never started");
                        task.abort();
                        assert!(task.await.unwrap_err().is_cancelled());
                        // Awaiting cancellation drops its child guard before runtime teardown.
                    } else {
                        assert!(run(
                            process_fixture(kind, &path),
                            "example.org",
                            Duration::from_secs(2)
                        )
                        .await
                        .is_err());
                    }
                });
                drop(runtime);
            }
            assert!(path.exists(), "fixture executed after stdin admission");
            unsafe {
                if let Some(handle) = cleanup.handle() {
                    assert_eq!(
                        WaitForSingleObject(handle.0, 1000),
                        windows_sys::Win32::Foundation::WAIT_OBJECT_0,
                        "{kind} retained a live owned child"
                    );
                }
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn helper_process_accepts_complete_success_answer_but_rejects_same_answer_with_failed_exit() {
        // A no-network single-process fixture using .NET builtins only; never
        // used by production, which re-executes only the packaged broker.
        let shell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for exit in [0, 2] {
            let mut command = tokio::process::Command::new(&shell);
            command.args(["-NoProfile", "-NonInteractive", "-Command", &format!("$null=[Console]::In.ReadToEnd(); [Console]::Out.Write('{{\"version\":1,\"addresses\":[\"8.8.8.8\",\"::1\"]}}'); exit {exit}")]);
            let result = runtime.block_on(run(command, "example.org", Duration::from_secs(5)));
            if exit == 0 {
                assert_eq!(
                    result.unwrap(),
                    vec!["8.8.8.8:0".parse().unwrap(), "[::1]:0".parse().unwrap()]
                );
            } else {
                assert!(result.is_err());
            }
        }
    }
}
