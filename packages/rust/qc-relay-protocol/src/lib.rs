use serde::{Deserialize, Serialize};
use serde_json::Value;

mod dispatch_error_details;
pub use dispatch_error_details::project_dispatch_error_details;

mod generated_profile;
pub use generated_profile::*;

/// The `origin` of an `invoke` frame that serves an MCP resource read. The
/// cloud worker sets it (`services/qc-remote-cloud/src/index.ts`,
/// `RESOURCE_READ_ORIGIN`); see [`DeviceFrame::Invoke`].
pub const RESOURCE_READ_ORIGIN: &str = "resourceRead";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    /// Explicit owner action needed; absent on older peers and unrelated errors.
    #[serde(
        rename = "userActionRequired",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub user_action_required: Option<bool>,
    /// Sanitized structured facts supplied with the original device error.
    /// Older peers omit this field; absent details stay absent on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<Value>>,
}

impl DeviceError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            user_action_required: None,
            details: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceFrame {
    Ready {
        protocol: String,
        #[serde(rename = "usbConnected")]
        usb_connected: bool,
        #[serde(
            rename = "mcpPresence",
            default,
            skip_serializing_if = "std::ops::Not::not"
        )]
        mcp_presence: bool,
        /// Gateway API implemented by this host. Absence identifies a legacy
        /// peer for which the cloud retains the older deadline allowance.
        #[serde(
            rename = "gatewayApiVersion",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        gateway_api_version: Option<u64>,
    },
    McpPresence {
        clients: Vec<McpClientPresence>,
    },
    Invoke {
        id: String,
        action: String,
        method: String,
        #[serde(default)]
        params: Value,
        /// How long the relay will wait for this call, in milliseconds.
        ///
        /// The relay derives this per method from the host's own timing
        /// profile. Sending it removes the fourth, uncoordinated deadline a
        /// host would otherwise pick for itself: a host that gives up early
        /// reports a retryable failure for a persistent write that may already
        /// have committed, which is exactly the retry that leaves a second
        /// copy of a preset on the unit. Optional so a relay that predates
        /// this field still parses.
        #[serde(
            rename = "deadlineMs",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        deadline_ms: Option<u64>,
        /// Who asked, as the relay's MCP ingress classified it. Only
        /// [`RESOURCE_READ_ORIGIN`] means anything: an MCP resource read,
        /// which a host serves only from a device connection and session that
        /// are already open, never bringing one up. It can only narrow a
        /// call. Absent (a relay that predates it) or any other value, the
        /// call is handled as before; a host that predates it ignores it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
    },
    Result {
        id: String,
        ok: bool,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<DeviceError>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpClientPresence {
    pub id: String,
    pub name: String,
    /// The OAuth grant the session's access token belongs to: what a paired
    /// host revokes to unlink exactly this AI app. An identifier, not a
    /// credential (revoking also needs this host's device credential), but
    /// still kept out of logs. `None` from a relay that predates it; a host
    /// built before it ignores the field, as serde ignores unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default)]
    pub connected_at_unix_ms: u64,
    #[serde(default)]
    pub last_seen_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

/// Whether `value` has the shape of an OAuth grant id the relay's connector
/// revoke route accepts. A malformed one is treated as absent, never sent.
pub fn is_valid_grant_id(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_error_details_are_optional_and_keep_old_wire_shape() {
        let legacy = serde_json::json!({
            "code": "DEVICE_TRANSPORT",
            "message": "offline",
            "retryable": true
        });
        let parsed: DeviceError = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(parsed.details, None);
        assert_eq!(parsed.user_action_required, None);
        assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);

        let detailed = DeviceError {
            code: "-32013".into(),
            message: "DEVICE_SUSPENDED: backup in progress".into(),
            retryable: false,
            user_action_required: None,
            details: Some(Box::new(serde_json::json!({
                "code": "DEVICE_SUSPENDED",
                "requestSent": false
            }))),
        };
        assert_eq!(
            serde_json::to_value(detailed).unwrap()["details"]["requestSent"],
            false
        );
    }

    #[test]
    fn user_action_metadata_survives_roundtrip_and_legacy_stays_absent() {
        for (code, retryable, required) in [
            ("UNSAVED_STATE_UNCONFIRMED", false, true),
            ("UNSAVED_STATE_NOT_YET_OBSERVED", true, false),
        ] {
            let wire = serde_json::json!({"code":code,"message":"fixture","retryable":retryable,"userActionRequired":required});
            let decoded: DeviceError = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(decoded.user_action_required, Some(required));
            assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
        }
        let unrelated = DeviceError::new("DEVICE_TRANSPORT", "offline", true);
        assert!(serde_json::to_value(unrelated)
            .unwrap()
            .get("userActionRequired")
            .is_none());
    }

    #[test]
    fn wire_names_match_existing_android_and_server_protocol() {
        let ready = serde_json::to_value(DeviceFrame::Ready {
            protocol: PROTOCOL_VERSION.into(),
            usb_connected: true,
            mcp_presence: false,
            gateway_api_version: Some(31),
        })
        .unwrap();
        assert_eq!(
            ready,
            serde_json::json!({
                "type": "ready", "protocol": "qc-relay.v1", "usbConnected": true, "gatewayApiVersion": 31
            })
        );
        let legacy_ready: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "ready", "protocol": "qc-relay.v1", "usbConnected": true
        }))
        .unwrap();
        assert!(matches!(
            legacy_ready,
            DeviceFrame::Ready {
                gateway_api_version: None,
                ..
            }
        ));
        let invoke: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "invoke", "id": "1", "action": "get_current_preset",
            "method": "device.currentPreset", "params": {}
        }))
        .unwrap();
        assert!(
            matches!(invoke, DeviceFrame::Invoke { method, .. } if method == "device.currentPreset")
        );
    }

    #[test]
    fn presence_grant_id_is_optional_in_both_directions() {
        // A relay that predates the field: the row parses, with no grant.
        let old: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "mcp_presence",
            "clients": [{"id": "s1", "name": "Claude", "clientId": "c", "expiresAtUnixMs": 9}]
        }))
        .unwrap();
        let DeviceFrame::McpPresence { clients } = old else {
            panic!("not presence")
        };
        assert_eq!(clients[0].grant_id, None);
        // A current relay: the grant round-trips under its wire name.
        let new: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "mcp_presence",
            "clients": [{"id": "s1", "name": "Claude", "grantId": "Grant_0001", "expiresAtUnixMs": 9}]
        }))
        .unwrap();
        let encoded = serde_json::to_value(&new).unwrap();
        assert_eq!(encoded["clients"][0]["grantId"], "Grant_0001");
        let DeviceFrame::McpPresence { clients } = new else {
            panic!("not presence")
        };
        assert_eq!(clients[0].grant_id.as_deref(), Some("Grant_0001"));
        // Absent stays absent on the wire.
        let mut without = clients[0].clone();
        without.grant_id = None;
        assert!(serde_json::to_value(&without)
            .unwrap()
            .get("grantId")
            .is_none());
    }

    /// What a host built before `grant_id` existed does with a current frame:
    /// its row type had no such field, and serde's default ignores it.
    #[test]
    fn a_host_without_the_field_still_parses_a_frame_carrying_it() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        #[allow(dead_code)]
        struct OldRow {
            id: String,
            name: String,
            expires_at_unix_ms: u64,
        }
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        #[allow(dead_code)]
        enum OldFrame {
            McpPresence { clients: Vec<OldRow> },
        }
        let parsed: OldFrame = serde_json::from_value(serde_json::json!({
            "type": "mcp_presence",
            "clients": [{"id": "s1", "name": "Claude", "grantId": "Grant_0001", "expiresAtUnixMs": 9}]
        }))
        .unwrap();
        let OldFrame::McpPresence { clients } = parsed;
        assert_eq!(clients[0].id, "s1");
    }

    #[test]
    fn grant_ids_have_the_relays_shape() {
        assert!(is_valid_grant_id("Grant_0001"));
        assert!(is_valid_grant_id(&"a".repeat(128)));
        for bad in ["", "short", "has space x", "colon:grant", &"a".repeat(129)] {
            assert!(!is_valid_grant_id(bad), "{bad}");
        }
    }

    /// The invoke frame's `origin` is optional both ways. A
    /// relay that predates it sends none; a host that predates it ignores it,
    /// whatever its value; and absent stays absent on the wire.
    #[test]
    fn the_invoke_origin_is_optional_in_both_directions() {
        let legacy: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "invoke", "id": "1", "action": "get_current_preset",
            "method": "device.currentPreset", "params": {}
        }))
        .unwrap();
        assert!(matches!(legacy, DeviceFrame::Invoke { origin: None, .. }));
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert!(encoded.get("origin").is_none(), "{encoded}");

        let marked: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "invoke", "id": "2", "action": "get_current_preset",
            "method": "device.currentPreset", "params": {}, "origin": RESOURCE_READ_ORIGIN
        }))
        .unwrap();
        let encoded = serde_json::to_value(&marked).unwrap();
        assert_eq!(encoded["origin"], RESOURCE_READ_ORIGIN);
        let DeviceFrame::Invoke { origin, .. } = marked else {
            panic!("not an invoke")
        };
        assert_eq!(origin.as_deref(), Some(RESOURCE_READ_ORIGIN));

        // A value a later relay may send still parses; only the host decides
        // what it means, and an unknown one means nothing.
        let later: DeviceFrame = serde_json::from_value(serde_json::json!({
            "type": "invoke", "id": "3", "action": "get_current_preset",
            "method": "device.currentPreset", "params": {}, "origin": "someLaterOrigin"
        }))
        .unwrap();
        assert!(matches!(
            later,
            DeviceFrame::Invoke {
                origin: Some(_),
                ..
            }
        ));
    }
}
