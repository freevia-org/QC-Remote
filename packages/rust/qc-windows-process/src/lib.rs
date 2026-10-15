//! Windows process-liveness checks shared by the two native hosts that each
//! need OS process facts: the Windows Tauri app checks a published broker,
//! its parent, and the owner of a loopback listener; the native device broker
//! checks its `--stdio` parent's liveness and creation time. The Windows
//! process-table walk is centralized here so both hosts use the same snapshot
//! and cleanup rules.

#[cfg(windows)]
pub mod owned_tool;

/// Whether the original native thread has completely exited.
///
/// On Windows Rust's body-completion flag can precede native TLS cleanup.
/// Only the retained thread HANDLE's signal permits a nonblocking join;
/// failed or unexpected waits conservatively leave its owner retained.
pub fn thread_signalled<T>(handle: &std::thread::JoinHandle<T>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: JoinHandle retains this original thread handle for the call.
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                handle.as_raw_handle() as _,
                0,
            ) == windows_sys::Win32::Foundation::WAIT_OBJECT_0
        }
    }
    #[cfg(not(windows))]
    {
        handle.is_finished()
    }
}

/// Whether a process with this id currently exists, asked of the OS.
///
/// Errs towards "alive": every caller uses a `false` to justify treating the
/// process as gone (replacing a broker, exiting an orphan), so an answer the
/// OS will not give (access denied, or any unexpected error) must never read
/// as dead.
#[cfg(windows)]
pub fn os_process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    // SAFETY: OpenProcess takes no pointers; the handle is checked and closed.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        // ERROR_INVALID_PARAMETER is how OpenProcess reports "no such
        // process". Anything else (typically access denied) means there is a
        // process there that this one may not open.
        // SAFETY: no preconditions.
        return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
    }
    // A process that has exited but whose handle someone still holds can be
    // opened; its handle is signalled. A running process's is not.
    // SAFETY: `process` is a valid handle returned by OpenProcess.
    let running = process_wait_means_alive(unsafe { WaitForSingleObject(process, 0) });
    // SAFETY: `process` is a valid handle returned by OpenProcess.
    unsafe { CloseHandle(process) };
    running
}

#[cfg(windows)]
fn process_wait_means_alive(result: u32) -> bool {
    // Only a signalled process handle proves exit. Failure or an unexpected
    // wait state must not authorize replacing a live owner or exiting its child.
    result != windows_sys::Win32::Foundation::WAIT_OBJECT_0
}

#[cfg(not(windows))]
pub fn os_process_is_alive(_pid: u32) -> bool {
    true
}

/// The process's creation time as the raw 64-bit `FILETIME` from
/// `GetProcessTimes` (100 ns ticks since 1601 UTC).
///
/// `None` when it cannot be read for any reason, including the process
/// having exited in the meantime; callers treat that as "unknown", never as
/// "gone" on its own.
#[cfg(windows)]
pub fn os_process_created_at(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: OpenProcess takes no pointers; the handle is checked and closed.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `process` is a valid handle and every FILETIME pointer refers to
    // an initialized, writable local.
    let read =
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } != 0;
    // SAFETY: `process` is a valid handle returned by OpenProcess.
    unsafe { CloseHandle(process) };
    read.then(|| (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

#[cfg(not(windows))]
pub fn os_process_created_at(_pid: u32) -> Option<u64> {
    None
}

/// The current process's parent id, or `None` when the process table cannot
/// be read or no entry for the current process is present.
#[cfg(windows)]
pub fn os_parent_process_id() -> Option<u32> {
    let current = std::process::id();
    process_entries()?
        .into_iter()
        .find(|entry| entry.pid == current)
        .map(|entry| entry.parent_pid)
}

#[cfg(not(windows))]
pub fn os_parent_process_id() -> Option<u32> {
    None
}

/// Which process owns the listening socket a connection to
/// `127.0.0.1:port` reaches, asked of the OS's TCP table.
///
/// The most specific listener wins, as it does for the connection itself:
/// one bound to `127.0.0.1`, then one bound to every IPv4 address, then a
/// dual-stack one bound to every IPv6 address. `Ok(None)` when nothing
/// listens there; `Err` when the table cannot be read at all, which callers
/// must not read as either answer.
#[cfg(windows)]
pub fn loopback_listener_owner(port: u16) -> Result<Option<u32>, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    };
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    let wire_port = |raw: u32| u16::from_be((raw & 0xffff) as u16);
    let mut best: Option<(u8, u32)> = None;
    let mut consider = |rank: u8, pid: u32| {
        if best.is_none_or(|(current, _)| rank < current) {
            best = Some((rank, pid));
        }
    };
    for row in listener_rows::<MIB_TCPROW_OWNER_PID>(AF_INET)? {
        if wire_port(row.dwLocalPort) != port {
            continue;
        }
        match row.dwLocalAddr.to_ne_bytes() {
            [127, 0, 0, 1] => consider(0, row.dwOwningPid),
            [0, 0, 0, 0] => consider(1, row.dwOwningPid),
            _ => {}
        }
    }
    for row in listener_rows::<MIB_TCP6ROW_OWNER_PID>(AF_INET6)? {
        if wire_port(row.dwLocalPort) == port && row.ucLocalAddr == [0; 16] {
            consider(2, row.dwOwningPid);
        }
    }
    Ok(best.map(|(_, pid)| pid))
}

/// Every listening row of one address family's owner-PID TCP table.
#[cfg(windows)]
fn listener_rows<Row: Copy>(family: u32) -> Result<Vec<Row>, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    // u64 cells keep the buffer aligned for the table's u32 fields.
    let mut buffer: Vec<u64> = Vec::new();
    let mut size = 0_u32;
    for _ in 0..8 {
        let pointer = if buffer.is_empty() {
            std::ptr::null_mut()
        } else {
            buffer.as_mut_ptr().cast()
        };
        // SAFETY: `pointer` is null with `size` 0, or a writable buffer of at
        // least `size` bytes; the API writes at most `size` bytes and reports
        // the size it needs through `size`.
        let status = unsafe {
            GetExtendedTcpTable(
                pointer,
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match status {
            0 if !buffer.is_empty() => {
                let bytes = buffer.len() * std::mem::size_of::<u64>();
                // SAFETY: the table starts with a u32 entry count, and the
                // buffer holds at least that.
                let count = unsafe { *(buffer.as_ptr() as *const u32) } as usize;
                // Rows start after the count, at the row type's alignment.
                let offset = std::mem::align_of::<Row>().max(std::mem::size_of::<u32>());
                if offset + count * std::mem::size_of::<Row>() > bytes {
                    return Err("The TCP table is shorter than its entry count".into());
                }
                // SAFETY: bounds were checked above; the buffer is aligned for
                // u64 and therefore for the u32-only row types.
                let rows = unsafe {
                    std::slice::from_raw_parts(
                        (buffer.as_ptr() as *const u8).add(offset) as *const Row,
                        count,
                    )
                };
                return Ok(rows.to_vec());
            }
            0 => return Ok(Vec::new()),
            ERROR_INSUFFICIENT_BUFFER => {
                // Grow with headroom: listeners can appear between calls.
                buffer = vec![0_u64; (size as usize + 1024).div_ceil(8)];
            }
            other => return Err(format!("GetExtendedTcpTable failed with {other}")),
        }
    }
    Err("The TCP table kept growing while it was read".into())
}

#[cfg(not(windows))]
pub fn loopback_listener_owner(_port: u16) -> Result<Option<u32>, String> {
    Err("Listener ownership is only read on Windows".into())
}

/// A running process found by [`processes_by_image`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    /// Raw `FILETIME` creation time; `None` when the process cannot be opened.
    pub created_at: Option<u64>,
    /// The command line, or `None` when it cannot be read (another account's
    /// or an elevated process, or one that exited meanwhile). Callers must
    /// treat `None` as "unknown", never as a match.
    pub command_line: Option<String>,
}

#[cfg(windows)]
struct ProcessEntry {
    pid: u32,
    parent_pid: u32,
    image_name: String,
}

#[cfg(windows)]
fn process_entries() -> Option<Vec<ProcessEntry>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: plain snapshot call; the handle is checked and closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: an all-zero PROCESSENTRY32W is a valid initial value.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = Vec::new();
    // SAFETY: `snapshot` is valid and `entry` is a correctly sized, writable local.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        found.push(ProcessEntry {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            image_name: String::from_utf16_lossy(&entry.szExeFile[..length]),
        });
        // SAFETY: `snapshot` and `entry` remain valid for this iteration.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: `snapshot` is a valid handle returned above.
    unsafe { CloseHandle(snapshot) };
    Some(found)
}

/// Every running process whose executable file name equals `image_name`
/// (case-insensitive), with creation time and command line where readable.
/// An empty list when the process table cannot be read.
#[cfg(windows)]
pub fn processes_by_image(image_name: &str) -> Vec<ProcessInfo> {
    process_entries()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.image_name.eq_ignore_ascii_case(image_name))
        .map(|entry| ProcessInfo {
            pid: entry.pid,
            created_at: os_process_created_at(entry.pid),
            command_line: process_command_line(entry.pid),
        })
        .collect()
}

#[cfg(not(windows))]
pub fn processes_by_image(_image_name: &str) -> Vec<ProcessInfo> {
    Vec::new()
}

/// Validate untrusted UTF-16 byte lengths before reserving the read buffer.
#[cfg(any(windows, test))]
fn validated_command_line_length(length: u16, maximum_length: u16) -> Option<usize> {
    (length != 0 && length.is_multiple_of(2) && length <= 32_768 && length <= maximum_length)
        .then_some(usize::from(length))
}

/// Reads a process's command line from its PEB. Same-bitness only; any
/// failure yields `None`.
#[cfg(windows)]
fn process_command_line(pid: u32) -> Option<String> {
    use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
    use windows_sys::Win32::Foundation::{CloseHandle, UNICODE_STRING};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };
    #[repr(C)]
    struct BasicInformation {
        exit_status: i32,
        peb: usize,
        affinity: usize,
        priority: i32,
        pid: usize,
        parent: usize,
    }
    const POINTER: usize = std::mem::size_of::<usize>();
    // PEB.ProcessParameters follows four pointer-sized slots on both widths.
    const PARAMETERS_AT: usize = 4 * POINTER;
    // RTL_USER_PROCESS_PARAMETERS.CommandLine follows 16 bytes, ten pointers
    // and the ImagePathName UNICODE_STRING.
    const COMMAND_LINE_AT: usize = 16 + 10 * POINTER + std::mem::size_of::<UNICODE_STRING>();

    // SAFETY: OpenProcess takes no pointers; the handle is checked and closed.
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if process.is_null() {
        return None;
    }
    let read = |address: usize, buffer: *mut u8, bytes: usize| -> bool {
        let mut done = 0_usize;
        // SAFETY: `buffer` points at `bytes` writable bytes owned by the caller.
        let ok = unsafe {
            ReadProcessMemory(
                process,
                address as *const _,
                buffer.cast(),
                bytes,
                &mut done,
            )
        } != 0;
        ok && done == bytes
    };
    let result = (|| {
        // SAFETY: zeroed plain-old-data.
        let mut basic: BasicInformation = unsafe { std::mem::zeroed() };
        let mut returned = 0_u32;
        // SAFETY: `basic` is writable and its size is passed.
        let status = unsafe {
            NtQueryInformationProcess(
                process,
                0,
                (&raw mut basic).cast(),
                std::mem::size_of::<BasicInformation>() as u32,
                &mut returned,
            )
        };
        if status != 0 || basic.peb == 0 {
            return None;
        }
        let mut parameters = 0_usize;
        if !read(
            basic.peb.checked_add(PARAMETERS_AT)?,
            (&raw mut parameters).cast(),
            POINTER,
        ) || parameters == 0
        {
            return None;
        }
        // SAFETY: zeroed plain-old-data.
        let mut text: UNICODE_STRING = unsafe { std::mem::zeroed() };
        if !read(
            parameters.checked_add(COMMAND_LINE_AT)?,
            (&raw mut text).cast(),
            std::mem::size_of::<UNICODE_STRING>(),
        ) || text.Buffer.is_null()
        {
            return None;
        }
        // The remote UNICODE_STRING is untrusted. An odd byte length would
        // otherwise allocate floor(length / 2) units but write length bytes.
        let byte_length = validated_command_line_length(text.Length, text.MaximumLength)?;
        let mut units = vec![0_u16; byte_length / 2];
        if !read(text.Buffer as usize, units.as_mut_ptr().cast(), byte_length) {
            return None;
        }
        Some(String::from_utf16_lossy(&units))
    })();
    // SAFETY: valid handle from OpenProcess.
    unsafe { CloseHandle(process) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    static FIXTURE_CHILD_COUNT: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    #[cfg(windows)]
    struct FixtureChildPermit;
    #[cfg(windows)]
    impl Drop for FixtureChildPermit {
        fn drop(&mut self) {
            FIXTURE_CHILD_COUNT.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
    #[cfg(windows)]
    struct RetainedFixtureChild {
        child: std::process::Child,
        _permit: FixtureChildPermit,
    }
    #[cfg(windows)]
    fn retained_fixture_children() -> &'static std::sync::Mutex<Vec<RetainedFixtureChild>> {
        static RETAINED: std::sync::Mutex<Vec<RetainedFixtureChild>> =
            std::sync::Mutex::new(Vec::new());
        &RETAINED
    }
    #[cfg(windows)]
    fn reap_fixture_children() {
        let records = std::mem::take(
            &mut *retained_fixture_children()
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        );
        let pending = records
            .into_iter()
            .filter_map(|mut record| {
                if matches!(record.child.try_wait(), Ok(Some(_))) {
                    None // Exact original acknowledges exit before admission drops.
                } else {
                    Some(record)
                }
            })
            .collect::<Vec<_>>();
        retained_fixture_children()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(pending);
    }
    #[cfg(windows)]
    struct FixtureChild {
        child: Option<std::process::Child>,
        permit: Option<FixtureChildPermit>,
    }
    #[cfg(windows)]
    impl FixtureChild {
        fn spawn(mode: &str) -> Self {
            reap_fixture_children();
            FIXTURE_CHILD_COUNT
                .fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |count| (count < 4).then_some(count + 1),
                )
                .expect("bounded process-fixture admission");
            let permit = FixtureChildPermit;
            let child =
                std::process::Command::new(std::env::current_exe().expect("test executable path"))
                    .args(["--exact", "tests::process_fixture_child", "--ignored"])
                    .env("QC_WINDOWS_PROCESS_FIXTURE_CHILD", mode)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("controlled test executable spawns");
            Self {
                child: Some(child),
                permit: Some(permit),
            }
        }
        fn pid(&self) -> u32 {
            self.child.as_ref().unwrap().id()
        }
        fn wait_until(
            &mut self,
            deadline: std::time::Instant,
        ) -> std::io::Result<std::process::ExitStatus> {
            loop {
                if let Some(status) = self.child.as_mut().unwrap().try_wait()? {
                    return Ok(status);
                }
                if std::time::Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "process fixture did not exit",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    #[cfg(windows)]
    impl Drop for FixtureChild {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else {
                return;
            };
            let permit = self.permit.take().unwrap();
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            let _ = child.kill();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            // Never drop an unacknowledged original merely to regain capacity.
            retained_fixture_children()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(RetainedFixtureChild {
                    child,
                    _permit: permit,
                });
            eprintln!(
                "process fixture cleanup unacknowledged; exact original and admission retained"
            );
        }
    }

    #[test]
    fn remote_command_line_lengths_never_exceed_the_utf16_allocation() {
        let mut visited = 0_usize;
        let mut accepted = 0_usize;
        for length in 0..=u16::MAX {
            visited += 1;
            if let Some(bytes) = validated_command_line_length(length, u16::MAX) {
                accepted += 1;
                assert_eq!((bytes / 2) * std::mem::size_of::<u16>(), bytes);
                assert!(bytes > 0 && bytes <= 32_768);
            } else {
                assert!(length == 0 || !length.is_multiple_of(2) || length > 32_768);
            }
        }
        assert_eq!(visited, 65_536, "every u16 length was checked");
        assert_eq!(
            accepted, 16_384,
            "every nonzero even supported length passed"
        );
        assert_eq!(validated_command_line_length(12, 10), None);
        assert_eq!(validated_command_line_length(12, 12), Some(12));
        assert_eq!(validated_command_line_length(32_768, 32_768), Some(32_768));
    }

    #[cfg(windows)]
    #[test]
    fn only_a_signalled_process_wait_proves_exit() {
        use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        assert!(!process_wait_means_alive(WAIT_OBJECT_0));
        assert!(process_wait_means_alive(WAIT_TIMEOUT));
        assert!(process_wait_means_alive(WAIT_FAILED));
        assert!(
            process_wait_means_alive(0x1234),
            "unknown is not proof of exit"
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_owner_of_a_loopback_listener_is_read_from_the_tcp_table() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            loopback_listener_owner(port).unwrap(),
            Some(std::process::id())
        );
        drop(listener);
        // A port nobody listens on has no owner (allow for an unrelated
        // process grabbing it in between: only our own pid is ruled out).
        assert_ne!(
            loopback_listener_owner(port).unwrap(),
            Some(std::process::id())
        );
    }

    #[test]
    fn the_current_process_is_alive() {
        assert!(os_process_is_alive(std::process::id()));
    }

    #[cfg(windows)]
    #[test]
    fn an_exited_process_is_not_read_as_alive() {
        let mut child = FixtureChild::spawn("exit");
        let pid = child.pid();
        assert!(child
            .wait_until(std::time::Instant::now() + std::time::Duration::from_secs(5))
            .expect("controlled child exits")
            .success());
        assert!(!os_process_is_alive(pid));
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn process_fixture_child() {
        match std::env::var("QC_WINDOWS_PROCESS_FIXTURE_CHILD").as_deref() {
            Ok("exit") => {}
            Ok("wait") => {
                use std::io::Read;
                // Only the explicitly filtered owned helper blocks here.
                let mut byte = [0_u8; 1];
                let _ = std::io::stdin().read(&mut byte);
            }
            _ => panic!("controlled process fixture requires an explicit child mode"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn process_fixture_unwind_reaps_its_original_before_releasing_admission() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{
            DuplicateHandle, DUPLICATE_SAME_ACCESS, WAIT_OBJECT_0,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, WaitForSingleObject};
        let mut process = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let child = FixtureChild::spawn("wait");
            let mut duplicate = std::ptr::null_mut();
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        child.child.as_ref().unwrap().as_raw_handle(),
                        GetCurrentProcess(),
                        &mut duplicate,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            process = Some(unsafe { OwnedHandle::from_raw_handle(duplicate) });
            panic!("injected assertion before child observation");
        }));
        assert!(outcome.is_err());
        assert_eq!(
            unsafe { WaitForSingleObject(process.unwrap().as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
    }

    #[cfg(windows)]
    #[test]
    fn this_process_creation_time_is_readable_and_stable() {
        let pid = std::process::id();
        let first = os_process_created_at(pid).expect("creation time reads");
        let second = os_process_created_at(pid).expect("creation time reads again");
        assert_eq!(
            first, second,
            "a process's own creation time does not change"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_both_checks_are_conservative_stubs() {
        assert!(os_process_is_alive(std::process::id()));
        assert_eq!(os_process_created_at(std::process::id()), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_scan_finds_this_process_with_its_command_line() {
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().to_string();
        let mine = processes_by_image(&name.to_uppercase())
            .into_iter()
            .find(|process| process.pid == std::process::id())
            .expect("this process is listed, case-insensitively");
        assert_eq!(mine.created_at, os_process_created_at(std::process::id()));
        let expected = std::env::args().next().unwrap();
        let line = mine.command_line.expect("own command line reads");
        assert!(
            line.contains(expected.trim_matches('"'))
                || line.to_lowercase().contains(&name.to_lowercase()),
            "{line}"
        );
    }

    #[test]
    fn an_absent_image_lists_nothing() {
        assert!(processes_by_image("qc-no-such-image-0000.exe").is_empty());
    }
}
