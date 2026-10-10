//! Shared broker transport: no Tauri dependency.
use crate::device_broker_descriptor;
use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};
pub const DEVICE_BROKER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
pub const DEVICE_BROKER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEVICE_BROKER_AUTHENTICATE_METHOD: &str = "broker.authenticate";
/// One deadline shared by all reads and writes, including partial frames.
/// Per-call socket timeouts alone allow a dribbling peer to extend a handshake.
pub struct DeadlineStream<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl<'a> DeadlineStream<'a> {
    pub fn new(stream: &'a mut TcpStream, budget: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + budget,
        }
    }
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "Broker response deadline expired")
            })
    }
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// One 4-byte big-endian length-prefixed JSON frame, from a broker's stdout
/// pipe or its TCP socket alike.
pub fn read_frame<R: Read + ?Sized>(reader: &mut R) -> Result<Value, String> {
    let payload = qc_broker_framing::read_frame(reader, quadcortex::domain::IPC_MAX_FRAME_BYTES)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                error
                    .to_string()
                    .strip_prefix("invalid frame length: ")
                    .map(|length| format!("Gateway returned invalid frame length: {length}"))
                    .unwrap_or_else(|| error.to_string())
            } else if error.kind() == std::io::ErrorKind::UnexpectedEof {
                error.to_string()
            } else {
                format!("Gateway closed: {error}")
            }
        })?
        .ok_or_else(|| "Gateway closed before sending a frame".to_string())?;
    serde_json::from_slice(&payload).map_err(|error| error.to_string())
}

/// The writing half of [`read_frame`].
pub fn write_frame<W: Write + ?Sized>(writer: &mut W, payload: &[u8]) -> Result<(), String> {
    qc_broker_framing::write_frame(writer, payload, quadcortex::domain::IPC_MAX_FRAME_BYTES)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                "Gateway request is too large".to_string()
            } else {
                error.to_string()
            }
        })
}

/// Why a published broker could not be attached to.
///
/// The two cases lead to opposite actions, so they must never be merged. A
/// broker that is `Gone` can be replaced. A broker that is `Unresponsive` must
/// not be: it may simply be slow (warming up, or busy with a long call), and
/// spawning a second broker beside a live one would put two processes on the
/// one USB device.
#[derive(Debug)]
pub enum BrokerAttachFailure {
    /// The connect was refused, so nothing listens on the published port, or
    /// the published process no longer exists - its pid is free, or is now
    /// held by a process created at a different time than the one recorded
    /// in the descriptor. Only these signals are accepted as proof that the
    /// broker is dead.
    Gone(String),
    /// Anything else - a handshake that timed out, a connect that timed out, a
    /// malformed or unexpected answer - while the published process is still
    /// running.
    Unresponsive(String),
}

impl BrokerAttachFailure {
    pub fn message(&self) -> &str {
        match self {
            Self::Gone(message) | Self::Unresponsive(message) => message,
        }
    }

    /// A failure other than a refused connect. It proves nothing about the
    /// broker, so whether the broker is gone depends only on whether its
    /// process still exists.
    ///
    /// On Windows a connect to a closed loopback port is retried for about
    /// two seconds before it is refused, far past the connect timeout, so a
    /// dead broker's port almost always lands here as a timeout. This check
    /// is therefore what decides nearly every `Gone`.
    fn ambiguous(
        descriptor: &device_broker_descriptor::DeviceBrokerDescriptor,
        message: String,
    ) -> Self {
        if published_broker_process_is_alive(descriptor) {
            Self::Unresponsive(message)
        } else {
            Self::Gone(message)
        }
    }
}

/// Whether the very process that published this descriptor is still running.
///
/// A live pid is not enough: Windows reuses process ids, and a broker that
/// died without releasing its descriptor can leave it naming a pid that now
/// belongs to an unrelated process. Trusting the pid alone would read that
/// descriptor as a live, unanswering broker forever, and nothing would ever
/// replace it. So the pid's current creation time must also equal the one the
/// broker recorded when it published.
pub fn published_broker_process_is_alive(
    descriptor: &device_broker_descriptor::DeviceBrokerDescriptor,
) -> bool {
    published_process_is_alive_with(
        descriptor.process_id,
        descriptor.process_created_at,
        os_process_is_alive,
        os_process_created_at,
    )
}

/// `published_broker_process_is_alive` with the two OS probes injected, so a
/// probe failure can be tested.
///
/// Errs towards "alive" wherever the answer is unclear: a descriptor that
/// recorded no creation time, or a creation time the OS will not report,
/// falls back to the pid check alone. Only a positive mismatch - the pid is
/// held by a process created at another time - proves the broker gone.
pub fn published_process_is_alive_with(
    pid: u32,
    recorded_created_at: Option<u64>,
    is_alive: impl Fn(u32) -> bool,
    created_at: impl Fn(u32) -> Option<u64>,
) -> bool {
    if !is_alive(pid) {
        return false;
    }
    match (recorded_created_at, created_at(pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => true,
    }
}

// `os_process_created_at`/`os_process_is_alive` live in `qc-windows-process`,
// shared with `services/device-broker`'s `orphan_guard.rs`, which needs the
// identical logic for its own parent-liveness judgment. The two copies had
// already drifted cosmetically despite identical logic - the real risk a
// behavioral fix landing in only one of them would realize.
use qc_windows_process::{os_process_created_at, os_process_is_alive};

/// Why the token must not be sent to whatever accepted the connection, if so.
///
/// The OS names the process that owns the listening socket. It must be the
/// published broker, and that broker must still be the process it recorded.
/// When the TCP table cannot be read, the published process being alive is
/// the remaining evidence: a live broker still holds its own port.
fn listener_refusal(
    descriptor: &device_broker_descriptor::DeviceBrokerDescriptor,
    owner: Result<Option<u32>, String>,
    published_process_alive: bool,
) -> Option<String> {
    if !published_process_alive {
        return Some(
            "The published QC device broker is no longer running; its port was not trusted".into(),
        );
    }
    match owner {
        Ok(Some(pid)) if pid == descriptor.process_id => None,
        Ok(Some(pid)) => Some(format!(
            "Process {pid}, not the published QC device broker, is listening on its port"
        )),
        Ok(None) => Some("Nothing is listening on the published QC device broker port".into()),
        Err(_) => None,
    }
}

/// Connect to a published broker and complete its `broker.authenticate`
/// handshake. Success requires the broker to answer with the very
/// `instanceId` the descriptor names - the same live-owner confirmation
/// `local_access::live_descriptor_owner` requires of an MCP descriptor - so a
/// port that some other process reused after the broker died is never
/// mistaken for it.
pub fn attach_to_device_broker(
    descriptor: &device_broker_descriptor::DeviceBrokerDescriptor,
) -> Result<TcpStream, BrokerAttachFailure> {
    let address = SocketAddr::from(([127, 0, 0, 1], descriptor.port));
    let mut stream =
        TcpStream::connect_timeout(&address, DEVICE_BROKER_CONNECT_TIMEOUT).map_err(|error| {
            let message = format!("Could not connect to the shared QC device broker: {error}");
            if error.kind() == std::io::ErrorKind::ConnectionRefused {
                BrokerAttachFailure::Gone(message)
            } else {
                BrokerAttachFailure::ambiguous(descriptor, message)
            }
        })?;
    let ambiguous = |message: String| BrokerAttachFailure::ambiguous(descriptor, message);
    stream.set_nodelay(true).map_err(|error| {
        ambiguous(format!(
            "Could not configure the shared broker connection: {error}"
        ))
    })?;
    stream
        .set_read_timeout(Some(DEVICE_BROKER_HANDSHAKE_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(DEVICE_BROKER_HANDSHAKE_TIMEOUT)))
        .map_err(|error| {
            ambiguous(format!(
                "Could not configure the shared broker connection: {error}"
            ))
        })?;
    // The token goes only to the process that published it. A broker that
    // died without removing its descriptor leaves the port free for anyone;
    // checking the answer's instanceId afterwards would already have handed
    // that squatter the token.
    if let Some(refusal) = listener_refusal(
        descriptor,
        qc_windows_process::loopback_listener_owner(descriptor.port),
        published_broker_process_is_alive(descriptor),
    ) {
        return Err(ambiguous(refusal));
    }
    let handshake = serde_json::to_vec(&json!({
        "method": DEVICE_BROKER_AUTHENTICATE_METHOD,
        "params": {"token": descriptor.bearer_token},
        "id": 0
    }))
    .map_err(|error| ambiguous(error.to_string()))?;
    let mut bounded = DeadlineStream::new(&mut stream, DEVICE_BROKER_HANDSHAKE_TIMEOUT);
    write_frame(&mut bounded, &handshake).map_err(|error| {
        ambiguous(format!(
            "Could not authenticate with the shared QC device broker: {error}"
        ))
    })?;
    let response = read_frame(&mut bounded).map_err(|error| {
        ambiguous(format!(
            "Could not authenticate with the shared QC device broker: {error}"
        ))
    })?;
    let answered_instance = response
        .get("result")
        .and_then(|result| result.get("instanceId"))
        .and_then(Value::as_str);
    if response.get("id").and_then(Value::as_u64) != Some(0)
        || answered_instance != Some(descriptor.instance_id.as_str())
    {
        let detail = response
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("it did not confirm the published instance");
        return Err(ambiguous(format!(
            "The shared QC device broker rejected this connection: {detail}"
        )));
    }
    // Requests may legitimately take minutes (a backup), and the reader thread
    // blocks here between pushes; only the handshake is bounded.
    stream
        .set_read_timeout(None)
        .and_then(|()| stream.set_write_timeout(None))
        .map_err(|error| {
            ambiguous(format!(
                "Could not configure the shared broker connection: {error}"
            ))
        })?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> device_broker_descriptor::DeviceBrokerDescriptor {
        device_broker_descriptor::DeviceBrokerDescriptor {
            port: 54123,
            bearer_token: "t".into(),
            instance_id: "broker".into(),
            process_id: 4242,
            process_created_at: Some(1),
            published_at_unix_ms: 1,
            gateway_api_version: None,
        }
    }

    #[test]
    fn the_token_goes_only_to_the_live_published_broker() {
        let published = descriptor();
        assert_eq!(listener_refusal(&published, Ok(Some(4242)), true), None);
        // A squatter on a dead broker's port, or any other listener.
        assert!(listener_refusal(&published, Ok(Some(7)), true).is_some());
        assert!(listener_refusal(&published, Ok(Some(4242)), false).is_some());
        assert!(listener_refusal(&published, Ok(None), true).is_some());
        // An unreadable table leaves the liveness check as the evidence.
        assert_eq!(listener_refusal(&published, Err("x".into()), true), None);
        assert!(listener_refusal(&published, Err("x".into()), false).is_some());
    }

    #[test]
    fn a_squatting_listener_is_refused_before_any_byte_is_sent() {
        use std::io::Read as _;
        // This process listens, but the descriptor names another pid.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut published = descriptor();
        published.port = listener.local_addr().unwrap().port();
        published.process_id = std::process::id().wrapping_add(1);
        published.process_created_at = None;
        let accepting = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut received = Vec::new();
            let _ = stream.read_to_end(&mut received);
            received
        });
        let failure = attach_to_device_broker(&published).expect_err("a squatter is refused");
        assert!(
            failure.message().contains("not the published"),
            "{}",
            failure.message()
        );
        assert!(accepting.join().unwrap().is_empty(), "the token was sent");
    }
}
