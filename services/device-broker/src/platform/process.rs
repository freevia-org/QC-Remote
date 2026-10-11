//! OS process facts. Windows keeps its existing implementation; Linux reads
//! procfs and treats inaccessible or malformed process data as unknown.

#[cfg(not(target_os = "linux"))]
pub use qc_windows_process::*;

#[cfg(target_os = "linux")]
mod linux {
    use sha2::{Digest, Sha256};
    use std::{fs, io, path::Path};

    #[derive(Debug, PartialEq, Eq)]
    struct Stat {
        parent: u32,
        start_ticks: u64,
        state: u8,
    }

    // comm is parenthesized and may itself contain spaces and parentheses.
    // Fields after its final ')' start with field 3 (state); starttime is 22.
    fn parse_stat(text: &str) -> Option<Stat> {
        let (_, fields) = text.rsplit_once(')')?;
        let fields: Vec<&str> = fields.split_whitespace().collect();
        let state = *fields.first()?.as_bytes().first()?;
        if fields.first()?.len() != 1 {
            return None;
        }
        Some(Stat {
            parent: fields.get(1)?.parse().ok()?,
            start_ticks: fields.get(19)?.parse().ok()?,
            state,
        })
    }

    fn read_stat(pid: u32) -> io::Result<Stat> {
        let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        parse_stat(&text).ok_or_else(|| io::Error::other("Malformed process stat"))
    }

    /// False only when the kernel proves absence or an exited/zombie state.
    pub fn os_process_is_alive(pid: u32) -> bool {
        let Ok(pid_native) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid == 0 {
            return false;
        }
        match read_stat(pid) {
            Ok(stat) => !matches!(stat.state, b'Z' | b'X' | b'x'),
            Err(_) => {
                // procfs may be hidden or unavailable. Signal zero changes
                // nothing; EPERM and every unexpected error mean unknown.
                // SAFETY: a positive PID and signal 0, no pointers.
                let exists = unsafe { libc::kill(pid_native, 0) == 0 };
                exists || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            }
        }
    }

    fn identity(boot_id: &str, start_ticks: u64) -> Option<u64> {
        let boot_id = boot_id.trim();
        if boot_id.len() != 36
            || !boot_id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        {
            return None;
        }
        let mut digest = Sha256::new();
        digest.update(b"qc-linux-process-v1\0");
        digest.update(boot_id.as_bytes());
        digest.update(start_ticks.to_le_bytes());
        Some(u64::from_le_bytes(digest.finalize()[..8].try_into().ok()?))
    }

    /// Opaque equality token for this boot and process start, not a timestamp.
    /// Hashing boot_id prevents a persisted record matching after a reboot;
    /// start ticks prevent PID reuse within a boot. Unknown remains None.
    pub fn os_process_created_at(pid: u32) -> Option<u64> {
        let stat = read_stat(pid).ok()?;
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        identity(&boot, stat.start_ticks)
    }

    pub fn os_parent_process_id() -> Option<u32> {
        read_stat(std::process::id())
            .ok()
            .map(|stat| stat.parent)
            .filter(|pid| *pid != 0)
    }

    pub struct ProcessInfo {
        pub pid: u32,
        pub created_at: Option<u64>,
        pub command_line: Option<String>,
    }

    /// A diagnostic fallback for this executable's stdio brokers, never a
    /// kill-by-name. A basename alone is not evidence of a Linux device owner.
    /// Other installations still contend through the lease and hidraw lock.
    pub fn processes_by_image(image_name: &str) -> Vec<ProcessInfo> {
        use std::os::unix::fs::MetadataExt;
        let Ok(own_executable) = fs::metadata("/proc/self/exe") else {
            return Vec::new();
        };
        // SAFETY: geteuid has no arguments or preconditions.
        let uid = unsafe { libc::geteuid() };
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let executable = fs::read_link(entry.path().join("exe")).ok()?;
                if executable.file_name()? != Path::new(image_name).file_name()? {
                    return None;
                }
                let metadata = fs::metadata(entry.path().join("exe")).ok()?;
                if (metadata.dev(), metadata.ino()) != (own_executable.dev(), own_executable.ino())
                    || fs::metadata(entry.path()).ok()?.uid() != uid
                {
                    return None;
                }
                let command_line = fs::read(entry.path().join("cmdline"))
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|args| args.replace('\0', " "));
                Some(ProcessInfo {
                    pid,
                    created_at: os_process_created_at(pid),
                    command_line,
                })
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn stat_parser_handles_parentheses_and_rejects_missing_fields() {
            let text = "42 (a tricky ) name) S 7 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 987654 0";
            assert_eq!(
                parse_stat(text),
                Some(Stat {
                    parent: 7,
                    state: b'S',
                    start_ticks: 987654
                })
            );
            assert!(parse_stat("42 (broken) S 7").is_none());
            assert!(parse_stat("42 no brackets S 7").is_none());
        }

        #[test]
        fn identity_distinguishes_reboots_and_pid_reuse() {
            let first = "d9fc508e-1333-4833-86cf-4bef99c34d3b";
            let next = "0dcb0798-5709-4051-a1a9-c7667d3c48ec";
            assert_eq!(identity(first, 100), identity(first, 100));
            assert_ne!(identity(first, 100), identity(first, 101));
            assert_ne!(identity(first, 100), identity(next, 100));
            assert!(identity("", 100).is_none());
            assert!(identity("not-a-boot-id", 100).is_none());
        }

        #[test]
        fn current_process_has_stable_identity_and_parent() {
            let pid = std::process::id();
            assert!(os_process_is_alive(pid));
            let identity = os_process_created_at(pid).expect("procfs boot and process identity");
            assert_eq!(os_process_created_at(pid), Some(identity));
            assert!(os_parent_process_id().is_some());
            assert!(!os_process_is_alive(0));
            assert!(!os_process_is_alive(u32::MAX));
        }

        #[test]
        fn process_scan_finds_its_exact_executable_and_refuses_an_absent_name() {
            let executable = fs::read_link("/proc/self/exe").unwrap();
            let name = executable.file_name().unwrap().to_str().unwrap();
            let found = processes_by_image(name);
            let own = found
                .iter()
                .find(|process| process.pid == std::process::id())
                .unwrap();
            assert_eq!(own.created_at, os_process_created_at(std::process::id()));
            assert!(own.command_line.is_some());
            assert!(processes_by_image("qc-remote-nonexistent-test-executable").is_empty());
        }

        #[test]
        fn process_scan_refuses_an_unrelated_binary_with_the_same_name() {
            struct Fixture {
                directory: std::path::PathBuf,
                child: Option<std::process::Child>,
            }
            impl Drop for Fixture {
                fn drop(&mut self) {
                    if let Some(child) = self.child.as_mut() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    let _ = fs::remove_dir_all(&self.directory);
                }
            }
            let mut fixture = Fixture {
                directory: std::env::temp_dir()
                    .join(format!("qc-process-scan-{}", std::process::id())),
                child: None,
            };
            fs::create_dir(&fixture.directory).unwrap();
            let own = fs::read_link("/proc/self/exe").unwrap();
            let name = own.file_name().unwrap().to_str().unwrap();
            let unrelated = fixture.directory.join(name);
            fs::copy("/bin/sleep", &unrelated).unwrap();
            fixture.child = Some(
                std::process::Command::new(&unrelated)
                    .arg("60")
                    .spawn()
                    .unwrap(),
            );
            let unrelated_pid = fixture.child.as_ref().unwrap().id();
            assert!(os_process_is_alive(unrelated_pid));
            let found = processes_by_image(name);
            assert!(found
                .iter()
                .any(|process| process.pid == std::process::id()));
            assert!(!found.iter().any(|process| process.pid == unrelated_pid));
        }

        #[test]
        fn an_exited_child_is_dead_even_before_reaping() {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while os_process_is_alive(child.id()) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let alive = os_process_is_alive(child.id());
            let _ = child.kill();
            child.wait().unwrap();
            assert!(!alive);
            assert!(!os_process_is_alive(child.id()));
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::*;
