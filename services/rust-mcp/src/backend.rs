use async_trait::async_trait;
use serde_json::{Map, Value};
use std::sync::Arc;
use thiserror::Error;

/// Sanitized authorization result. The embedding host's loopback HTTP layer
/// inserts it into Axum request extensions after authenticating the caller, and
/// a stdio launcher supplies it through `QcMcp::for_route`. It contains routing
/// identifiers, never bearer tokens or provider secrets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalRoute {
    pub principal_id: Arc<str>,
    pub device_id: Arc<str>,
}

impl PrincipalRoute {
    pub fn new(principal_id: impl Into<Arc<str>>, device_id: impl Into<Arc<str>>) -> Self {
        Self {
            principal_id: principal_id.into(),
            device_id: device_id.into(),
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct BackendError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    /// Optional source-domain code kept separate from the transport class in
    /// `code` (for example, a device's `DEVICE_SUSPENDED` response).
    pub detail_code: Option<String>,
    /// Optional allowlisted facts from the source error, such as requestSent.
    pub details: Option<Box<Value>>,
}

impl BackendError {
    /// Reporting policy follows only the stable source-domain code, never text or nested flags.
    pub(crate) fn reporting_policy(&self) -> (bool, Option<bool>) {
        match self.detail_code.as_deref() {
            Some("UNSAVED_STATE_UNCONFIRMED") => (false, Some(true)),
            Some("UNSAVED_STATE_NOT_YET_OBSERVED") => (true, Some(false)),
            _ => (self.retryable, None),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: "device_unavailable",
            message: message.into(),
            retryable: true,
            detail_code: None,
            details: None,
        }
    }
}

/// Implemented by the embedding host (the Windows app). One backend instance MUST be
/// scoped to exactly one authenticated principal/device route.
#[async_trait]
pub trait QcBackend: Send + Sync + 'static {
    async fn request(
        &self,
        route: &PrincipalRoute,
        rpc: &'static str,
        params: Map<String, Value>,
    ) -> Result<Value, BackendError>;
}
