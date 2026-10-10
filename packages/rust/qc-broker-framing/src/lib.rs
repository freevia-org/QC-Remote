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

/// Writes one TCP frame under one absolute deadline, recomputing the socket
/// timeout before each partial write and restoring the previous timeout.
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

/// Writes already-framed TCP bytes under an absolute deadline and restores
/// the socket's prior write timeout.
pub fn write_tcp_bytes_before_deadline(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;
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

    #[test]
    fn tcp_read_uses_one_deadline_across_partial_header_and_restores_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (first_byte_tx, first_byte_rx) = mpsc::channel();
        let (first_byte_sent_tx, first_byte_sent_rx) = mpsc::channel();
        let (read_finished_tx, read_finished_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
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
        let mut client = TcpStream::connect(address).unwrap();
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
    }
}
