//! Sandbox-compatible proof of the process owning an already-connected TCP peer.
//!
//! A private App Group Unix socket identifies its server through kernel peer
//! credentials. That server inspects only its OWN TCP descriptors and confirms
//! the exact accepted connection tuple. The client never sends its broker token
//! until this proof succeeds. No process-inspection exception or bearer-token
//! fallback is used.
use crate::thread_owner::{Budget, Owned};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

static ACTORS: Budget = Budget::new(1);
const REQUEST: &[u8; 4] = b"QCP1";
const RESPONSE: &[u8; 4] = b"QCR1";
extern "C" {
    fn qc_store_owns_tcp_pair(server_port: u16, client_port: u16) -> i32;
}

fn witness_path(pid: u32) -> Result<PathBuf, String> {
    let state = crate::app_store::shared_directory()?;
    let group = state.parent().ok_or("Missing App Group container")?;
    // Put this short name at the group root, not the longer state subdirectory:
    // Darwin Unix socket paths have a 103-byte usable path envelope.
    let path = group.join(format!(".w{pid}"));
    if path.as_os_str().as_bytes().len() >= 104 {
        return Err("The App Group Unix socket path exceeds macOS's path limit".into());
    }
    Ok(path)
}

struct SocketFile {
    path: PathBuf,
    dev: u64,
    ino: u64,
}
impl Drop for SocketFile {
    fn drop(&mut self) {
        if let Ok(meta) = std::fs::symlink_metadata(&self.path) {
            if meta.dev() == self.dev && meta.ino() == self.ino && meta.file_type().is_socket() {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

pub struct IdentityWitness {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    actor: Owned,
}
impl Drop for IdentityWitness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = connect_until(&self.path, Instant::now() + Duration::from_millis(50));
        // Owned retains an unfinished original and its socket-file resource;
        // a deadline is not permission to detach the actor or release its claim.
        let _ = self.actor.wait_within(Duration::from_millis(500));
    }
}

pub fn start_witness() -> Result<IdentityWitness, String> {
    start_at(witness_path(std::process::id())?)
}

fn start_at(path: PathBuf) -> Result<IdentityWitness, String> {
    crate::paths_macos::prepare_directory(path.parent().ok_or("Missing witness parent")?)?;
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
            return Err("Refusing an unrelated broker identity socket".into());
        }
        // Production names contain THIS process's PID. A live endpoint there
        // is never replaced. A refused stale socket may remain after a crash.
        match connect_until(&path, Instant::now() + Duration::from_millis(100)) {
            Err(error) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {}
            _ => return Err("A broker identity socket already exists".into()),
        }
        let again = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if again.dev() != meta.dev() || again.ino() != meta.ino() {
            return Err("The identity socket changed during startup".into());
        }
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let listener =
        UnixListener::bind(&path).map_err(|e| format!("Bind broker identity socket: {e}"))?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    let resource = SocketFile {
        path: path.clone(),
        dev: meta.dev(),
        ino: meta.ino(),
    };
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let actor = Owned::spawn("qc-broker-peer-proof", &ACTORS, resource, move || {
        while !stopped.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let deadline = Instant::now() + Duration::from_millis(250);
                    let mut request = [0_u8; 8];
                    if peer(&stream).is_err()
                        || read_until(&mut stream, &mut request, deadline).is_err()
                        || &request[..4] != REQUEST
                    {
                        continue;
                    }
                    let server_port = u16::from_be_bytes([request[4], request[5]]);
                    let client_port = u16::from_be_bytes([request[6], request[7]]);
                    // SAFETY: this SDK-compiled shim inspects only getpid()'s
                    // own FDs and returns 1/0 or a negative errno, within 100ms.
                    let found = unsafe { qc_store_owns_tcp_pair(server_port, client_port) };
                    let mut response = [0_u8; 5];
                    response[..4].copy_from_slice(RESPONSE);
                    response[4] = if found == 1 {
                        1
                    } else if found == 0 {
                        0
                    } else {
                        2
                    };
                    let _ = write_until(&mut stream, &response, deadline);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let mut ready = libc::pollfd {
                        fd: listener.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // SAFETY: ready is one live pollfd. Incoming proof requests
                    // and the stop waker interrupt this low-frequency idle wait.
                    unsafe {
                        libc::poll(&mut ready, 1, 1000);
                    }
                }
                Err(_) => break,
            }
        }
    })?;
    Ok(IdentityWitness { path, stop, actor })
}

fn peer(stream: &UnixStream) -> Result<u32, String> {
    let mut uid = 0;
    let mut gid = 0;
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SAFETY: all output pointers refer to properly-sized live POD values.
    unsafe {
        if libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) != 0 || uid != libc::geteuid() {
            return Err("The identity socket has a foreign or unknown user".into());
        }
        if libc::getsockopt(
            stream.as_raw_fd(),
            0,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        ) != 0
            || length as usize != std::mem::size_of_val(&pid)
            || pid <= 0
        {
            return Err("The identity socket's kernel process identity is unavailable".into());
        }
    }
    Ok(pid as u32)
}

fn remaining(deadline: Instant) -> std::io::Result<Duration> {
    let duration = deadline.saturating_duration_since(Instant::now());
    if duration.is_zero() {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Broker identity proof timed out",
        ))
    } else {
        Ok(duration)
    }
}
fn read_until(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        match stream.read(bytes) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn write_until(
    stream: &mut UnixStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        match stream.write(bytes) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// std's UnixStream::connect has no timeout. Use a nonblocking socket plus poll
// so a full backlog or an unresponsive witness cannot consume an unbounded wait.
fn connect_until(path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= 104 || bytes.contains(&0) {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: socket returns a new owned FD; all syscall pointers/sizes below
    // refer to initialized POD storage and the descriptor remains owned here.
    unsafe {
        let raw = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(raw);
        let no_sigpipe = 1_i32;
        if libc::setsockopt(
            raw,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&no_sigpipe as *const i32).cast(),
            std::mem::size_of_val(&no_sigpipe) as _,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) < 0
            || libc::fcntl(raw, libc::F_SETFL, libc::O_NONBLOCK) < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut address: libc::sockaddr_un = std::mem::zeroed();
        address.sun_family = libc::AF_UNIX as _;
        for (out, byte) in address.sun_path.iter_mut().zip(bytes) {
            *out = *byte as _;
        }
        address.sun_len =
            (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as _;
        if libc::connect(
            raw,
            (&address as *const libc::sockaddr_un).cast(),
            address.sun_len as _,
        ) != 0
        {
            let error = std::io::Error::last_os_error();
            if !matches!(
                error.raw_os_error(),
                Some(libc::EINPROGRESS | libc::EAGAIN | libc::EINTR)
            ) {
                return Err(error);
            }
            loop {
                let mut item = libc::pollfd {
                    fd: raw,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let millis = remaining(deadline)?.as_millis().clamp(1, i32::MAX as u128) as i32;
                match libc::poll(&mut item, 1, millis) {
                    0 => return Err(std::io::ErrorKind::TimedOut.into()),
                    -1 if std::io::Error::last_os_error().kind()
                        == std::io::ErrorKind::Interrupted =>
                    {
                        continue
                    }
                    -1 => return Err(std::io::Error::last_os_error()),
                    _ => {}
                }
                let mut error = 0_i32;
                let mut size = std::mem::size_of_val(&error) as libc::socklen_t;
                if libc::getsockopt(
                    raw,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut error as *mut i32).cast(),
                    &mut size,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if error != 0 {
                    return Err(std::io::Error::from_raw_os_error(error));
                }
                break;
            }
        }
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(false)?;
        Ok(stream)
    }
}

pub fn owner_of_connection(
    stream: &TcpStream,
    expected_pid: u32,
    within: Duration,
) -> Result<Option<u32>, String> {
    verify_at(stream, expected_pid, &witness_path(expected_pid)?, within)
}
fn verify_at(
    stream: &TcpStream,
    expected_pid: u32,
    path: &Path,
    within: Duration,
) -> Result<Option<u32>, String> {
    fn local_port(address: SocketAddr) -> Result<u16, String> {
        match address {
            SocketAddr::V4(value) if *value.ip() == Ipv4Addr::LOCALHOST && value.port() != 0 => {
                Ok(value.port())
            }
            _ => Err("Broker peer proof requires exact IPv4 localhost endpoints".into()),
        }
    }
    let server = local_port(stream.peer_addr().map_err(|e| e.to_string())?)?;
    let client = local_port(stream.local_addr().map_err(|e| e.to_string())?)?;
    let generation = crate::process::os_process_created_at(expected_pid)
        .ok_or("The published broker's process generation is unavailable")?;
    let deadline = Instant::now() + within;
    loop {
        remaining(deadline).map_err(|e| e.to_string())?;
        let mut witness = match connect_until(path, deadline) {
            Ok(stream) => stream,
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ECONNREFUSED)) => {
                std::thread::sleep(
                    Duration::from_millis(5).min(remaining(deadline).map_err(|e| e.to_string())?),
                );
                continue;
            }
            Err(e) => {
                return Err(format!(
                    "Connect to the broker's private identity witness: {e}"
                ))
            }
        };
        if peer(&witness)? != expected_pid {
            return Err(
                "The identity witness belongs to a different process; no broker token was sent"
                    .into(),
            );
        }
        let mut request = [0_u8; 8];
        request[..4].copy_from_slice(REQUEST);
        request[4..6].copy_from_slice(&server.to_be_bytes());
        request[6..8].copy_from_slice(&client.to_be_bytes());
        write_until(&mut witness, &request, deadline).map_err(|e| e.to_string())?;
        let mut response = [0_u8; 5];
        read_until(&mut witness, &mut response, deadline).map_err(|e| e.to_string())?;
        if &response[..4] != RESPONSE || response[4] > 1 {
            return Err("The broker could not inspect its own accepted TCP connection".into());
        }
        if response[4] == 1 {
            if crate::process::os_process_created_at(expected_pid) != Some(generation) {
                return Err("The broker identity changed during its connection proof".into());
            }
            return Ok(Some(expected_pid));
        }
        std::thread::sleep(
            Duration::from_millis(5).min(remaining(deadline).map_err(|e| e.to_string())?),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{atomic::AtomicUsize, Mutex};
    static SERIAL: Mutex<()> = Mutex::new(());
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = PathBuf::from("/private/tmp").join(format!(
                "qw-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new().create(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn socket(&self) -> PathBuf {
            self.0.join("w")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn connection() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }
    #[test]
    fn proves_the_exact_accepted_connection_without_sending_tcp_bytes() {
        let _serial = SERIAL.lock().unwrap();
        let fixture = Fixture::new();
        let witness = start_at(fixture.socket()).unwrap();
        let (client, mut server) = connection();
        assert_eq!(
            verify_at(
                &client,
                std::process::id(),
                &fixture.socket(),
                Duration::from_secs(2)
            )
            .unwrap(),
            Some(std::process::id())
        );
        server.set_nonblocking(true).unwrap();
        assert_eq!(
            server.read(&mut [0]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(witness);
        assert!(!fixture.socket().exists());
    }
    #[test]
    fn rejects_a_witness_with_the_wrong_kernel_process_identity() {
        let _serial = SERIAL.lock().unwrap();
        let fixture = Fixture::new();
        let _witness = start_at(fixture.socket()).unwrap();
        let (client, _server) = connection();
        let parent = crate::process::os_parent_process_id().unwrap();
        assert_ne!(parent, std::process::id());
        assert!(verify_at(&client, parent, &fixture.socket(), Duration::from_secs(1)).is_err());
    }
    #[test]
    fn a_missing_accepted_socket_never_produces_an_owner_proof() {
        let _serial = SERIAL.lock().unwrap();
        let fixture = Fixture::new();
        let _witness = start_at(fixture.socket()).unwrap();
        let (client, server) = connection();
        drop(server);
        assert!(verify_at(
            &client,
            std::process::id(),
            &fixture.socket(),
            Duration::from_millis(75)
        )
        .is_err());
    }
    #[test]
    fn an_unresponsive_witness_cannot_hold_the_client_beyond_its_io_deadline() {
        let _serial = SERIAL.lock().unwrap();
        let fixture = Fixture::new();
        let listener = UnixListener::bind(fixture.socket()).unwrap();
        let (client, mut server) = connection();
        let actor = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(150));
        });
        assert!(verify_at(
            &client,
            std::process::id(),
            &fixture.socket(),
            Duration::from_millis(25)
        )
        .is_err());
        server.set_nonblocking(true).unwrap();
        assert_eq!(
            server.read(&mut [0]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        actor.join().unwrap();
    }
    #[test]
    fn unrelated_files_are_not_replaced_by_a_witness() {
        let _serial = SERIAL.lock().unwrap();
        let fixture = Fixture::new();
        std::fs::write(fixture.socket(), b"unrelated").unwrap();
        assert!(start_at(fixture.socket()).is_err());
        assert_eq!(std::fs::read(fixture.socket()).unwrap(), b"unrelated");
    }
    #[test]
    fn malformed_requests_do_not_poison_later_connection_proofs() {
        let _serial = SERIAL.lock().unwrap();
        let fixture = Fixture::new();
        let _witness = start_at(fixture.socket()).unwrap();
        let mut malformed =
            connect_until(&fixture.socket(), Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(peer(&malformed).unwrap(), std::process::id());
        malformed.write_all(b"BAD!\0\0\0\0").unwrap();
        malformed
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(malformed.read(&mut [0]).unwrap(), 0);
        let (client, _server) = connection();
        assert_eq!(
            verify_at(
                &client,
                std::process::id(),
                &fixture.socket(),
                Duration::from_secs(1)
            )
            .unwrap(),
            Some(std::process::id())
        );
    }
}
