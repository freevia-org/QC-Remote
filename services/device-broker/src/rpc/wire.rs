use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub(super) jsonrpc: String,
    pub(super) id: Value,
    pub(super) method: String,
    #[serde(default = "empty_params")]
    pub(super) params: Value,
    /// Who asked, when a client says so. Only [`RESOURCE_READ_ORIGIN`] means
    /// anything: an MCP resource read, which the broker serves only from a
    /// device session that is open and never opens, reopens or reconnects
    /// one for (`dispatch_resource_read`). Absent, or any other value, the
    /// request is handled exactly as before. It can only narrow a request.
    /// A client sends it only to a broker that lists
    /// [`RESOURCE_READ_ORIGIN_CAPABILITY`] in `system.status`: an older
    /// broker rejects unknown envelope fields.
    #[serde(default)]
    pub(super) origin: Option<String>,
    #[serde(default, rename = "physicalOnly")]
    pub(super) physical_only: bool,
    #[serde(
        default,
        rename = "expectedBrokerTarget",
        deserialize_with = "deserialize_expected_target"
    )]
    pub(super) expected_broker_target: Option<crate::target::TargetIdentity>,
}

pub(super) fn parse_target_identity(value: Value) -> Result<crate::target::TargetIdentity, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Target {
        source: crate::target::TargetSource,
        session_id: String,
        generation: u64,
    }
    let target: Target = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if target.session_id.trim().is_empty()
        || target.session_id.chars().count() > 512
        || !(1..=9_007_199_254_740_991).contains(&target.generation)
    {
        return Err("Invalid broker target session id or generation".into());
    }
    Ok(crate::target::TargetIdentity {
        source: target.source,
        session_id: target.session_id,
        generation: target.generation,
    })
}

fn deserialize_expected_target<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<crate::target::TargetIdentity>, D::Error> {
    parse_target_identity(Value::deserialize(deserializer)?)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// The `origin` value that marks an MCP resource read.
pub(super) const RESOURCE_READ_ORIGIN: &str = "resourceRead";
/// Listed in `system.status` capabilities by a broker that honours
/// [`RESOURCE_READ_ORIGIN`].
pub(super) const RESOURCE_READ_ORIGIN_CAPABILITY: &str = "resourceReadOrigin";

fn empty_params() -> Value {
    json!({})
}

pub(super) fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Reads one 4-byte-length-prefixed frame body off `input`, or `Ok(None)` on
/// a clean EOF before any byte of a new frame arrives.
///
/// This is the byte-level half of the protocol shared by every transport
/// (`--stdio` and the TCP `--broker-serve` clients alike): `read_request`
/// parses the body as a JSON-RPC `Request`, and the TCP auth handshake
/// (`authenticate_tcp_client`) parses the same bytes as a bare `Value`
/// instead, since the handshake frame omits `jsonrpc` and carries `id: 0`,
/// which `Request` rejects.
pub(super) fn read_frame_bytes(input: &mut impl Read) -> Result<Option<Vec<u8>>, String> {
    read_frame_bytes_limited(input, quadcortex::domain::IPC_MAX_FRAME_BYTES)
}

/// `read_frame_bytes` with an explicit body-size ceiling. The declared
/// length is checked before any body buffer is allocated.
pub(super) fn read_frame_bytes_limited(
    input: &mut impl Read,
    max_frame_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    qc_broker_framing::read_frame(input, max_frame_bytes).map_err(|error| match error.kind() {
        io::ErrorKind::InvalidData => error
            .to_string()
            .strip_prefix("invalid frame length: ")
            .map(|length| format!("Invalid native broker frame length: {length}"))
            .unwrap_or_else(|| format!("Invalid native broker frame: {error}")),
        io::ErrorKind::UnexpectedEof if error.to_string().contains("header") => {
            "Incomplete native broker frame header".into()
        }
        io::ErrorKind::UnexpectedEof => format!("Incomplete native broker frame: {error}"),
        _ => format!("Could not read native broker input: {error}"),
    })
}

pub(super) fn read_request(input: &mut impl Read) -> Result<Option<Request>, String> {
    let Some(body) = read_frame_bytes(input)? else {
        return Ok(None);
    };
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("Invalid native broker JSON: {error}"))
}

pub(super) fn write_response(output: &mut impl Write, response: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if body.len() > quadcortex::domain::IPC_MAX_FRAME_BYTES {
        return Err("Native broker response exceeds the IPC frame limit".into());
    }
    qc_broker_framing::write_frame(output, &body, quadcortex::domain::IPC_MAX_FRAME_BYTES)
        .map_err(|error| error.to_string())
}

/// One TCP frame has one budget across all partial writes. A peer that stops
/// reading cannot keep a writer mutex and scoped connection threads forever.
pub(super) fn write_tcp_response(output: &mut TcpStream, response: &Value) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let body = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if body.len() > quadcortex::domain::IPC_MAX_FRAME_BYTES {
        return Err("Native broker response exceeds the IPC frame limit".into());
    }
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    write_tcp_frame_before_deadline(output, &frame, deadline).map_err(|error| error.to_string())
}

/// Finish an observer action before making its response visible. The reader
/// may admit the next request as soon as it receives the first response byte,
/// so release the single-action latch while holding the writer lock and before
/// writing any bytes.
#[cfg(test)]
pub(super) fn write_observer_action_response<W: Write>(
    output: &Mutex<W>,
    action_pending: &AtomicBool,
    response: &Value,
    on_write_error: impl FnOnce(&mut W),
) {
    write_observer_action_response_with(
        output,
        action_pending,
        response,
        write_response,
        on_write_error,
    );
}

pub(super) fn write_observer_action_response_with<W: Write>(
    output: &Mutex<W>,
    action_pending: &AtomicBool,
    response: &Value,
    write: impl FnOnce(&mut W, &Value) -> Result<(), String>,
    on_write_error: impl FnOnce(&mut W),
) {
    let mut output = output.lock_recover();
    action_pending.store(false, Ordering::Release);
    if write(&mut *output, response).is_err() {
        on_write_error(&mut *output);
    }
}

/// A shutdown broadcast has one absolute deadline shared by all sockets.
/// `write_all` cannot enforce it: each partial write could consume the same
/// timeout again. Recompute the remaining budget before every system call.
pub(super) fn write_tcp_frame_before_deadline(
    output: &mut TcpStream,
    frame: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    qc_broker_framing::write_tcp_bytes_before_deadline(output, frame, deadline)
}
