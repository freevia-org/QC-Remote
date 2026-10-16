//! Decoding a type-61 (`UpdaterForward`) frame into the request the relay
//! would run. Decoding performs no networking and trusts nothing in the frame.

use super::policy::DeviceHeader;
use prost::Message as _;
use quadcortex::proto::cortex_protobuf_v2 as pa;
use std::fmt;

/// What the unit is asking for, from `UpdaterForwardRequestType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Check,
    DownloadChangelog,
    DownloadUpdateHeader,
    DownloadUpdate,
}

impl RequestKind {
    pub fn from_wire(value: i32) -> Option<Self> {
        match pa::updater_forward_request_type::Enum::try_from(value).ok()? {
            pa::updater_forward_request_type::Enum::Check => Some(Self::Check),
            pa::updater_forward_request_type::Enum::DownloadChangelog => {
                Some(Self::DownloadChangelog)
            }
            pa::updater_forward_request_type::Enum::DownloadUpdateHeader => {
                Some(Self::DownloadUpdateHeader)
            }
            pa::updater_forward_request_type::Enum::DownloadUpdate => Some(Self::DownloadUpdate),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::DownloadChangelog => "download-changelog",
            Self::DownloadUpdateHeader => "download-update-header",
            Self::DownloadUpdate => "download-update",
        }
    }
}

/// A decoded request frame. `Debug` shows names and sizes, never the URL or a
/// header value.
#[derive(Clone, PartialEq, Eq)]
pub struct RelayRequest {
    pub updater_request_id: u64,
    pub request_id: Option<u64>,
    /// `None` when the unit sent a request type this build does not know.
    pub kind: Option<RequestKind>,
    pub url: String,
    pub timeout_ms: i32,
    pub headers: Vec<DeviceHeader>,
    /// Device-supplied bytes, including a body on GET.
    pub payload: Vec<u8>,
}

impl fmt::Debug for RelayRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayRequest")
            .field("updater_request_id", &self.updater_request_id)
            .field("request_id", &self.request_id)
            .field("kind", &self.kind)
            .field("url_bytes", &self.url.len())
            .field("timeout_ms", &self.timeout_ms)
            .field("headers", &self.headers)
            .field("payload_bytes", &self.payload.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Malformed,
    /// A well-formed type-61 message with no request in it (a response).
    NotARequest,
}

pub fn decode_request(payload: &[u8]) -> Result<RelayRequest, DecodeError> {
    let message = pa::UpdaterForwardMessage::decode(payload).map_err(|_| DecodeError::Malformed)?;
    let Some(pa::updater_forward_message::Request::Request(request)) = message.request else {
        return Err(DecodeError::NotARequest);
    };
    let request_payload = match request.payload {
        Some(pa::updater_forward_request::Payload::Payload(text)) => text.into_bytes(),
        None => Vec::new(),
    };
    Ok(RelayRequest {
        updater_request_id: message.updater_request_id,
        request_id: message
            .request_id
            .map(|pa::updater_forward_message::RequestId::RequestId(id)| id),
        kind: RequestKind::from_wire(request.r#type),
        url: request.url,
        timeout_ms: request.timeout,
        headers: request
            .headers
            .into_iter()
            .map(|pair| DeviceHeader {
                name: pair.name,
                value: pair.value,
            })
            .collect(),
        payload: request_payload,
    })
}
