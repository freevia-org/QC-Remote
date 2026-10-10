// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
/// Shared relay boundary response text.
pub const INVALID_REQUEST_TEXT: &str = "Invalid relay request.";
/// Shared relay boundary response text.
pub const INVALID_REQUEST_ID_TEXT: &str = "The request identifier is invalid.";
/// Shared relay boundary response text.
pub const REPLAYED_REQUEST_TEXT: &str = "This request identifier was already processed.";
/// Shared relay boundary response text.
pub const METHOD_NOT_ALLOWED_TEXT: &str =
    "The requested method is not in the generated remote action contract.";
/// Shared relay boundary response text.
pub const ACCESS_MODE_RESTRICTED_TEXT: &str =
    "The requested operation is outside this host's remote access mode.";
/// Shared relay boundary response text.
pub const MESSAGE_TOO_LARGE_TEXT: &str = "message too large";
/// Shared relay boundary response text.
pub const BINARY_UNSUPPORTED_TEXT: &str = "binary messages are not supported";
pub const PROTOCOL_VERSION: &str = "qc-relay.v1";
pub const DEVICE_PAIR_PATH: &str = "/v1/device/pair";
pub const DEVICE_REPLACEMENT_PATH: &str = "/v1/device/pair/replace-v1";
pub const REPLACEMENT_ACK_QUERY_PARAMETER: &str = "replacementAck";
pub const REPLACEMENT_ACK_FRAME_TYPE: &str = "replacement_ack";
pub const ACCOUNT_DELETION_WEB_PATH: &str = "/account/delete";
pub const ACCOUNT_DELETION_INFORMATION_URL: &str = "https://freevia.org/qcremote/account/delete/";
pub const DEVICE_CONNECT_PATH: &str = "/v1/device/connect";
pub const DEVICE_REVOKE_PATH: &str = "/v1/device/revoke";
pub const DEVICE_ROSTER_PATH: &str = "/v1/device/roster";
pub const DEVICE_CONNECTORS_PATH: &str = "/v1/device/connectors";
pub const CREDENTIAL_TTL_DAYS: u32 = 90;
pub const MAXIMUM_INVOKE_DEADLINE_MS: u64 = 1800000;
pub const HOST_APPROVAL_TIMEOUT_MS: u64 = 120000;
/// Field names, already lowercased, removed from every result that leaves a
/// relay for a model. The relay Worker and the native hosts read this one list.
pub const REMOVED_IDENTIFIER_FIELDS: &[&str] =
    &["devicename", "serial", "serialnumber", "deviceserial"];
pub const PAIRING_CODE_MINIMUM_LENGTH: usize = 6;
pub const PAIRING_CODE_MAXIMUM_LENGTH: usize = 128;
pub const MINIMUM_CREDENTIAL_LENGTH: usize = 24;
pub const MAX_REQUEST_FRAME_BYTES: usize = 65536;
pub const MAX_RESULT_FRAME_BYTES: usize = 16777216;
pub const COMPLETED_REQUEST_CACHE_SIZE: usize = 512;
pub const READINESS_INTERVAL_MS: u64 = 1000;
pub const READINESS_GRACE_MS: u64 = 1500;
pub const MAXIMUM_FAILURE_COUNT: u32 = 10;
pub const MAXIMUM_BACKOFF_EXPONENT: u32 = 6;
pub const MAXIMUM_BACKOFF_SECONDS: u64 = 60;
pub const BACKOFF_JITTER_MS: u64 = 1000;

/// Rejection is evidence about the exact held credential, never a display state.
pub fn pairing_owner_refusal(
    already_paired: bool,
    held: &str,
    incoming: &str,
    rejected: bool,
) -> Option<&'static str> {
    if rejected {
        return None;
    }
    let held = held.trim();
    let incoming = incoming.trim();
    if !held.is_empty() && !incoming.is_empty() && !held.eq_ignore_ascii_case(incoming) {
        return Some("PAIRING_OWNER_CHANGED");
    }
    if !held.is_empty() && incoming.is_empty() {
        return Some("PAIRING_OWNER_UNVERIFIED");
    }
    if already_paired && held.is_empty() {
        return Some("PAIRING_OWNER_UNKNOWN");
    }
    None
}
pub fn pairing_rejection_http(status: u16) -> bool {
    [401, 403].contains(&status)
}
pub fn pairing_rejection_close(code: u16) -> bool {
    [4001, 4003].contains(&code)
}
