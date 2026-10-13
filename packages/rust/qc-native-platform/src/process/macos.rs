//! macOS process liveness and start identity via libproc.
use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

fn process_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    if pid <= 0 {
        return None;
    }
    // SAFETY: proc_pidinfo fills this correctly-sized POD structure when it
    // returns exactly its size; a short or failed read is treated as unknown.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast::<c_void>(),
            std::mem::size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    (read == std::mem::size_of::<libc::proc_bsdinfo>() as i32).then_some(info)
}

/// Unknown permission and OS errors stay "alive"; callers may only reap when
/// the kernel proves the PID no longer exists.
pub fn os_process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    if let Some(info) = process_info(pid as u32) {
        return info.pbi_status != libc::SZOMB;
    }
    // SAFETY: kill with signal 0 only asks whether this positive PID exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Process start timestamp, stable for the life of the process and different
/// when the kernel reuses its PID.
pub fn os_process_created_at(pid: u32) -> Option<u64> {
    let info = process_info(pid)?;
    info.pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)
}

pub fn os_parent_process_id() -> Option<u32> {
    process_info(std::process::id())
        .map(|info| info.pbi_ppid)
        .filter(|pid| *pid != 0)
}

fn process_path(pid: u32) -> Option<Vec<u8>> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    let mut path = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: libproc writes at most the buffer capacity passed here.
    let length =
        unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast::<c_void>(), path.len() as u32) };
    (length > 0).then(|| {
        path.truncate(length as usize);
        path
    })
}

fn process_command_line(pid: u32) -> Option<String> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut length = 0_usize;
    // SAFETY: this first sysctl call asks only for the required buffer length.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || length < std::mem::size_of::<i32>()
    {
        return None;
    }
    let mut bytes = vec![0_u8; length];
    // SAFETY: `bytes` is writable for `length`, as requested above.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast::<c_void>(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || length > bytes.len()
    {
        return None;
    }
    bytes.truncate(length);
    let argc = i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?);
    if argc <= 0 {
        return None;
    }
    let mut arguments = bytes.get(4..)?;
    // The executable path is followed by NUL padding before argv begins.
    let executable_end = arguments.iter().position(|byte| *byte == 0)?;
    arguments = arguments.get(executable_end + 1..)?;
    while arguments.first() == Some(&0) {
        arguments = arguments.get(1..)?;
    }
    let mut values = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        let end = arguments.iter().position(|byte| *byte == 0)?;
        values.push(String::from_utf8_lossy(&arguments[..end]).into_owned());
        arguments = arguments.get(end + 1..)?;
    }
    Some(values.join(" "))
}

/// Processes owned by this user whose executable has the requested name.
pub fn processes_by_image(image_name: &str) -> Vec<qc_windows_process::ProcessInfo> {
    use std::os::unix::fs::MetadataExt;

    // PROC_ALL_PIDS asks libproc for the complete process list.
    // Only inspect this exact executable running as this user. macOS apps in
    // /Applications are commonly root-owned, so executable file ownership is
    // not a reliable proxy for the process owner. A same-named broker from a
    // different installation must not make this hardware lease appear owned.
    let Ok(own_executable) = std::env::current_exe() else {
        return Vec::new();
    };
    let Ok(own_metadata) = std::fs::metadata(&own_executable) else {
        return Vec::new();
    };
    // SAFETY: geteuid has no pointer arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: a null buffer with zero capacity is the documented size query.
    let required = unsafe { libc::proc_listpids(1, 0, std::ptr::null_mut(), 0) };
    if required <= 0 {
        return Vec::new();
    }
    let bytes = required as usize + std::mem::size_of::<libc::pid_t>() * 32;
    let mut pids = vec![0 as libc::pid_t; bytes / std::mem::size_of::<libc::pid_t>()];
    // SAFETY: libproc fills only the process-id buffer described by its size.
    let received =
        unsafe { libc::proc_listpids(1, 0, pids.as_mut_ptr().cast::<c_void>(), bytes as i32) };
    if received <= 0 {
        return Vec::new();
    }
    pids.truncate((received as usize / std::mem::size_of::<libc::pid_t>()).min(pids.len()));
    pids.into_iter()
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter_map(|pid| {
            let path = process_path(pid)?;
            let path = Path::new(std::ffi::OsStr::from_bytes(&path));
            if path.file_name()? != std::ffi::OsStr::new(image_name) {
                return None;
            }
            let metadata = std::fs::metadata(path).ok()?;
            if (metadata.dev(), metadata.ino()) != (own_metadata.dev(), own_metadata.ino())
                || process_info(pid)?.pbi_uid != uid
            {
                return None;
            }
            Some(qc_windows_process::ProcessInfo {
                pid,
                created_at: os_process_created_at(pid),
                command_line: process_command_line(pid),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_live_and_its_creation_identity_is_stable() {
        let pid = std::process::id();

        assert!(os_process_is_alive(pid));
        let created_at = os_process_created_at(pid).expect("current process has a start time");
        assert_ne!(created_at, 0);
        assert_eq!(os_process_created_at(pid), Some(created_at));
    }

    #[test]
    fn nonexistent_processes_are_not_live_and_have_no_creation_identity() {
        let pid = u32::MAX;

        assert!(!os_process_is_alive(pid));
        assert_eq!(os_process_created_at(pid), None);
    }

    #[test]
    fn image_enumeration_matches_the_running_test_executable() {
        let executable = std::env::current_exe().expect("test executable path");
        let image_name = executable
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("test executable has a Unicode file name");

        let processes = processes_by_image(image_name);

        assert!(
            processes
                .iter()
                .any(|process| process.pid == std::process::id()),
            "the current process must be found by its exact executable identity"
        );
    }
}
