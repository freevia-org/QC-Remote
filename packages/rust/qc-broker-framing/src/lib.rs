//! Length-prefixed byte framing shared by local broker clients and servers.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Instant;

/// Reads one frame body, returning `None` only for a clean EOF before a header.
/// The declared size is rejected before allocating the body.
pub fn read_frame<R: Read + ?Sized>(
    input: &mut R,
    max_bytes: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0_u8; 4];
    let mut filled = 0;
    while filled < header.len() {
        match input.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete frame header",
                ))
            }
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid frame length: {length}"),
        ));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Writes one length-prefixed frame and flushes the writer.
pub fn write_frame<W: Write + ?Sized>(
    output: &mut W,
    body: &[u8],
    max_bytes: usize,
) -> io::Result<()> {
    if body.is_empty() || body.len() > max_bytes || body.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid frame length",
        ));
    }
    output.write_all(&(body.len() as u32).to_be_bytes())?;
    output.write_all(body)?;
    output.flush()
}

/// Writes one TCP frame under one absolute deadline. Frame preparation counts
/// against the same deadline: the caller captured it before this call, and the
/// first send checks the clock before writing anything.
pub fn write_tcp_frame_before_deadline(
    output: &mut TcpStream,
    body: &[u8],
    max_bytes: usize,
    deadline: Instant,
) -> io::Result<()> {
    if body.is_empty() || body.len() > max_bytes || body.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid frame length",
        ));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    write_tcp_bytes_before_deadline(output, &frame, deadline)
}

// FUTURE macOS WORK (not implemented; deliberately no placeholder tests).
// Darwin's `sosend` waits for send-buffer space based on `SS_NBIO`/`MSG_NBIO`
// only, so `send(MSG_DONTWAIT)` on a blocking socket does not bound that wait
// (PR #203 macOS CI: single sends blocked for tens of seconds). Until a
// native reviewer chooses a supported Darwin primitive that provably bounds
// the buffer wait under the original deadline, macOS keeps the `SO_SNDTIMEO`
// path above and its overshoot remains a known limit. Do not substitute the
// private `MSG_NBIO` flag, and do not infer non-blocking behaviour from
// `SO_SNDBUF`/`SO_NWRITE` samples (a readiness-then-blocking-send heuristic).

/// Writes already-framed TCP bytes under an absolute deadline.
///
/// On Linux every send is non-blocking for that call only and every wait is
/// recomputed from the one original deadline; the socket's mode and timeouts
/// are never changed.
///
/// Every other target, including macOS and Windows, keeps the existing
/// blocking write whose `SO_SNDTIMEO` is recomputed between partial writes
/// and restored afterwards. On macOS that is a KNOWN LIMIT: the deadline can
/// be exceeded under backpressure (CI measured 7.0-7.1 s against a 5 s
/// budget). The Linux technique is not applied there because Darwin's
/// `send(MSG_DONTWAIT)` on a blocking socket was not honoured in the PR #203
/// macOS CI run (sends blocked for tens of seconds), and no other Darwin
/// approach has been proven safe.
pub fn write_tcp_bytes_before_deadline(
    output: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        unix_send::write_tcp_bytes(output, bytes, deadline)
    }
    #[cfg(not(target_os = "linux"))]
    {
        write_tcp_bytes_with_socket_timeout(output, bytes, deadline)
    }
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn write_tcp_bytes_with_socket_timeout(
    output: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    let previous = output.write_timeout()?;
    let result = write_bytes_before_deadline(
        bytes,
        || deadline.saturating_duration_since(Instant::now()),
        |pending, remaining| {
            output.set_write_timeout(Some(remaining))?;
            output.write(pending)
        },
    );
    let restored = output.set_write_timeout(previous);
    restore_result(result, restored)
}

/// Reads one TCP frame under one absolute deadline, recomputing the socket
/// timeout before each partial read and restoring the previous timeout.
pub fn read_tcp_frame_before_deadline(
    input: &mut TcpStream,
    max_bytes: usize,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    let previous = input.read_timeout()?;
    let result = (|| {
        let mut header = [0_u8; 4];
        read_tcp_exact_before_deadline(input, &mut header, deadline)?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid frame length: {length}"),
            ));
        }
        let mut body = vec![0; length];
        read_tcp_exact_before_deadline(input, &mut body, deadline)?;
        Ok(body)
    })();
    let restored = input.set_read_timeout(previous);
    restore_result(result, restored)
}

fn restore_result<T>(result: io::Result<T>, restored: io::Result<()>) -> io::Result<T> {
    match (result, restored) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn write_bytes_before_deadline(
    bytes: &[u8],
    mut remaining: impl FnMut() -> std::time::Duration,
    mut write: impl FnMut(&[u8], std::time::Duration) -> io::Result<usize>,
) -> io::Result<()> {
    let mut pending = bytes;
    while !pending.is_empty() {
        let timeout = remaining();
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "frame write deadline elapsed",
            ));
        }
        match write(pending, timeout) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "broker socket closed",
                ))
            }
            Ok(count) => pending = &pending[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_tcp_exact_before_deadline(
    input: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "frame read deadline elapsed",
            ));
        }
        input.set_read_timeout(Some(remaining))?;
        match input.read(&mut buffer[filled..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "broker socket closed",
                ))
            }
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Deadline-bounded non-blocking send loop, independent of the socket API so it
/// can be exercised with a scripted sender.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod readiness {
    use std::io;
    use std::time::{Duration, Instant};

    /// Outcome of waiting for the socket to become writable.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Readiness {
        Writable,
        /// The wait ended without readiness; the caller re-checks the deadline.
        Expired,
        /// The wait was interrupted; the caller recomputes the remaining budget.
        Interrupted,
        /// The socket reports an error or hang-up condition.
        ErrorState,
    }

    pub(crate) trait NonblockingSender {
        /// Offers `pending` once without ever blocking. `WouldBlock` and
        /// `Interrupted` are reported through the error kind.
        fn send(&mut self, pending: &[u8]) -> io::Result<usize>;
        /// Waits at most `remaining` for writability. An invalid descriptor is
        /// an `Err`, never a readiness value.
        fn wait_writable(&mut self, remaining: Duration) -> io::Result<Readiness>;
        /// The error behind an error-state readiness that a send did not surface.
        fn pending_error(&mut self) -> io::Error;
    }

    /// Converts a remaining budget to a `poll` timeout: rounded up so a
    /// sub-millisecond remainder cannot become a zero-timeout spin, saturated
    /// to the largest value `poll` accepts.
    pub(crate) fn poll_timeout_millis(remaining: Duration) -> i32 {
        let millis = remaining.as_nanos().div_ceil(1_000_000);
        i32::try_from(millis).unwrap_or(i32::MAX)
    }

    fn deadline_elapsed() -> io::Error {
        io::Error::new(io::ErrorKind::TimedOut, "frame write deadline elapsed")
    }

    /// Writes all of `bytes` or fails by `deadline`. Every offer is a suffix of
    /// `bytes` and advances only by the count the sender accepted, so bytes are
    /// never repeated or reordered. A frame whose last byte was accepted is
    /// `Ok` even if the clock has since passed the deadline.
    pub(crate) fn write_with_readiness<S: NonblockingSender>(
        sender: &mut S,
        bytes: &[u8],
        deadline: Instant,
        mut now: impl FnMut() -> Instant,
    ) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut pending = bytes;
        let mut error_ready = false;
        loop {
            if pending.is_empty() {
                return Ok(());
            }
            if deadline.saturating_duration_since(now()).is_zero() {
                return Err(deadline_elapsed());
            }
            match sender.send(pending) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "broker socket closed",
                    ))
                }
                Ok(count) => {
                    pending = pending.get(count..).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "sender accepted more bytes than offered",
                        )
                    })?;
                    error_ready = false;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if error_ready {
                        // The previous wait reported an error state and this
                        // send still did not surface it: report it, no re-wait.
                        return Err(sender.pending_error());
                    }
                    let remaining = deadline.saturating_duration_since(now());
                    if remaining.is_zero() {
                        return Err(deadline_elapsed());
                    }
                    if sender.wait_writable(remaining)? == Readiness::ErrorState {
                        error_ready = true;
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod unix_send {
    use super::readiness::{
        poll_timeout_millis, write_with_readiness, NonblockingSender, Readiness,
    };
    use std::io;
    use std::net::TcpStream;
    use std::os::unix::io::{AsRawFd, RawFd};
    use std::time::{Duration, Instant};

    /// Per-call flags: never block, never raise SIGPIPE. Neither changes the
    /// shared socket's mode, so a cloned blocking reader is unaffected.
    pub(super) const SEND_FLAGS: libc::c_int = libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL;

    /// Latency courtesy only: bounds how much one send copies before the
    /// deadline is checked again. It is not a bound on kernel copy time.
    const SEND_CHUNK_CAP: usize = 1024 * 1024;

    struct SocketSender {
        fd: RawFd,
        flags: libc::c_int,
    }

    impl NonblockingSender for SocketSender {
        fn send(&mut self, pending: &[u8]) -> io::Result<usize> {
            let length = pending.len().min(SEND_CHUNK_CAP);
            // SAFETY: the pointer and length describe a live slice, and the
            // descriptor is borrowed from a TcpStream that outlives this call.
            let sent = unsafe { libc::send(self.fd, pending.as_ptr().cast(), length, self.flags) };
            if sent < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(sent as usize)
            }
        }

        fn wait_writable(&mut self, remaining: Duration) -> io::Result<Readiness> {
            let mut descriptor = libc::pollfd {
                fd: self.fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: one valid pollfd that lives for the duration of the call.
            let ready = unsafe { libc::poll(&mut descriptor, 1, poll_timeout_millis(remaining)) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                return if error.kind() == io::ErrorKind::Interrupted {
                    Ok(Readiness::Interrupted)
                } else {
                    Err(error)
                };
            }
            if ready == 0 {
                return Ok(Readiness::Expired);
            }
            if descriptor.revents & libc::POLLNVAL != 0 {
                Err(io::Error::from_raw_os_error(libc::EBADF))
            } else if descriptor.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                Ok(Readiness::ErrorState)
            } else {
                Ok(Readiness::Writable)
            }
        }

        fn pending_error(&mut self) -> io::Error {
            let mut value: libc::c_int = 0;
            let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: the out-pointers reference live locals of the stated size.
            let result = unsafe {
                libc::getsockopt(
                    self.fd,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut value as *mut libc::c_int).cast(),
                    &mut length,
                )
            };
            if result == 0 && value != 0 {
                io::Error::from_raw_os_error(value)
            } else {
                io::Error::new(io::ErrorKind::BrokenPipe, "broker socket reported an error")
            }
        }
    }

    pub(super) fn write_fd_before_deadline(
        fd: RawFd,
        bytes: &[u8],
        deadline: Instant,
        flags: libc::c_int,
    ) -> io::Result<()> {
        let mut sender = SocketSender { fd, flags };
        write_with_readiness(&mut sender, bytes, deadline, Instant::now)
    }

    pub(super) fn write_tcp_bytes(
        output: &TcpStream,
        bytes: &[u8],
        deadline: Instant,
    ) -> io::Result<()> {
        write_fd_before_deadline(output.as_raw_fd(), bytes, deadline, SEND_FLAGS)
    }
}

#[cfg(test)]
mod tests {
    use super::readiness::{
        poll_timeout_millis, write_with_readiness, NonblockingSender, Readiness,
    };
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    struct OneByteReader<R>(R);

    impl<R: Read> Read for OneByteReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let limit = buffer.len().min(1);
            self.0.read(&mut buffer[..limit])
        }
    }

    #[test]
    fn round_trips_partial_reads_and_checks_limit_before_body() {
        let mut wire = Cursor::new(Vec::new());
        write_frame(&mut wire, b"frame", 5).unwrap();
        let bytes = wire.into_inner();
        assert_eq!(
            read_frame(&mut OneByteReader(Cursor::new(bytes)), 5).unwrap(),
            Some(b"frame".to_vec())
        );
        assert_eq!(
            read_frame(&mut Cursor::new(Vec::<u8>::new()), 5).unwrap(),
            None
        );
        assert_eq!(
            write_frame(&mut Vec::new(), b"123456", 5)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let oversized = (6_u32).to_be_bytes();
        assert_eq!(
            read_frame(&mut Cursor::new(oversized), 5)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn partial_write_cannot_refresh_its_deadline() {
        let mut remaining = [Duration::from_millis(3), Duration::ZERO].into_iter();
        let mut written = Vec::new();
        let mut attempts = 0;
        let error = write_bytes_before_deadline(
            b"frame",
            || remaining.next().unwrap_or(Duration::ZERO),
            |pending, timeout| {
                attempts += 1;
                assert!(!timeout.is_zero());
                let count = pending.len().min(2);
                written.extend_from_slice(&pending[..count]);
                Ok(count)
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(written, b"fr");
        assert_eq!(attempts, 1);
    }

    #[test]
    fn timeout_restoration_preserves_the_original_io_result() {
        let restore_error = || io::Error::other("restore failed");
        assert_eq!(
            restore_result(Ok(7), Err(restore_error()))
                .unwrap_err()
                .to_string(),
            "restore failed"
        );
        assert_eq!(
            restore_result::<()>(
                Err(io::Error::new(io::ErrorKind::TimedOut, "deadline")),
                Err(restore_error()),
            )
            .unwrap_err()
            .to_string(),
            "deadline"
        );
    }

    /// Accepts on a nonblocking listener, retrying `WouldBlock` until `limit`.
    /// A completed `connect` can be visible to the client before the listener's
    /// accept queue shows it (observed on macOS), so one `accept` call is not
    /// enough. Any other error, or the limit expiring, fails the test loudly.
    fn accept_before_deadline(listener: &TcpListener, limit: Duration) -> TcpStream {
        let deadline = Instant::now() + limit;
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "no pending connection within {limit:?} after connect completed"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    #[test]
    fn tcp_read_uses_one_deadline_across_partial_header_and_restores_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        // Establish the pair before starting a worker: connection failure must
        // not detach a thread blocked forever in accept().
        let mut client = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut stream = accept_before_deadline(&listener, Duration::from_secs(5));
        stream.set_nonblocking(false).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (first_byte_tx, first_byte_rx) = mpsc::channel();
        let (first_byte_sent_tx, first_byte_sent_rx) = mpsc::channel();
        let (read_finished_tx, read_finished_rx) = mpsc::channel();
        thread::scope(|scope| {
            let server = scope.spawn(move || {
                accepted_tx.send(()).unwrap();
                first_byte_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("client releases first byte");
                stream.write_all(&[0]).unwrap();
                first_byte_sent_tx.send(()).unwrap();
                read_finished_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("client reports read result");
                let _ = stream.write_all(&[0, 0, 1, b'x']);
            });
            accepted_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("server accepted client");
            let previous_timeout = Some(Duration::from_secs(2));
            client.set_read_timeout(previous_timeout).unwrap();
            // Start the absolute deadline before allowing the server to send the
            // first header byte. The server holds the tail until this read has
            // returned, so scheduler delays cannot turn the test into a success.
            let deadline = Instant::now() + Duration::from_millis(10);
            first_byte_tx.send(()).unwrap();
            first_byte_sent_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("server sent a partial header");
            let error = read_tcp_frame_before_deadline(&mut client, 16, deadline).unwrap_err();
            read_finished_tx.send(()).unwrap();
            assert!(matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ));
            assert_eq!(client.read_timeout().unwrap(), previous_timeout);
            server.join().unwrap();
        });
    }

    enum SendStep {
        Accept(usize),
        WouldBlock,
        Interrupted,
    }

    enum WaitStep {
        Writable,
        Expired,
        Interrupted,
        ErrorState,
        Invalid,
    }

    /// Scripted sender on a virtual clock. An exhausted send script reports
    /// `WouldBlock`; an exhausted wait script reports `Expired`.
    struct FakeSender {
        clock: Rc<Cell<Duration>>,
        send_cost: Duration,
        wait_cost: Duration,
        sends: VecDeque<SendStep>,
        waits: VecDeque<WaitStep>,
        offered: Vec<Vec<u8>>,
        accepted: Vec<u8>,
        wait_remaining: Vec<Duration>,
        pending_error_calls: usize,
    }

    impl FakeSender {
        fn new(send_cost_ms: u64, wait_cost_ms: u64) -> Self {
            Self {
                clock: Rc::new(Cell::new(Duration::ZERO)),
                send_cost: Duration::from_millis(send_cost_ms),
                wait_cost: Duration::from_millis(wait_cost_ms),
                sends: VecDeque::new(),
                waits: VecDeque::new(),
                offered: Vec::new(),
                accepted: Vec::new(),
                wait_remaining: Vec::new(),
                pending_error_calls: 0,
            }
        }

        fn advance(&self, by: Duration) {
            self.clock.set(self.clock.get() + by);
        }

        fn run(&mut self, bytes: &[u8], deadline_ms: u64) -> io::Result<()> {
            let base = Instant::now();
            let clock = Rc::clone(&self.clock);
            write_with_readiness(
                self,
                bytes,
                base + Duration::from_millis(deadline_ms),
                move || base + clock.get(),
            )
        }
    }

    impl NonblockingSender for FakeSender {
        fn send(&mut self, pending: &[u8]) -> io::Result<usize> {
            self.advance(self.send_cost);
            self.offered.push(pending.to_vec());
            match self.sends.pop_front().unwrap_or(SendStep::WouldBlock) {
                SendStep::Accept(count) => {
                    let count = count.min(pending.len());
                    self.accepted.extend_from_slice(&pending[..count]);
                    Ok(count)
                }
                SendStep::WouldBlock => Err(io::ErrorKind::WouldBlock.into()),
                SendStep::Interrupted => Err(io::ErrorKind::Interrupted.into()),
            }
        }

        fn wait_writable(&mut self, remaining: Duration) -> io::Result<Readiness> {
            self.advance(self.wait_cost);
            self.wait_remaining.push(remaining);
            match self.waits.pop_front().unwrap_or(WaitStep::Expired) {
                WaitStep::Writable => Ok(Readiness::Writable),
                WaitStep::Expired => Ok(Readiness::Expired),
                WaitStep::Interrupted => Ok(Readiness::Interrupted),
                WaitStep::ErrorState => Ok(Readiness::ErrorState),
                WaitStep::Invalid => Err(io::Error::from_raw_os_error(9)),
            }
        }

        fn pending_error(&mut self) -> io::Error {
            self.pending_error_calls += 1;
            io::Error::new(io::ErrorKind::BrokenPipe, "fake pending error")
        }
    }

    #[test]
    fn partial_progress_cannot_refresh_deadline() {
        let mut fake = FakeSender::new(4, 0);
        fake.sends.extend([
            SendStep::Accept(2),
            SendStep::Accept(2),
            SendStep::Accept(2),
            SendStep::Accept(2),
        ]);
        let error = fake.run(b"abcdefgh", 10).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // Sends start at 0, 4 and 8 ms; at 12 ms the deadline has passed.
        assert_eq!(fake.offered.len(), 3);
        assert_eq!(fake.accepted, b"abcdef");
    }

    #[test]
    fn interrupted_send_and_poll_retry_without_extending_deadline() {
        let mut fake = FakeSender::new(1, 1);
        fake.sends.extend([
            SendStep::Interrupted,
            SendStep::Interrupted,
            SendStep::WouldBlock,
        ]);
        fake.waits
            .extend([WaitStep::Interrupted, WaitStep::Interrupted]);
        let error = fake.run(b"frame", 10).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(fake.accepted.is_empty());
        assert!(fake
            .wait_remaining
            .iter()
            .all(|r| *r <= Duration::from_millis(10)));
        assert!(fake.wait_remaining.windows(2).all(|pair| pair[1] < pair[0]));
        assert!(fake.clock.get() <= Duration::from_millis(10));
    }

    #[test]
    fn would_block_waits_only_for_remaining_budget() {
        let mut fake = FakeSender::new(0, 2);
        let error = fake.run(b"frame", 5).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            fake.wait_remaining,
            [
                Duration::from_millis(5),
                Duration::from_millis(3),
                Duration::from_millis(1)
            ]
        );
        // Sends at 0, 2 and 4 ms; the next loop observes 6 ms and stops
        // without a fourth, post-deadline send.
        assert_eq!(fake.offered.len(), 3);
    }

    #[test]
    fn zero_progress_send_is_write_zero_not_a_spin() {
        let mut fake = FakeSender::new(0, 0);
        fake.sends.push_back(SendStep::Accept(0));
        let error = fake.run(b"frame", 1000).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
        assert_eq!(fake.offered.len(), 1);
        assert!(fake.wait_remaining.is_empty());
    }

    #[test]
    fn poll_error_states_exit_without_spinning() {
        // Invalid descriptor: one send, one wait, then the OS error.
        let mut fake = FakeSender::new(0, 0);
        fake.waits.push_back(WaitStep::Invalid);
        let error = fake.run(b"frame", 1000).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(9));
        assert_eq!(fake.offered.len(), 1);
        assert_eq!(fake.wait_remaining.len(), 1);

        // Error state with the next send still blocked: exactly one extra
        // send, no second wait, and the pending error is reported.
        let mut fake = FakeSender::new(0, 0);
        fake.waits.push_back(WaitStep::ErrorState);
        let error = fake.run(b"frame", 1000).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(fake.offered.len(), 2);
        assert_eq!(fake.wait_remaining.len(), 1);
        assert_eq!(fake.pending_error_calls, 1);

        // Error state whose next send surfaces the real result completes.
        let mut fake = FakeSender::new(0, 0);
        fake.sends
            .extend([SendStep::WouldBlock, SendStep::Accept(5)]);
        fake.waits.push_back(WaitStep::ErrorState);
        fake.run(b"frame", 1000).unwrap();
        assert_eq!(fake.pending_error_calls, 0);
    }

    #[test]
    fn no_replay_after_partial() {
        let frame = b"01234567";
        let mut fake = FakeSender::new(0, 6);
        fake.sends.push_back(SendStep::Accept(3));
        fake.waits.push_back(WaitStep::Writable);
        let error = fake.run(frame, 10).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(fake.accepted, &frame[..3]);
        assert_eq!(fake.offered[0], frame);
        assert!(fake.offered.len() >= 2);
        for offer in &fake.offered[1..] {
            assert_eq!(offer.as_slice(), &frame[3..]);
        }
    }

    #[test]
    fn completed_frame_is_ok_even_if_the_clock_passed() {
        let mut fake = FakeSender::new(20, 0);
        fake.sends.push_back(SendStep::Accept(8));
        fake.run(b"01234567", 10).unwrap();
        assert_eq!(fake.accepted, b"01234567");
    }

    #[test]
    fn expired_deadline_never_sends() {
        let mut fake = FakeSender::new(0, 0);
        let error = fake.run(b"frame", 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(fake.offered.is_empty());
    }

    #[test]
    fn poll_timeout_conversion_rounds_up_and_saturates() {
        assert_eq!(poll_timeout_millis(Duration::ZERO), 0);
        assert_eq!(poll_timeout_millis(Duration::from_nanos(1)), 1);
        assert_eq!(poll_timeout_millis(Duration::from_millis(1)), 1);
        assert_eq!(
            poll_timeout_millis(Duration::from_millis(1) + Duration::from_nanos(1)),
            2
        );
        assert_eq!(poll_timeout_millis(Duration::MAX), i32::MAX);
    }

    /// Owned child-process supervision shared by the Linux socket tests and
    /// available wherever Unix process control exists (it uses no socket code).
    #[cfg(unix)]
    mod supervisor {
        use super::*;
        use std::process::{Command, Stdio};

        const CHILD_RETIRE_LIMIT: Duration = Duration::from_secs(10);

        /// Terminates and reaps the child on every exit path. Retirement is
        /// bounded (kill, then `try_wait` until a limit) and proven only by a
        /// reaped exit status. Kill and wait errors are kept and reported; a
        /// failure to retire fails the test loudly, except while already
        /// unwinding, where it is printed instead of causing a second panic.
        pub(super) struct ChildGuard(std::process::Child);

        impl ChildGuard {
            fn retire(&mut self) -> Result<(), String> {
                let mut notes: Vec<String> = Vec::new();
                if let Err(error) = self.0.kill() {
                    notes.push(format!("kill: {error}"));
                }
                let limit = Instant::now() + CHILD_RETIRE_LIMIT;
                loop {
                    match self.0.try_wait() {
                        Ok(Some(_)) => return Ok(()),
                        Ok(None) => {}
                        Err(error) if notes.len() < 8 => notes.push(format!("try_wait: {error}")),
                        Err(_) => {}
                    }
                    if Instant::now() >= limit {
                        return Err(format!(
                            "child {} not proven retired within {CHILD_RETIRE_LIMIT:?}: {}",
                            self.0.id(),
                            notes.join("; ")
                        ));
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }

        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if let Err(message) = self.retire() {
                    eprintln!("SIGPIPE CHILD NOT RETIRED: {message}");
                    if !thread::panicking() {
                        panic!("SIGPIPE child not retired: {message}");
                    }
                }
            }
        }

        pub(super) enum ChildOutcome {
            Exited(std::process::ExitStatus),
            /// The limit passed; the child was killed. Holds whether its
            /// retirement was proven (reaped) or why it was not.
            TimedOut(Result<(), String>),
        }

        /// Runs this test binary as an owned child selecting exactly one test.
        /// The child is owned by a `ChildGuard` from the moment it is spawned,
        /// so every exit path (return, panic, timeout) kills and reaps it. Never
        /// aborts the calling harness.
        pub(super) fn run_owned_child(
            exact_name: &str,
            ignored: bool,
            env: (&str, &str),
            limit: Duration,
            ready: Option<&std::path::Path>,
        ) -> ChildOutcome {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", exact_name, "--nocapture", "--test-threads=1"]);
            if ignored {
                command.arg("--ignored");
            }
            let child = command
                .env(env.0, env.1)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let mut child = ChildGuard(child);
            if let Some(marker) = ready {
                // Separate capped startup wait: the limit below is measured from
                // the child's acknowledgement, not from the spawn.
                let startup = Instant::now() + CHILD_STARTUP_LIMIT;
                while !marker.exists() {
                    match child.0.try_wait() {
                        Ok(Some(status)) => return ChildOutcome::Exited(status),
                        Ok(None) => {}
                        Err(error) => panic!("waiting for child {exact_name} failed: {error}"),
                    }
                    assert!(
                        Instant::now() < startup,
                        "child {exact_name} did not acknowledge start within {CHILD_STARTUP_LIMIT:?}"
                    );
                    thread::sleep(Duration::from_millis(20));
                }
            }
            let limit = Instant::now() + limit;
            loop {
                match child.0.try_wait() {
                    Ok(Some(status)) => return ChildOutcome::Exited(status),
                    Ok(None) if Instant::now() >= limit => {
                        return ChildOutcome::TimedOut(child.retire());
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(20)),
                    Err(error) => panic!("waiting for child {exact_name} failed: {error}"),
                }
            }
        }

        const HANG_CHILD_ENV: &str = "QC_FRAMING_HANG_CHILD";
        const CHILD_STARTUP_LIMIT: Duration = Duration::from_secs(30);

        /// Per-run acknowledgement marker, removed when the parent drops it.
        struct MarkerFile(std::path::PathBuf);

        impl Drop for MarkerFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }

        /// Child entry point that, when selected, writes the marker named by
        /// the environment (proving it entered the loop) and blocks forever.
        #[test]
        #[ignore = "child entry point for the supervisor regression test"]
        fn supervisor_hang_probe() {
            let Ok(marker) = std::env::var(HANG_CHILD_ENV) else {
                return;
            };
            std::fs::write(&marker, b"started").expect("write start marker");
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }

        #[test]
        fn supervisor_kills_and_retires_a_deliberately_hanging_child() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let marker = MarkerFile(
                std::env::temp_dir()
                    .join(format!("qc-framing-hang-{}-{nanos}", std::process::id())),
            );
            let marker_text = marker.0.to_str().expect("utf-8 temp path").to_owned();
            let started = Instant::now();
            // The 1 s hang limit starts when the child acknowledges that it is
            // inside the hang loop; a child that exits without the marker
            // (zero tests matched) is reported as Exited and fails below.
            let outcome = run_owned_child(
                "tests::supervisor::supervisor_hang_probe",
                true,
                (HANG_CHILD_ENV, marker_text.as_str()),
                Duration::from_secs(1),
                Some(&marker.0),
            );
            match outcome {
                ChildOutcome::TimedOut(retired) => {
                    assert_eq!(retired, Ok(()), "retirement was not verified");
                    assert!(marker.0.exists(), "child never acknowledged start");
                }
                ChildOutcome::Exited(status) => {
                    panic!("the hanging child exited by itself: {status:?}")
                }
            }
            assert!(started.elapsed() < Duration::from_secs(45));
        }
    }

    #[cfg(target_os = "linux")]
    mod unix_socket {
        use super::supervisor::{run_owned_child, ChildOutcome};
        use super::*;
        use std::net::Shutdown;
        use std::os::unix::io::AsRawFd;
        use std::os::unix::process::ExitStatusExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        const CHILD_ENV: &str = "QC_FRAMING_SIGPIPE_CHILD";

        fn loopback_pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let writer = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
            let (peer, _) = listener.accept().unwrap();
            (writer, peer)
        }

        /// Fills the send path until `WouldBlock` is sustained. Only used before
        /// any reader exists, so toggling the writer's mode is acceptable here.
        fn prefill_until_would_block(writer: &mut TcpStream) {
            writer.set_nonblocking(true).unwrap();
            let chunk = [0_u8; 64 * 1024];
            let mut total = 0_usize;
            let mut stalls = 0;
            let started = Instant::now();
            while stalls < 3 {
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "prefill setup: no sustained backpressure within 20 s"
                );
                match writer.write(&chunk) {
                    Ok(0) => panic!("prefill setup: zero-progress write"),
                    Ok(count) => {
                        total += count;
                        stalls = 0;
                        assert!(total <= 512 << 20, "prefill never met backpressure");
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        stalls += 1;
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => panic!("prefill failed: {error}"),
                }
            }
            writer.set_nonblocking(false).unwrap();
        }

        fn is_nonblocking(stream: &TcpStream) -> bool {
            // SAFETY: F_GETFL on a descriptor owned by a live TcpStream.
            let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
            assert!(flags >= 0);
            flags & libc::O_NONBLOCK != 0
        }

        fn pattern_frame(body_len: usize) -> Vec<u8> {
            let mut frame = (body_len as u32).to_be_bytes().to_vec();
            frame.extend((0..body_len).map(|index| (index % 251) as u8));
            frame
        }

        /// Shuts the socket down on drop so a panicking test cannot leave a
        /// scoped reader blocked forever.
        struct ShutdownOnDrop(TcpStream);

        impl Drop for ShutdownOnDrop {
            fn drop(&mut self) {
                let _ = self.0.shutdown(Shutdown::Both);
            }
        }

        /// Owned watchdog for one test, started before any setup, prefill,
        /// write or child spawn. On timeout it records a stage marker, shuts
        /// down every registered socket (waking blocked reads and failing
        /// writes) and marks itself fired, so the test body fails normally and
        /// every other guard still runs. It never aborts the shared test
        /// executable, which may be running other tests and their children
        /// concurrently. Drop cancels and joins the thread (none is detached)
        /// and fails the test if the watchdog fired. It is cooperative only: it
        /// cannot stop a hang that ignores socket shutdown, and it is not
        /// evidence that any child retired. The outer process supervisor
        /// (`supervised`) is the enforcement.
        struct Watchdog {
            cancel: Option<mpsc::Sender<()>>,
            stage: Arc<Mutex<&'static str>>,
            sockets: Arc<Mutex<Vec<TcpStream>>>,
            fired: Arc<AtomicBool>,
            test: &'static str,
            thread: Option<thread::JoinHandle<()>>,
        }

        impl Watchdog {
            fn start(test: &'static str, limit: Duration) -> Self {
                let (cancel, cancelled) = mpsc::channel::<()>();
                let stage = Arc::new(Mutex::new("setup"));
                let sockets = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
                let fired = Arc::new(AtomicBool::new(false));
                let thread = {
                    let (stage, sockets, fired) =
                        (Arc::clone(&stage), Arc::clone(&sockets), Arc::clone(&fired));
                    thread::spawn(move || {
                        if let Err(mpsc::RecvTimeoutError::Timeout) = cancelled.recv_timeout(limit)
                        {
                            fired.store(true, Ordering::SeqCst);
                            let stage = stage.lock().map(|stage| *stage).unwrap_or("unknown");
                            eprintln!("WATCHDOG: {test} exceeded {limit:?} in stage '{stage}'");
                            if let Ok(sockets) = sockets.lock() {
                                for socket in sockets.iter() {
                                    let _ = socket.shutdown(Shutdown::Both);
                                }
                            }
                        }
                    })
                };
                Self {
                    cancel: Some(cancel),
                    stage,
                    sockets,
                    fired,
                    test,
                    thread: Some(thread),
                }
            }

            fn stage(&self, stage: &'static str) {
                if let Ok(mut current) = self.stage.lock() {
                    *current = stage;
                }
            }

            /// Registers a duplicate of a socket under test for shutdown on timeout.
            fn watch(&self, socket: &TcpStream) {
                let duplicate = socket.try_clone().expect("duplicate socket for watchdog");
                self.sockets.lock().unwrap().push(duplicate);
            }
        }

        impl Drop for Watchdog {
            fn drop(&mut self) {
                drop(self.cancel.take());
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                if self.fired.load(Ordering::SeqCst) && !thread::panicking() {
                    panic!("watchdog fired for {}", self.test);
                }
            }
        }

        const WATCHDOG_LIMIT: Duration = Duration::from_secs(60);

        /// Cancels a scoped reader and shuts the socket down on drop, so a
        /// panic before the ordinary cleanup cannot make `thread::scope` join a
        /// live reader forever.
        struct StopAndShutdown<'a> {
            stop: &'a AtomicBool,
            socket: TcpStream,
        }

        impl Drop for StopAndShutdown<'_> {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::SeqCst);
                let _ = self.socket.shutdown(Shutdown::Both);
            }
        }

        #[test]
        fn prefilled_socket_write_is_bounded_by_the_absolute_deadline() {
            let Some(_sentinel) =
                supervised("prefilled_socket_write_is_bounded_by_the_absolute_deadline")
            else {
                return;
            };
            let watchdog = Watchdog::start("prefilled_socket_write", WATCHDOG_LIMIT);
            let (mut writer, _peer) = loopback_pair();
            watchdog.watch(&writer);
            watchdog.watch(&_peer);
            watchdog.stage("prefill");
            prefill_until_would_block(&mut writer);
            watchdog.stage("write");
            let started = Instant::now();
            let deadline = started + Duration::from_secs(1);
            let error =
                write_tcp_bytes_before_deadline(&mut writer, &vec![7_u8; 1 << 20], deadline)
                    .unwrap_err();
            let elapsed = started.elapsed();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(elapsed >= Duration::from_secs(1));
            // Slack for scheduling only; not a claim about kernel latency.
            assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
            assert!(!is_nonblocking(&writer));
        }

        #[test]
        fn progress_does_not_restart_the_deadline_when_peer_reads_slowly() {
            let Some(_sentinel) =
                supervised("progress_does_not_restart_the_deadline_when_peer_reads_slowly")
            else {
                return;
            };
            let watchdog = Watchdog::start("slow_reader", WATCHDOG_LIMIT);
            let (mut writer, mut peer) = loopback_pair();
            watchdog.watch(&writer);
            watchdog.watch(&peer);
            peer.set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let stop = AtomicBool::new(false);
            thread::scope(|scope| {
                // Declared before any fallible work: on a panic it drops
                // before the scope joins and ends the reader.
                let _cleanup = StopAndShutdown {
                    stop: &stop,
                    socket: writer.try_clone().unwrap(),
                };
                let reader = scope.spawn(|| {
                    let mut buffer = [0_u8; 4096];
                    while !stop.load(Ordering::SeqCst) {
                        match peer.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(_) => thread::sleep(Duration::from_millis(50)),
                            Err(_) => {}
                        }
                    }
                });
                watchdog.stage("write");
                let started = Instant::now();
                let deadline = started + Duration::from_secs(1);
                let result =
                    write_tcp_bytes_before_deadline(&mut writer, &vec![1_u8; 32 << 20], deadline);
                let elapsed = started.elapsed();
                stop.store(true, Ordering::SeqCst);
                let _ = writer.shutdown(Shutdown::Both);
                reader.join().unwrap();
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
                assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
            });
        }

        #[test]
        fn closed_peer_surfaces_error_without_failing_the_process() {
            let Some(_sentinel) =
                supervised("closed_peer_surfaces_error_without_failing_the_process")
            else {
                return;
            };
            let watchdog = Watchdog::start("closed_peer", WATCHDOG_LIMIT);
            let (mut writer, peer) = loopback_pair();
            watchdog.watch(&writer);
            drop(peer);
            let mut outcome = None;
            for _ in 0..100 {
                let deadline = Instant::now() + Duration::from_secs(2);
                if let Err(error) =
                    write_tcp_bytes_before_deadline(&mut writer, &[0_u8; 1024], deadline)
                {
                    outcome = Some(error);
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let error = outcome.expect("a closed peer must surface an error");
            assert!(
                matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                ),
                "unexpected kind: {error:?}"
            );
        }

        #[test]
        fn partial_write_then_deadline_keeps_exact_prefix() {
            let Some(_sentinel) = supervised("partial_write_then_deadline_keeps_exact_prefix")
            else {
                return;
            };
            let watchdog = Watchdog::start("exact_prefix", WATCHDOG_LIMIT);
            let (mut writer, mut peer) = loopback_pair();
            watchdog.watch(&writer);
            watchdog.watch(&peer);
            let body_len = 32 << 20;
            let frame = pattern_frame(body_len);
            let body = &frame[4..];
            watchdog.stage("write");
            let deadline = Instant::now() + Duration::from_millis(500);
            let error =
                write_tcp_frame_before_deadline(&mut writer, body, body_len, deadline).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            // The caller's fatal policy: shut the connection down, no replay.
            writer.shutdown(Shutdown::Write).unwrap();
            watchdog.stage("peer drain");
            peer.set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut received = Vec::new();
            peer.read_to_end(&mut received).unwrap();
            assert!(!received.is_empty());
            assert!(received.len() < frame.len());
            assert_eq!(received.as_slice(), &frame[..received.len()]);
        }

        #[test]
        fn cloned_blocking_reader_survives_a_deadline_bounded_write() {
            let Some(_sentinel) =
                supervised("cloned_blocking_reader_survives_a_deadline_bounded_write")
            else {
                return;
            };
            let watchdog = Watchdog::start("cloned_reader_survives", WATCHDOG_LIMIT);
            let (mut writer, mut peer) = loopback_pair();
            watchdog.watch(&writer);
            watchdog.watch(&peer);
            watchdog.stage("prefill");
            prefill_until_would_block(&mut writer);
            let mut reader = writer.try_clone().unwrap();
            let (result_tx, result_rx) = mpsc::channel();
            let (started_tx, started_rx) = mpsc::channel();
            thread::scope(|scope| {
                let _guard = ShutdownOnDrop(writer.try_clone().unwrap());
                scope.spawn(move || {
                    let mut byte = [0_u8; 1];
                    let _ = started_tx.send(());
                    let _ = result_tx.send(reader.read(&mut byte).map(|count| (count, byte[0])));
                });
                // Rendezvous: the reader has started and (after a short settle)
                // is blocked in read() before the write begins, so a temporary
                // shared-mode change during the write would surface as an early
                // reader result. The settle delay is best-effort, not a proof
                // that the reader is already inside the syscall.
                started_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("reader thread started");
                thread::sleep(Duration::from_millis(100));
                watchdog.stage("write");
                let deadline = Instant::now() + Duration::from_millis(300);
                let error =
                    write_tcp_bytes_before_deadline(&mut writer, &vec![3_u8; 1 << 20], deadline)
                        .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert!(matches!(
                    result_rx.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
                assert!(!is_nonblocking(&writer));
                peer.write_all(&[9]).unwrap();
                let received = result_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("reader still blocked and woken by the peer")
                    .unwrap();
                assert_eq!(received, (1, 9));
            });
        }

        #[test]
        fn explicit_fatal_shutdown_after_a_failed_write_ends_the_cloned_reader() {
            let Some(_sentinel) =
                supervised("explicit_fatal_shutdown_after_a_failed_write_ends_the_cloned_reader")
            else {
                return;
            };
            let watchdog = Watchdog::start("fatal_shutdown_ends_reader", WATCHDOG_LIMIT);
            let (mut writer, _peer) = loopback_pair();
            watchdog.watch(&writer);
            watchdog.watch(&_peer);
            watchdog.stage("prefill");
            prefill_until_would_block(&mut writer);
            let mut reader = writer.try_clone().unwrap();
            let (result_tx, result_rx) = mpsc::channel();
            thread::scope(|scope| {
                let _guard = ShutdownOnDrop(writer.try_clone().unwrap());
                scope.spawn(move || {
                    let mut byte = [0_u8; 1];
                    let _ = result_tx.send(reader.read(&mut byte));
                });
                watchdog.stage("write");
                let deadline = Instant::now() + Duration::from_millis(200);
                let error =
                    write_tcp_bytes_before_deadline(&mut writer, &vec![3_u8; 1 << 20], deadline)
                        .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                writer.shutdown(Shutdown::Both).unwrap();
                // EOF or an error both end the reader; it never needs a byte.
                let _ = result_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("shutdown ends the blocked reader");
            });
        }

        /// Entry point re-executed by the supervised SIGPIPE tests below. It
        /// does nothing unless the parent set the mode variable. The process
        /// exit code (or death by signal) is what the parent reads.
        #[test]
        #[ignore = "child entry point for the supervised SIGPIPE tests"]
        fn sigpipe_child_probe() {
            let Ok(mode) = std::env::var(CHILD_ENV) else {
                return;
            };
            // SAFETY: this child process owns its signal dispositions; the
            // main harness never reaches this code.
            unsafe {
                assert_ne!(libc::signal(libc::SIGPIPE, libc::SIG_DFL), libc::SIG_ERR);
                let mut action: libc::sigaction = std::mem::zeroed();
                assert_eq!(
                    libc::sigaction(libc::SIGPIPE, std::ptr::null(), &mut action),
                    0
                );
                assert_eq!(action.sa_sigaction, libc::SIG_DFL);
            }
            let (mut writer, _peer) = loopback_pair();
            // A send after a local write shutdown fails with EPIPE
            // deterministically, which is the signal-raising path.
            writer.shutdown(Shutdown::Write).unwrap();
            let bytes = [1_u8; 16];
            let deadline = Instant::now() + Duration::from_secs(5);
            match mode.as_str() {
                "protected" => {
                    let error =
                        write_tcp_bytes_before_deadline(&mut writer, &bytes, deadline).unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                    std::process::exit(42);
                }
                "unprotected" => {
                    let _ = unix_send::write_fd_before_deadline(
                        writer.as_raw_fd(),
                        &bytes,
                        deadline,
                        libc::MSG_DONTWAIT,
                    );
                    // Surviving means the setup cannot detect SIGPIPE.
                    std::process::exit(43);
                }
                other => panic!("unknown child mode {other}"),
            }
        }

        fn run_sigpipe_child(mode: &str) -> std::process::ExitStatus {
            match run_owned_child(
                "tests::unix_socket::sigpipe_child_probe",
                true,
                (CHILD_ENV, mode),
                Duration::from_secs(30),
                None,
            ) {
                ChildOutcome::Exited(status) => status,
                ChildOutcome::TimedOut(retired) => {
                    panic!("SIGPIPE child did not finish in 30 s (retirement: {retired:?})")
                }
            }
        }

        const SOCKET_CHILD_ENV: &str = "QC_FRAMING_SOCKET_CHILD";
        /// Distinct from 0 and from the SIGPIPE child's 42/43, so a child that
        /// matched zero tests (exit 0) or panicked (101) can never pass.
        const SOCKET_CHILD_SENTINEL: i32 = 44;
        const SOCKET_SUPERVISOR_LIMIT: Duration = Duration::from_secs(90);

        /// Exits the child process with the success sentinel when the test body
        /// ends normally (declared first in the body, so it drops last).
        struct SentinelExit;

        impl Drop for SentinelExit {
            fn drop(&mut self) {
                if !thread::panicking() {
                    std::process::exit(SOCKET_CHILD_SENTINEL);
                }
            }
        }

        /// Runs a socket test under an owned outer process supervisor. In the
        /// parent it re-executes the test binary with the exact test name, waits
        /// at most `SOCKET_SUPERVISOR_LIMIT`, kills and reaps a hung child, and
        /// requires the sentinel exit code; it returns `None` and the test body
        /// is skipped. In the child it returns the guard and the body runs. The
        /// cooperative `Watchdog` stays as an inner aid only.
        fn supervised(name: &'static str) -> Option<SentinelExit> {
            if let Ok(selected) = std::env::var(SOCKET_CHILD_ENV) {
                assert_eq!(selected, name, "child selected an unexpected test");
                return Some(SentinelExit);
            }
            let exact = format!("tests::unix_socket::{name}");
            match run_owned_child(
                &exact,
                false,
                (SOCKET_CHILD_ENV, name),
                SOCKET_SUPERVISOR_LIMIT,
                None,
            ) {
                ChildOutcome::Exited(status) => assert_eq!(
                    status.code(),
                    Some(SOCKET_CHILD_SENTINEL),
                    "{exact} child did not report the success sentinel: {status:?}"
                ),
                ChildOutcome::TimedOut(retired) => {
                    panic!("{exact} hung past {SOCKET_SUPERVISOR_LIMIT:?}; child retirement: {retired:?}")
                }
            }
            None
        }

        #[test]
        fn sigpipe_protected_child_gets_broken_pipe_under_default_disposition() {
            // Armed before the spawn and dropped after the child guard's cleanup.
            let _watchdog = Watchdog::start("sigpipe_protected", WATCHDOG_LIMIT);
            let status = run_sigpipe_child("protected");
            assert_eq!(status.code(), Some(42), "status: {status:?}");
        }

        #[test]
        fn sigpipe_negative_control_child_is_killed_without_per_call_suppression() {
            let _watchdog = Watchdog::start("sigpipe_negative_control", WATCHDOG_LIMIT);
            let status = run_sigpipe_child("unprotected");
            assert_eq!(status.signal(), Some(libc::SIGPIPE), "status: {status:?}");
        }
    }
}
