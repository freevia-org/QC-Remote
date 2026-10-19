//! Private recovery prototype. WMI runs only in the exact gated child mode.
//! Ordinary liveness checks do not invoke this module or start a helper.
use std::{
    io::{self, Read, Write},
    path::Path,
    process::Command,
    time::Duration,
};
use windows::core::{w, BSTR};
use windows::Win32::System::{Com::*, Variant::*, Wmi::*};

pub const HELPER_FLAG: &str = "--process-creation-only";
pub(crate) const DEADLINE_ENV: &str = "QC_CREATION_ADMISSION_DEADLINE_MS";
const ADMISSION_PREFIX: &str = "QC_CREATION_V1 ";
const LIMIT: u64 = 128;

/// Exact early executable dispatch. No mixed mode may fall through into an
/// app, broker, MCP bridge or HID initialization.
pub fn dispatch_helper_mode() -> bool {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(exact) = helper_request(&args) else {
        return false;
    };
    if !exact || serve(std::io::stdout().lock()).is_err() {
        std::process::exit(2);
    }
    true
}

fn helper_request(args: &[std::ffi::OsString]) -> Option<bool> {
    args.iter()
        .any(|arg| arg == HELPER_FLAG)
        .then_some(args.len() == 1)
}

/// GetTickCount64 is boot-monotonic and shared by parent/child, unlike a fresh
/// per-process Instant. Its coarse timer resolution is not a real-time promise.
pub(crate) fn tick_now() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

pub(crate) fn admission_deadline() -> io::Result<u64> {
    let value =
        std::env::var(DEADLINE_ENV).map_err(|_| io::Error::other("missing admission expiry"))?;
    let deadline: u64 = value
        .parse()
        .map_err(|_| io::Error::other("invalid admission expiry"))?;
    let remaining = deadline
        .checked_sub(tick_now())
        .filter(|&ms| ms > 0 && ms <= 10_000)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "admission expiry outside bounded window",
            )
        })?;
    let _ = remaining;
    Ok(deadline)
}

/// Sole reader of an anonymous stdin pipe. Peek before ReadFile so a writer
/// holding stdin open cannot suspend this helper past the original expiry.
/// This executes even before job assignment; no COM or provider code is loaded.
pub(crate) fn read_admission_pipe(deadline: u64) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::{
        Foundation::{
            GetLastError, ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{GetFileType, ReadFile, FILE_TYPE_PIPE},
        System::{
            Console::{GetStdHandle, STD_INPUT_HANDLE},
            Pipes::PeekNamedPipe,
        },
    };
    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null()
        || handle == INVALID_HANDLE_VALUE
        || unsafe { GetFileType(handle) } != FILE_TYPE_PIPE
    {
        return Err(io::Error::other("admission requires pipe stdin"));
    }
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_sub(tick_now())
            .filter(|&ms| ms > 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "admission pipe expired"))?;
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            let error = unsafe { GetLastError() };
            if error == ERROR_BROKEN_PIPE || error == ERROR_PIPE_NOT_CONNECTED {
                return Ok(bytes);
            }
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        if available == 0 {
            std::thread::sleep(Duration::from_millis(remaining.min(2)));
            continue;
        }
        if bytes
            .len()
            .checked_add(available as usize)
            .is_none_or(|n| n > 64)
        {
            return Err(io::Error::other("admission pipe overflow"));
        }
        let mut buffer = [0u8; 64];
        let mut read = 0;
        // Only this reader consumes stdin, so a read <= peeked availability
        // cannot wait for additional writer bytes. The handle is borrowed.
        if unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                available.min(64),
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..read as usize]);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assessment {
    Same,
    GoneOrDifferent,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CreationInterval {
    lower: u64,
    upper: u64,
}

/// Explicit startup/recovery admission only. The caller must invoke at most once
/// for its observed descriptor generation and revalidate it under election before
/// retiring anything. This function itself neither reads nor changes owner files.
pub fn assess_at_recovery_admission(helper: &Path, pid: u32, recorded: Option<u64>) -> Assessment {
    if pid == 0 {
        return Assessment::Unknown;
    }
    if !crate::os_process_is_alive(pid) {
        return Assessment::GoneOrDifferent;
    }
    let Some(recorded) = recorded else {
        return Assessment::Unknown;
    };
    if let Some(current) = crate::os_process_created_at(pid) {
        return if current == recorded {
            Assessment::Same
        } else {
            Assessment::GoneOrDifferent
        };
    }
    let mut command = Command::new(helper);
    command.arg(HELPER_FLAG).stderr(std::process::Stdio::null());
    let admission = format!("{ADMISSION_PREFIX}{pid}\n");
    let Ok(output) = super::owned_tool::run_recovery_helper(
        &mut command,
        admission.as_bytes(),
        Duration::from_secs(2),
    ) else {
        return Assessment::Unknown;
    };
    if !output.status.success() {
        return Assessment::Unknown;
    }
    decode(&output.stdout, pid).map_or(Assessment::Unknown, |interval| compare(recorded, interval))
}

fn compare(recorded: u64, interval: CreationInterval) -> Assessment {
    if recorded < interval.lower || recorded >= interval.upper {
        Assessment::GoneOrDifferent
    } else {
        Assessment::Unknown
    }
}

fn admission_pid(bytes: &[u8]) -> Option<u32> {
    if bytes.len() > 64 {
        return None;
    }
    let value = std::str::from_utf8(bytes)
        .ok()?
        .strip_prefix(ADMISSION_PREFIX)?
        .strip_suffix('\n')?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid = value.parse().ok()?;
    (pid != 0).then_some(pid)
}

fn decode(bytes: &[u8], expected_pid: u32) -> Option<CreationInterval> {
    if bytes.len() as u64 > LIMIT {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
    let fields: Vec<_> = text.split(' ').collect();
    if fields.len() != 4 || fields[0] != "1" || fields[1].parse::<u32>().ok()? != expected_pid {
        return None;
    }
    if !fields[1..]
        .iter()
        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let interval = CreationInterval {
        lower: fields[2].parse().ok()?,
        upper: fields[3].parse().ok()?,
    };
    (interval.lower > 0 && interval.upper.checked_sub(interval.lower)? == 20).then_some(interval)
}

/// Called before broker initialization. EOF without a complete valid admission
/// exits without initializing COM, querying WMI, loading state or opening HID.
pub fn serve(writer: impl Write) -> io::Result<()> {
    let deadline = admission_deadline()?;
    let bytes = read_admission_pipe(deadline)?;
    serve_with(
        bytes.as_slice(),
        writer,
        |pid| query_creation(pid, deadline),
        deadline,
    )
}

fn serve_with(
    mut reader: impl Read,
    mut writer: impl Write,
    query: impl FnOnce(u32) -> Option<CreationInterval>,
    deadline: u64,
) -> io::Result<()> {
    let mut bytes = Vec::new();
    reader.by_ref().take(65).read_to_end(&mut bytes)?;
    let pid =
        admission_pid(&bytes).ok_or_else(|| io::Error::other("invalid creation admission"))?;
    if tick_now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "creation admission expired",
        ));
    }
    let interval = query(pid).ok_or_else(|| io::Error::other("creation identity unavailable"))?;
    if tick_now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "late creation result",
        ));
    }
    writeln!(writer, "1 {pid} {} {}", interval.lower, interval.upper)
}

struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: this guard is created only after successful initialization on
        // this same helper thread; all COM interface locals drop before it.
        unsafe {
            CoUninitialize();
        }
    }
}

fn query_creation(pid: u32, deadline: u64) -> Option<CreationInterval> {
    macro_rules! checked {
        ($stage:literal, $result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    #[cfg(test)]
                    eprintln!("WMI stage={} HRESULT={:?}", $stage, error);
                    let _ = error;
                    return None;
                }
            }
        };
    }
    // SAFETY: all calls use generated Microsoft bindings. The isolated helper
    // initializes its own COM apartment; a parent-owned job bounds provider hangs.
    unsafe {
        if tick_now() >= deadline {
            return None;
        }
        checked!(
            "initialize",
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()
        );
        let _apartment = ComApartment;
        if tick_now() >= deadline {
            return None;
        }
        checked!(
            "security",
            CoInitializeSecurity(
                None,
                -1,
                None,
                None,
                RPC_C_AUTHN_LEVEL_DEFAULT,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
                None,
            )
        );
        if tick_now() >= deadline {
            return None;
        }
        let locator: IWbemLocator = checked!(
            "locator",
            CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)
        );
        let empty = BSTR::new();
        if tick_now() >= deadline {
            return None;
        }
        let services = checked!(
            "connect",
            locator.ConnectServer(
                &BSTR::from("ROOT\\CIMV2"),
                &empty,
                &empty,
                &empty,
                0,
                &empty,
                None,
            )
        );
        if tick_now() >= deadline {
            return None;
        }
        checked!(
            "proxy",
            CoSetProxyBlanket(
                &services,
                10,
                0,
                None,
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )
        );
        let query = BSTR::from(format!(
            "SELECT ProcessId, CreationDate FROM Win32_Process WHERE ProcessId = {pid}"
        ));
        if tick_now() >= deadline {
            return None;
        }
        let rows = checked!(
            "query",
            services.ExecQuery(
                &BSTR::from("WQL"),
                &query,
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )
        );
        let mut first = [None];
        let mut count = 0;
        let remaining = deadline.checked_sub(tick_now())?.min(i32::MAX as u64);
        if remaining == 0 {
            return None;
        }
        let status = rows.Next(remaining as i32, &mut first, &mut count);
        if status != windows::core::HRESULT(0) || count != 1 {
            #[cfg(test)]
            eprintln!("WMI first enumeration status={status:?} count={count}");
            return None;
        }
        let row = first[0].take()?;
        if tick_now() >= deadline {
            return None;
        }
        let found_pid = property_text(&row, w!("ProcessId"), 19).or_else(|| {
            #[cfg(test)]
            eprintln!("WMI ProcessId unavailable");
            None
        })?;
        if found_pid.parse::<u32>().ok()? != pid {
            return None;
        }
        if tick_now() >= deadline {
            return None;
        }
        let creation = property_text(&row, w!("CreationDate"), 101).or_else(|| {
            #[cfg(test)]
            eprintln!("WMI CreationDate unavailable");
            None
        })?;
        let mut extra = [None];
        count = 0;
        // Only an explicit end-of-enumeration, not timeout or partial response,
        // establishes that this selected PID produced exactly one record.
        let remaining = deadline.checked_sub(tick_now())?.min(i32::MAX as u64);
        if remaining == 0 {
            return None;
        }
        let status = rows.Next(remaining as i32, &mut extra, &mut count);
        if status != windows::core::HRESULT(WBEM_S_FALSE.0) || count != 0 {
            #[cfg(test)]
            eprintln!("WMI final enumeration status={status:?} count={count}");
            return None;
        }
        if tick_now() >= deadline {
            return None;
        }
        parse_creation(&creation).or_else(|| {
            #[cfg(test)]
            eprintln!("WMI date rejected length={}", creation.len());
            None
        })
    }
}

unsafe fn property_text(
    row: &IWbemClassObject,
    name: windows::core::PCWSTR,
    expected_type: i32,
) -> Option<String> {
    let mut value = VARIANT::default();
    struct Value(VARIANT);
    impl Drop for Value {
        fn drop(&mut self) {
            unsafe {
                let _ = VariantClear(&mut self.0);
            }
        }
    }
    let mut kind = 0;
    // SAFETY: row is a generated owned COM interface; initialized VARIANT lives
    // through Get and is always cleared, including a partially failed call.
    let result = unsafe { row.Get(name, 0, &mut value, Some(&mut kind), None) };
    let value = Value(value);
    result.ok()?;
    if kind != expected_type {
        #[cfg(test)]
        eprintln!("WMI property type={kind} expected={expected_type}");
        return None;
    }
    // Null/empty variants must not become fabricated dates or PID zero.
    let vt = unsafe { value.0.Anonymous.Anonymous.vt };
    if (expected_type == 101 && vt != VT_BSTR)
        || (expected_type == 19 && vt != VT_I4 && vt != VT_UI4)
    {
        #[cfg(test)]
        eprintln!("WMI variant type={vt:?} expected CIM={expected_type}");
        return None;
    }
    let mut buffer = [0u16; 64];
    unsafe { VariantToString(&value.0, &mut buffer) }.ok()?;
    let end = buffer.iter().position(|&unit| unit == 0)?;
    String::from_utf16(&buffer[..end]).ok()
}

fn parse_creation(text: &str) -> Option<CreationInterval> {
    let b = text.as_bytes();
    if b.len() != 25 || b[14] != b'.' || !matches!(b[21], b'+' | b'-') {
        return None;
    }
    if !(0..25)
        .filter(|&i| i != 14 && i != 21)
        .all(|i| b[i].is_ascii_digit())
    {
        return None;
    }
    let number = |a, z| text.get(a..z)?.parse::<u16>().ok();
    let year = number(0, 4)?;
    let month = number(4, 6)?;
    let day = number(6, 8)?;
    let hour = number(8, 10)?;
    let minute = number(10, 12)?;
    let second = number(12, 14)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    if year < 1601 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let system = windows_sys::Win32::Foundation::SYSTEMTIME {
        wYear: year,
        wMonth: month,
        wDay: day,
        wHour: hour,
        wMinute: minute,
        wSecond: second,
        wDayOfWeek: 0,
        wMilliseconds: 0,
    };
    let mut file = windows_sys::Win32::Foundation::FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: initialized input/output of the documented Windows calendar API.
    if unsafe { windows_sys::Win32::System::Time::SystemTimeToFileTime(&system, &mut file) } == 0 {
        return None;
    }
    let base = (u64::from(file.dwHighDateTime) << 32) | u64::from(file.dwLowDateTime);
    let micros = text[15..21].parse::<u64>().ok()?;
    let local = base.checked_add(micros.checked_mul(10)?)?;
    let offset = u64::from(number(22, 25)?).checked_mul(60 * 10_000_000)?;
    let utc = if b[21] == b'+' {
        local.checked_sub(offset)?
    } else {
        local.checked_add(offset)?
    };
    // A conservative +/-1 microsecond envelope accommodates formatting at CIM
    // microsecond precision. An overlap never authenticates or evicts an owner.
    Some(CreationInterval {
        lower: utc.checked_sub(10)?,
        upper: utc.checked_add(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_mode_mixed_with_app_or_broker_flags_never_falls_through() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(helper_request(&args(&[HELPER_FLAG])), Some(true));
        assert_eq!(
            helper_request(&args(&[HELPER_FLAG, "--local-mcp-bridge"])),
            Some(false)
        );
        assert_eq!(
            helper_request(&args(&["--broker-serve", HELPER_FLAG])),
            Some(false)
        );
        assert_eq!(helper_request(&args(&["--resident"])), None);
    }
    #[test]
    fn invalid_or_absent_admission_never_calls_provider() {
        for bytes in [
            b"".as_slice(),
            b"QC_CREATION_V1 0\n",
            b"QC_CREATION_V1 1\nextra",
            b"QC_CREATION_V1 -1\n",
        ] {
            assert!(serve_with(
                bytes,
                Vec::new(),
                |_| panic!("provider before valid admission"),
                tick_now() + 1000
            )
            .is_err());
        }
    }
    #[test]
    fn validated_admission_queries_once_and_requires_matching_bounded_output() {
        let interval = CreationInterval {
            lower: 100,
            upper: 120,
        };
        let mut bytes = Vec::new();
        serve_with(
            b"QC_CREATION_V1 7\n".as_slice(),
            &mut bytes,
            |pid| {
                assert_eq!(pid, 7);
                Some(interval)
            },
            tick_now() + 1000,
        )
        .unwrap();
        assert_eq!(decode(&bytes, 7), Some(interval));
        assert_eq!(decode(&bytes, 8), None);
        bytes.extend_from_slice(b"1 7 100 120\n");
        assert_eq!(decode(&bytes, 7), None);
    }
    #[test]
    fn interval_overlap_stays_unknown_and_only_disjoint_ticks_prove_difference() {
        let interval = CreationInterval {
            lower: 100,
            upper: 120,
        };
        for tick in [100, 109, 119] {
            assert_eq!(compare(tick, interval), Assessment::Unknown);
        }
        for tick in [99, 120] {
            assert_eq!(compare(tick, interval), Assessment::GoneOrDifferent);
        }
    }
    #[test]
    fn expired_original_deadline_never_enters_provider() {
        assert!(serve_with(
            b"QC_CREATION_V1 7\n".as_slice(),
            Vec::new(),
            |_| panic!("expired provider"),
            tick_now()
        )
        .is_err());
        assert_eq!(query_creation(7, tick_now()), None);
    }
    #[test]
    fn late_provider_result_is_rejected_before_emitting_proof() {
        let mut output = Vec::new();
        let mut entered = false;
        let result = serve_with(
            b"QC_CREATION_V1 7\n".as_slice(),
            &mut output,
            |_| {
                entered = true;
                std::thread::sleep(Duration::from_millis(150));
                Some(CreationInterval {
                    lower: 100,
                    upper: 120,
                })
            },
            tick_now() + 100,
        );
        assert!(entered);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(output.is_empty());
    }
    #[test]
    fn cim_dates_validate_calendar_precision_and_timezone() {
        // Synthetic 2000-01-02 03:04:05 UTC, with a five-tick submicrosecond
        // residue. No process observation is embedded in this public fixture.
        let utc = parse_creation("20000102030405.123456+000").unwrap();
        assert_eq!(Some(utc), parse_creation("20000102050405.123456+120"));
        assert_eq!(utc.upper - utc.lower, 20);
        assert!(utc.lower <= 125912558451234565 && 125912558451234565 < utc.upper);
        for bad in [
            "20000230030405.123456+000",
            "20000102030460.123456+000",
            "20000102030405.******+000",
            "20000102030405.123456:000",
            "",
            "00000102030405.123456+000",
        ] {
            assert_eq!(parse_creation(bad), None, "{bad}");
        }
    }

    #[test]
    fn microsecond_residues_and_rounding_day_rollover_never_false_differ() {
        let before = parse_creation("20000102235959.999999+000").unwrap();
        let rounded = parse_creation("20000103000000.000000+000").unwrap();
        let tick = before.lower + 10;
        for residue in 0..10 {
            assert_eq!(compare(tick + residue, before), Assessment::Unknown);
        }
        // Rounding the last half-microsecond carries into the next day. The
        // original FILETIME remains inside either valid formatting envelope.
        for residue in 5..10 {
            assert_eq!(compare(tick + residue, rounded), Assessment::Unknown);
        }
        assert_eq!(parse_creation("20000103020000.000000+120"), Some(rounded));
    }

    #[test]
    #[ignore = "Explicit supervised local-provider validation only; no QC"]
    fn local_provider_creation_interval_contains_original_filetime() {
        let original =
            crate::os_process_created_at(std::process::id()).expect("original native identity");
        let start = std::time::Instant::now();
        let interval = query_creation(std::process::id(), tick_now() + 2000)
            .expect("local WMI provider creation date");
        assert_eq!(
            compare(original, interval),
            Assessment::Unknown,
            "WMI must not falsely distinguish the original process"
        );
        println!("provider elapsed_ms={}", start.elapsed().as_millis());
    }

    #[test]
    #[ignore = "Explicit supervised private-environment reproduction only; no QC"]
    fn private_protected_process_generation_is_disjoint() {
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_ACCESS_DENIED},
            System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        };
        // Values are supplied only by the private local runner, never fixtures.
        let pid = std::env::var("QC_TEST_CREATION_TARGET_PID")
            .expect("private target PID required")
            .parse::<u32>()
            .expect("valid private target PID");
        let recorded = std::env::var("QC_TEST_CREATION_RECORDED_FILETIME")
            .expect("private original identity required")
            .parse::<u64>()
            .expect("valid private original identity");
        assert!(pid != 0 && recorded != 0);
        let start = std::time::Instant::now();
        let deadline = tick_now() + 2000;
        assert!(crate::os_process_is_alive(pid));
        assert_eq!(
            crate::os_process_created_at(pid),
            None,
            "normal identity must be unavailable for this case"
        );
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if !process.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(process);
            }
            panic!("private target is no longer access-denied");
        }
        assert_eq!(unsafe { GetLastError() }, ERROR_ACCESS_DENIED);
        let interval = query_creation(pid, deadline).expect("independent WMI generation evidence");
        assert_eq!(
            compare(recorded, interval),
            Assessment::GoneOrDifferent,
            "independent interval must be disjoint; access denial alone is not evidence"
        );
        println!(
            "protected_provider elapsed_ms={}",
            start.elapsed().as_millis()
        );
    }
}
