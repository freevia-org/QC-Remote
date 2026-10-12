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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Listener {
        rank: u8,
        inode: u64,
        uid: u32,
    }

    // procfs prints each address word in native byte order, not IP notation.
    fn address_bytes(text: &str) -> Option<Vec<u8>> {
        if !matches!(text.len(), 8 | 32) || !text.is_ascii() {
            return None;
        }
        let (chunks, remainder) = text.as_bytes().as_chunks::<8>();
        if !remainder.is_empty() {
            return None;
        }
        chunks
            .iter()
            .map(|chunk| {
                let word = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
                Some(word.to_ne_bytes())
            })
            .collect::<Option<Vec<_>>>()
            .map(|words| words.into_iter().flatten().collect())
    }

    fn listeners(table: &str, port: u16) -> Result<Vec<Listener>, String> {
        let mut found = Vec::new();
        for row in table.lines().skip(1).filter(|line| !line.trim().is_empty()) {
            let fields: Vec<&str> = row.split_whitespace().collect();
            if fields.len() < 4 {
                return Err("Malformed kernel TCP listener table".into());
            }
            if fields[3] != "0A" {
                continue;
            }
            if fields.len() < 10 {
                return Err("Malformed kernel TCP listener row".into());
            }
            let (address, wire_port) = fields[1]
                .split_once(':')
                .ok_or("Malformed kernel TCP address")?;
            let wire_port =
                u16::from_str_radix(wire_port, 16).map_err(|_| "Malformed kernel TCP port")?;
            if wire_port != port {
                continue;
            }
            let bytes = address_bytes(address).ok_or("Malformed kernel TCP address bytes")?;
            let rank = match bytes.as_slice() {
                [127, 0, 0, 1] => 0,
                [0, 0, 0, 0] => 1,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 127, 0, 0, 1] => 0,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => 2,
                _ => continue,
            };
            found.push(Listener {
                rank,
                uid: fields[7].parse().map_err(|_| "Malformed TCP socket uid")?,
                inode: fields[9]
                    .parse()
                    .map_err(|_| "Malformed TCP socket inode")?,
            });
        }
        Ok(found)
    }

    fn loopback_socket(port: u16) -> Result<Option<Listener>, String> {
        if port == 0 {
            return Err("A zero broker port cannot identify a listener".into());
        }
        let ipv4 = fs::read_to_string("/proc/self/net/tcp")
            .map_err(|error| format!("Read IPv4 TCP owners: {error}"))?;
        let ipv6 = match fs::read_to_string("/proc/self/net/tcp6") {
            Ok(table) => table,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("Read IPv6 TCP owners: {error}")),
        };
        let mut matches = listeners(&ipv4, port)?;
        matches.extend(listeners(&ipv6, port)?);
        let Some(rank) = matches.iter().map(|item| item.rank).min() else {
            return Ok(None);
        };
        matches.retain(|item| item.rank == rank);
        matches.sort_by_key(|item| (item.inode, item.uid));
        matches.dedup();
        if matches.len() != 1 || matches[0].inode == 0 {
            return Err("The loopback port does not have one identifiable listening socket".into());
        }
        Ok(matches.pop())
    }

    /// Resolve the listening socket's inode back to its owning process.
    /// A foreign uid, hidden procfs, inherited multi-process listener or a
    /// changing socket is unknown, never permission to transmit a bearer token.
    pub fn loopback_listener_owner(port: u16) -> Result<Option<u32>, String> {
        use std::os::unix::fs::MetadataExt;
        let Some(socket) = loopback_socket(port)? else {
            return Ok(None);
        };
        // SAFETY: geteuid has no preconditions.
        if socket.uid != unsafe { libc::geteuid() } {
            return Err("The broker port belongs to another user".into());
        }
        let expected = format!("socket:[{}]", socket.inode);
        let entries =
            fs::read_dir("/proc").map_err(|error| format!("Read process owners: {error}"))?;
        let mut owners = Vec::new();
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(metadata) = fs::metadata(entry.path()) else {
                continue;
            };
            if metadata.uid() != socket.uid {
                continue;
            }
            let Ok(files) = fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            if files.flatten().any(|file| {
                fs::read_link(file.path()).is_ok_and(|link| link == Path::new(&expected))
            }) {
                owners.push((pid, os_process_created_at(pid)));
            }
        }
        if owners.len() != 1 {
            return Err("The listening socket has no unique readable process owner".into());
        }
        let (owner, identity) = owners[0];
        if identity.is_none()
            || !os_process_is_alive(owner)
            || os_process_created_at(owner) != identity
            || loopback_socket(port)? != Some(socket)
        {
            return Err("The listening socket changed while its owner was checked".into());
        }
        Ok(Some(owner))
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
        fn listener_parser_distinguishes_loopback_wildcards_and_remote_addresses() {
            let row = |address: &str, state: &str| {
                format!(
                "header\n 0: {address}:4321 00000000:0000 {state} 00000000:00000000 00:00000000 00000000 1000 0 12345 1"
            )
            };
            let word = |bytes| format!("{:08X}", u32::from_ne_bytes(bytes));
            assert_eq!(
                listeners(&row(&word([127, 0, 0, 1]), "0A"), 0x4321).unwrap(),
                vec![Listener {
                    rank: 0,
                    inode: 12345,
                    uid: 1000
                }]
            );
            assert_eq!(
                listeners(&row("00000000", "0A"), 0x4321).unwrap()[0].rank,
                1
            );
            assert!(listeners(&row(&word([192, 168, 1, 1]), "0A"), 0x4321)
                .unwrap()
                .is_empty());
            assert!(listeners(&row(&word([127, 0, 0, 1]), "01"), 0x4321)
                .unwrap()
                .is_empty());
            assert!(listeners("header\ntruncated", 0x4321).is_err());
            assert!(
                listeners("header\n0: 00000000:4321 00000000:0000 06", 0x4321)
                    .unwrap()
                    .is_empty()
            );
            assert!(address_bytes("0000000").is_none());
        }

        #[test]
        fn live_loopback_listener_is_bound_to_its_actual_process() {
            let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            let port = socket.local_addr().unwrap().port();
            assert_eq!(
                loopback_listener_owner(port).unwrap(),
                Some(std::process::id())
            );
            drop(socket);
            assert_eq!(loopback_listener_owner(port).unwrap(), None);
            assert!(loopback_listener_owner(0).is_err());
        }

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
            use std::os::unix::fs::MetadataExt;
            let expected = fs::metadata(&unrelated).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut observed_executable = None;
            while std::time::Instant::now() < deadline {
                if !os_process_is_alive(unrelated_pid) {
                    break;
                }
                observed_executable = fs::metadata(format!("/proc/{unrelated_pid}/exe"))
                    .ok()
                    .map(|metadata| (metadata.dev(), metadata.ino()));
                if observed_executable == Some((expected.dev(), expected.ino())) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(
                os_process_is_alive(unrelated_pid),
                "unrelated executable exited before its procfs identity was observable"
            );
            assert_eq!(
                observed_executable,
                Some((expected.dev(), expected.ino())),
                "procfs did not expose the unrelated executable identity before the deadline"
            );
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
