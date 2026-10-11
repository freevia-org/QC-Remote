//! The reply the relay sends the unit: a plaintext type-61 message that echoes
//! the request's ids. Type 61 is outside the encrypted message families, so
//! nothing here involves any cipher.

use prost::Message as _;
use quadcortex::commands::OutboundMessage;
use quadcortex::profile;
use quadcortex::proto::cortex_protobuf_v2 as pa;

/// What `error_code` carries when the relay could not deliver a body.
///
/// UNKNOWN: the meaning the unit gives `error_code` has not been observed. The
/// working assumption is that zero or absent is success and that a non-zero
/// value is an HTTP status, so a status the server returned is passed through
/// as it is and each local failure is given a status-shaped number. These
/// values are chosen here and nowhere else.
pub mod error_code {
    pub const BAD_REQUEST: i32 = 400;
    pub const REFUSED: i32 = 403;
    pub const TIMEOUT: i32 = 408;
    pub const TOO_LARGE: i32 = 413;
    pub const BUSY: i32 = 429;
    pub const PROTOCOL: i32 = 502;
    pub const NETWORK: i32 = 503;
    pub const TOO_MANY_REDIRECTS: i32 = 508;
}

/// One reply frame. `last` is written on every frame, `false` until the final
/// one (UNKNOWN: whether the unit wants the field absent on interim chunks).
pub fn forward_response(
    request_id: Option<u64>,
    updater_request_id: u64,
    payload: Vec<u8>,
    last: bool,
    error_code: Option<i32>,
) -> OutboundMessage {
    let message = pa::UpdaterForwardMessage {
        action: pa::message_action::Enum::Update as i32,
        updater_request_id,
        request_id: request_id.map(pa::updater_forward_message::RequestId::RequestId),
        request: None,
        response: Some(pa::updater_forward_message::Response::Response(
            pa::UpdaterForwardResponse {
                payload,
                is_last_chunk: Some(pa::updater_forward_response::IsLastChunk::IsLastChunk(last)),
                error_code: error_code.map(pa::updater_forward_response::ErrorCode::ErrorCode),
            },
        )),
    };
    OutboundMessage {
        message_type: profile::MESSAGE_TYPE_UPDATER_FORWARD,
        payload: message.encode_to_vec(),
    }
}
