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
    /// Internal admission evidence, never accepted from a wire caller.
    #[serde(skip)]
    pub(super) captured_target_generation: Option<(u64, u64)>,
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

/// The answer that goes out in place of a result too large for one IPC frame
/// (DB-2). Such a result is an answer, not a transport failure: a backup
/// document can be larger than a frame, and treating that as a write error
/// closed the client's socket (or ended a `--stdio` broker and its USB
/// session) after the device work had already finished. Only a response that
/// carries a `result` is replaced; a notification or an error has nothing to
/// answer with and still fails the write.
pub(super) fn result_too_large(response: &Value) -> Option<Value> {
    let id = response.get("id")?;
    response.get("result")?;
    Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": HANDLER_ERROR_CODE,
            "message": format!(
                "Result exceeds the {} MiB IPC frame limit",
                quadcortex::domain::IPC_MAX_FRAME_BYTES / (1024 * 1024)
            ),
            "data": {"code": "RESULT_TOO_LARGE", "retryable": false}
        }
    }))
}

/// The serialized frame body for `response`: the response itself, or the
/// [`result_too_large`] answer when it does not fit.
pub(super) fn frame_body(response: &Value) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if body.len() <= quadcortex::domain::IPC_MAX_FRAME_BYTES {
        return Ok(body);
    }
    match result_too_large(response) {
        Some(replacement) => serde_json::to_vec(&replacement).map_err(|error| error.to_string()),
        None => Err("Native broker response exceeds the IPC frame limit".into()),
    }
}

pub(super) fn write_response(output: &mut impl Write, response: &Value) -> Result<(), String> {
    let body = frame_body(response)?;
    qc_broker_framing::write_frame(output, &body, quadcortex::domain::IPC_MAX_FRAME_BYTES)
        .map_err(|error| error.to_string())
}

/// One TCP frame has one budget across all partial writes. A peer that stops
/// reading cannot keep a writer mutex and scoped connection threads forever.
pub(super) fn write_tcp_response(output: &mut TcpStream, response: &Value) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let body = frame_body(response)?;
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

#[cfg(test)]
mod demo_envelope_tests {
    use super::*;

    #[test]
    fn demo_target_envelope_rejects_partial_null_unsafe_or_foreign_fields() {
        let target = json!({"source":"demo","sessionId":"broker:demo:window","generation":1});
        let request = |target: Value| json!({"jsonrpc":"2.0","id":1,"method":"device.snapshot","params":{},"expectedBrokerTarget":target});
        let parsed: Request = serde_json::from_value(request(target.clone())).unwrap();
        assert_eq!(
            parsed.expected_broker_target.unwrap().source,
            crate::target::TargetSource::Demo
        );
        let legacy: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"device.snapshot"}))
                .unwrap();
        assert!(legacy.expected_broker_target.is_none());
        assert!(!legacy.physical_only);
        for bad in [
            Value::Null,
            json!({}),
            json!({"source":"virtual","sessionId":"id","generation":1}),
            json!({"source":"demo","sessionId":" ","generation":1}),
            json!({"source":"demo","sessionId":"x".repeat(513),"generation":1}),
            json!({"source":"demo","sessionId":"id","generation":0}),
            json!({"source":"demo","sessionId":"id","generation":1.5}),
            json!({"source":"demo","sessionId":"id","generation":9007199254740992_u64}),
            json!({"source":"demo","sessionId":"id","generation":1,"extra":true}),
        ] {
            assert!(
                serde_json::from_value::<Request>(request(bad.clone())).is_err(),
                "accepted {bad}"
            );
        }
    }
}

#[cfg(test)]
mod oversize_result_tests {
    use super::*;

    fn oversize() -> Value {
        json!({"jsonrpc":"2.0","id":41,"result":{"document":"x".repeat(quadcortex::domain::IPC_MAX_FRAME_BYTES)}})
    }

    fn assert_too_large(answer: &Value) {
        assert_eq!(answer["id"], 41);
        assert!(answer.get("result").is_none());
        assert_eq!(answer["error"]["code"], HANDLER_ERROR_CODE);
        assert_eq!(answer["error"]["data"]["code"], "RESULT_TOO_LARGE");
        assert_eq!(answer["error"]["data"]["retryable"], false);
        assert!(answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("16 MiB"));
    }

    /// DB-2: a result that does not fit one frame is answered with an error
    /// for the same request, on every transport, and the connection survives.
    #[test]
    fn an_oversize_result_is_answered_with_an_error_for_the_same_request() {
        let mut stream = Vec::new();
        write_response(&mut stream, &oversize()).expect("an answer, not a write failure");
        let answer = read_frame_bytes(&mut stream.as_slice()).unwrap().unwrap();
        assert_too_large(&serde_json::from_slice(&answer).unwrap());
        // The same writer is still good for the next, ordinary response.
        write_response(&mut stream, &json!({"jsonrpc":"2.0","id":42,"result":true})).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write_tcp_response(&mut server, &oversize()).expect("an answer, not a write failure");
        let answer = read_frame_bytes(&mut client).unwrap().unwrap();
        assert_too_large(&serde_json::from_slice(&answer).unwrap());
        write_tcp_response(&mut server, &json!({"jsonrpc":"2.0","id":42,"result":true})).unwrap();
        let next = read_frame_bytes(&mut client).unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&next).unwrap()["id"], 42);
    }

    /// A notification or an error has nothing to answer with, so an oversize
    /// one still fails the write rather than being silently replaced.
    #[test]
    fn an_oversize_notification_still_fails_the_write() {
        let huge = "x".repeat(quadcortex::domain::IPC_MAX_FRAME_BYTES);
        let notification =
            json!({"jsonrpc":"2.0","method":"device.stateFrame","params":{"frame":huge}});
        assert!(write_response(&mut Vec::new(), &notification).is_err());
        assert!(result_too_large(&notification).is_none());
        let failure = json!({"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":huge}});
        assert!(write_response(&mut Vec::new(), &failure).is_err());
    }
}
