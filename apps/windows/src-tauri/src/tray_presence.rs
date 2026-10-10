//! Sanitized MCP presence the host publishes. The device broker's icon reads the
//! published copy (tray-host-status.json), never this process's memory.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrayClientKind {
    QcRemoteApp,
    LocalMcp,
    RemoteMcp,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayClient {
    pub name: String,
    pub kind: TrayClientKind,
    pub process_id: Option<u32>,
    #[serde(default)]
    pub metadata: Option<TrayClientMetadata>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayClientMetadata {
    pub session_id: Option<String>,
    /// A remote session's OAuth grant, as the relay reported it. Published so
    /// the broker icon knows whether Unlink can be offered; the icon never
    /// acts on it: Unlink names the session, and this process revokes the
    /// grant its own live presence holds for it. Absent from an older relay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    /// For a remote session: the opaque id of the linked app (OAuth grant) it
    /// runs under, from [`Presence::linked_apps`], so the icon can nest it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_app_id: Option<String>,
    pub version: Option<String>,
    pub device: Option<String>,
    pub browser: Option<String>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub connected_at_unix_ms: Option<u64>,
    pub last_seen_at_unix_ms: Option<u64>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    /// The loopback listener. The GUI's LOCAL panel reads this field.
    pub endpoint: Option<String>,
    /// The paired public connector address, independently of local MCP enablement.
    #[serde(default)]
    pub remote_endpoint: Option<String>,
    /// False for an older cloud/host or a disconnected relay; true includes a
    /// measured empty snapshot. Absence in an older document means unknown.
    #[serde(default)]
    pub remote_activity_available: bool,
    pub clients: Vec<TrayClient>,
    /// The account's linked AI apps (one per OAuth grant), as this process last
    /// fetched them from the relay. `None` before a fetch (or from an older
    /// host); a failed fetch is `loaded: false`. Display only: Unlink names an
    /// entry id, and this process revokes the grant its own list holds for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_apps: Option<TrayLinkedApps>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayLinkedApps {
    /// False when the last fetch failed: the icon shows sessions only.
    pub loaded: bool,
    pub apps: Vec<TrayLinkedApp>,
}

/// One linked app: what the relay's connector list offers, never a grant id.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayLinkedApp {
    /// Opaque, per process, stable across refreshes while the grant is listed.
    pub entry_id: String,
    pub name: String,
    /// "Control access" or "Read-only access", from the grant's scopes.
    pub access: String,
    /// The device and browser (or the label) recorded when it was authorized.
    pub authorized_from: Option<String>,
    /// Coarse location recorded when it was authorized.
    pub city: Option<String>,
    pub country: Option<String>,
    pub authorized_at_unix_ms: u64,
    /// The relay's last activity, raised by any later live session seen here.
    pub last_active_at_unix_ms: Option<u64>,
    pub app_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn remote_connector_is_additive_and_never_replaces_the_local_listener() {
        let legacy = json!({"endpoint":"http://127.0.0.1:54321/mcp","clients":[]});
        let mut presence: Presence = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(
            presence.remote_endpoint, None,
            "older hosts remain readable"
        );
        assert!(!presence.remote_activity_available);
        presence.remote_endpoint = Some("https://relay.example.test/mcp".into());
        let document = serde_json::to_value(presence).unwrap();
        assert_eq!(
            document["endpoint"], legacy["endpoint"],
            "LOCAL status keeps its own port"
        );
        assert_eq!(document["remoteEndpoint"], "https://relay.example.test/mcp");
    }
    #[test]
    fn linked_apps_are_optional_and_carry_no_grant_id() {
        let older: Presence =
            serde_json::from_value(json!({"endpoint": null, "clients": []})).unwrap();
        assert_eq!(older.linked_apps, None);
        let presence = Presence {
            linked_apps: Some(TrayLinkedApps {
                loaded: true,
                apps: vec![TrayLinkedApp {
                    entry_id: "app-1".into(),
                    name: "Claude".into(),
                    access: "Control access".into(),
                    authorized_at_unix_ms: 5,
                    ..TrayLinkedApp::default()
                }],
            }),
            ..Presence::default()
        };
        let document = serde_json::to_value(&presence).unwrap();
        assert_eq!(document["linkedApps"]["apps"][0]["entryId"], "app-1");
        assert!(!document.to_string().contains("grantId"));
    }

    #[test]
    fn a_remote_grant_is_published_only_when_known_and_older_documents_still_read() {
        let mut metadata = TrayClientMetadata {
            session_id: Some("relay-1".into()),
            grant_id: Some("GrantAAAA1".into()),
            ..TrayClientMetadata::default()
        };
        let published = serde_json::to_value(&metadata).unwrap();
        assert_eq!(published["grantId"], "GrantAAAA1");
        metadata.grant_id = None;
        assert!(serde_json::to_value(&metadata)
            .unwrap()
            .get("grantId")
            .is_none());
        let older: TrayClientMetadata =
            serde_json::from_value(json!({"sessionId": "relay-1"})).unwrap();
        assert_eq!(older.grant_id, None);
    }
}
