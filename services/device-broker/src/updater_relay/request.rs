//! Decoding a type-61 (`UpdaterForward`) frame into the request the relay
//! would run. Decoding performs no networking and trusts nothing in the frame.

use super::policy::DeviceHeader;
use prost::Message as _;
use quadcortex::proto::cortex_protobuf_v2 as pa;
use std::fmt;

/// A request frame carries a URL, a few headers and a timeout; one the size of
/// a whole USB frame is not one.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// What the unit is asking for, from `UpdaterForwardRequestType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    Check,
    DownloadChangelog,
    DownloadUpdateHeader,
    DownloadUpdate,
}

/// The HTTP method of a relayed request. One variant on purpose: the protocol
/// has no method field, so adding a second is a reviewed change here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
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

    /// The method is derived from the request type and from nothing else the
    /// unit sends. UNKNOWN: no capture shows the method Cortex Control uses.
    /// Every type is a plain GET here because the one captured request is a
    /// CHECK with no body, the unit's own Content-Type header is passed through
    /// as supplied, and the HTTP stack Cortex Control embeds issues a GET
    /// whenever there is no body to post. DOWNLOAD_UPDATE_HEADER is assumed to
    /// be a ranged GET (the unit supplies any `Range` header itself). If a live
    /// test shows otherwise, this function is the only place to change.
    pub fn http_method(self) -> HttpMethod {
        HttpMethod::Get
    }

    /// Whether the unit is asking for the firmware image itself, which is the
    /// only kind allowed the large size cap and the long transfer time.
    pub fn is_image_download(self) -> bool {
        matches!(self, Self::DownloadUpdate)
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
    /// The unit supplied a non-empty body string. The relay sends none.
    pub has_payload: bool,
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
            .field("has_payload", &self.has_payload)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    TooLarge,
    Malformed,
    /// A well-formed type-61 message with no request in it (a response).
    NotARequest,
}

pub fn decode_request(payload: &[u8]) -> Result<RelayRequest, DecodeError> {
    if payload.len() > MAX_REQUEST_BYTES {
        return Err(DecodeError::TooLarge);
    }
    let message = pa::UpdaterForwardMessage::decode(payload).map_err(|_| DecodeError::Malformed)?;
    let Some(pa::updater_forward_message::Request::Request(request)) = message.request else {
        return Err(DecodeError::NotARequest);
    };
    let has_payload = matches!(
        &request.payload,
        Some(pa::updater_forward_request::Payload::Payload(text)) if !text.is_empty()
    );
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
        has_payload,
    })
}
