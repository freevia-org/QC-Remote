//! The background MCP host's `tray-host-status.json`, trusted only when the
//! process that wrote it proves it is still that process. Display only: it
//! never feeds the idle policy, so a stale or forged record can at worst
//! mislabel a menu row.
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

pub const FILE_NAME: &str = "tray-host-status.json";
/// The host republishes every second; three missed beats is not a live host.
const FRESH_FOR_MS: u64 = 3_000;
const MAX_BYTES: u64 = 128 * 1024;
const MAX_CLIENTS: usize = 32;

/// One MCP client session the host published. Every field but `name` and
/// `remote` comes from the record's `metadata` and is `None` when the host did
/// not publish it: the menu shows only what is here and never fills a gap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostClient {
    pub name: String,
    pub remote: bool,
    /// The host's own id for this session: a local bridge's lease id, or the
    /// relay's presence id for a remote client. Addresses the session for
    /// Unlink; never a credential.
    pub session_id: Option<String>,
    /// Whether the host published an OAuth grant for this remote session, so
    /// the icon may offer Unlink. Only the fact is kept: the icon never holds
    /// or sends the grant, and the host resolves it itself from the session.
    pub grant_known: bool,
    /// The linked app (from [`HostPresence::linked_apps`]) this remote session
    /// runs under, when the host could place it.
    pub linked_app_id: Option<String>,
    pub version: Option<String>,
    /// For a remote client these four describe the provider route that
    /// reached the relay (the Remote panel's "Provider route"), which may be
    /// provider infrastructure rather than the user's own device.
    pub device: Option<String>,
    pub browser: Option<String>,
    pub ip_address: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub connected_at_unix_ms: Option<u64>,
    /// Only remote clients publish it: a local bridge's heartbeat is a lease,
    /// not activity, so the host does not claim a "last active" for it.
    pub last_seen_at_unix_ms: Option<u64>,
}

/// One linked remote app (one OAuth grant of the account), as the host last
/// fetched the account's connector list. Display fields only: the entry id is
/// opaque, and Unlink sends it to this process's host, which resolves it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkedApp {
    pub entry_id: String,
    pub name: String,
    pub access: Option<String>,
    pub authorized_from: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub authorized_at_unix_ms: Option<u64>,
    pub last_active_at_unix_ms: Option<u64>,
    pub app_version: Option<String>,
}

/// The host's linked-apps list. `loaded: false` is a failed fetch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkedApps {
    pub loaded: bool,
    pub apps: Vec<LinkedApp>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostPresence {
    pub endpoint: Option<String>,
    pub remote_endpoint: Option<String>,
    pub clients: Vec<HostClient>,
    /// `None` when the host published no list (not fetched yet, or an older
    /// host): the menu then shows live sessions only, as before.
    pub linked_apps: Option<LinkedApps>,
    /// The record was written by this very process, so its local sessions
    /// live in this process's host and the icon can end one in process.
    pub published_by_this_process: bool,
}

pub(crate) const MAX_FIELD_CHARS: usize = 80;
const MAX_SESSION_ID_BYTES: usize = 128;

/// Unicode bidirectional formatting characters. They are not `is_control`,
/// yet an embedded override (U+202E) reverses how the rest of a menu label
/// reads, so a self-declared name could spoof another.
fn is_bidi_format(character: char) -> bool {
    matches!(
        character,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Every untrusted string the menu shows passes through here: bidirectional
/// formatting removed, trimmed, refused when empty or when it still carries a
/// control character, and cut to a menu-sized length.
pub(crate) fn display_text(text: &str) -> Option<String> {
    let cleaned: String = text
        .chars()
        .filter(|character| !is_bidi_format(*character))
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty() && !cleaned.chars().any(char::is_control))
        .then(|| cleaned.chars().take(MAX_FIELD_CHARS).collect())
}

/// A display string from the record, through [`display_text`].
fn field(value: &Value) -> Option<String> {
    display_text(value.as_str()?)
}

fn session_id(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    (!id.is_empty()
        && id.len() <= MAX_SESSION_ID_BYTES
        && id.chars().all(|character| character.is_ascii_graphic()))
    .then(|| id.to_owned())
}

/// The relay's grant shape (`[A-Za-z0-9_-]{8,128}`); anything else is absent.
fn grant_id(value: &Value) -> bool {
    value.as_str().is_some_and(|id| {
        (8..=128).contains(&id.len())
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    })
}

fn instant(value: &Value) -> Option<u64> {
    value.as_u64().filter(|ms| *ms > 0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    process_id: u32,
    process_created_at: u64,
    published_at_unix_ms: u64,
    status: Value,
}

fn presence(status: &Value) -> HostPresence {
    let text = |key: &str| {
        status[key]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let clients = status["clients"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    // A row whose name cannot be shown safely is one the host
                    // itself would never publish (it refuses such names).
                    let name = field(&row["name"])?;
                    let remote = match row["kind"].as_str()? {
                        "localMcp" => false,
                        "remoteMcp" => true,
                        // The app's own windows are participants already.
                        _ => return None,
                    };
                    let metadata = &row["metadata"];
                    Some(HostClient {
                        name,
                        remote,
                        session_id: session_id(&metadata["sessionId"]),
                        grant_known: remote && grant_id(&metadata["grantId"]),
                        linked_app_id: remote
                            .then(|| session_id(&metadata["linkedAppId"]))
                            .flatten(),
                        version: field(&metadata["version"]),
                        device: field(&metadata["device"]),
                        browser: field(&metadata["browser"]),
                        ip_address: field(&metadata["ipAddress"]),
                        country: field(&metadata["country"]),
                        city: field(&metadata["city"]),
                        connected_at_unix_ms: instant(&metadata["connectedAtUnixMs"]),
                        last_seen_at_unix_ms: instant(&metadata["lastSeenAtUnixMs"]),
                    })
                })
                .take(MAX_CLIENTS)
                .collect()
        })
        .unwrap_or_default();
    HostPresence {
        endpoint: text("endpoint"),
        remote_endpoint: text("remoteEndpoint"),
        clients,
        linked_apps: linked_apps(&status["linkedApps"]),
        published_by_this_process: false,
    }
}

fn linked_apps(value: &Value) -> Option<LinkedApps> {
    let loaded = value["loaded"].as_bool()?;
    let apps = value["apps"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(LinkedApp {
                        entry_id: session_id(&row["entryId"])?,
                        name: field(&row["name"])?,
                        access: field(&row["access"]),
                        authorized_from: field(&row["authorizedFrom"]),
                        city: field(&row["city"]),
                        country: field(&row["country"]),
                        authorized_at_unix_ms: instant(&row["authorizedAtUnixMs"]),
                        last_active_at_unix_ms: instant(&row["lastActiveAtUnixMs"]),
                        app_version: field(&row["appVersion"]),
                    })
                })
                .take(MAX_CLIENTS)
                .collect()
        })
        .unwrap_or_default();
    Some(LinkedApps {
        loaded: loaded && value["apps"].is_array(),
        apps,
    })
}

/// Every check the record must pass, with the process lookups injected.
fn trusted(
    bytes: &[u8],
    now_ms: u64,
    own_process_id: u32,
    alive: impl FnOnce(u32) -> bool,
    created_at: impl FnOnce(u32) -> Option<u64>,
) -> Option<HostPresence> {
    let record: Record = serde_json::from_slice(bytes).ok()?;
    let fresh = record.published_at_unix_ms <= now_ms
        && now_ms - record.published_at_unix_ms < FRESH_FOR_MS;
    (record.process_id != 0
        && record.process_created_at != 0
        && fresh
        && alive(record.process_id)
        && created_at(record.process_id) == Some(record.process_created_at))
    .then(|| HostPresence {
        published_by_this_process: record.process_id == own_process_id,
        ..presence(&record.status)
    })
}

fn unix_time_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}

/// `None` unless the file is owner-only, small, fresh, and names a live
/// process whose creation time matches (so a recycled pid cannot vouch).
pub fn read(directory: &Path) -> Option<HostPresence> {
    let path = directory.join(FILE_NAME);
    if !qc_owner_only_file::is_owner_only(&path).ok()? {
        return None;
    }
    if std::fs::metadata(&path).ok()?.len() > MAX_BYTES {
        return None;
    }
    trusted(
        &std::fs::read(&path).ok()?,
        unix_time_ms(),
        std::process::id(),
        crate::platform::process::os_process_is_alive,
        crate::platform::process::os_process_created_at,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(published: u64) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "processId": 7,
            "processCreatedAt": 99,
            "publishedAtUnixMs": published,
            "status": {
                "endpoint": "http://127.0.0.1:5000/mcp",
                "remoteEndpoint": null,
                "clients": [
                    {"name": "QC Remote", "kind": "qcRemoteApp", "processId": 1},
                    {"name": "codex-mcp-client", "kind": "localMcp", "processId": 2,
                        "metadata": {"sessionId": "stdio-0001", "version": "0.44.0",
                            "connectedAtUnixMs": 500}},
                    {"name": "Claude", "kind": "remoteMcp", "processId": null,
                        "metadata": {"sessionId": "relay-7", "grantId": "GrantAAAA1", "version": "1.2",
                            "device": "Windows", "browser": "Chrome",
                            "ipAddress": "192.0.2.4", "country": "BG", "city": "Sofia",
                            "connectedAtUnixMs": 100, "lastSeenAtUnixMs": 900}},
                    {"name": "  ", "kind": "localMcp"}
                ]
            }
        }))
        .unwrap()
    }

    const OTHER_PROCESS: u32 = 8;

    #[test]
    fn a_live_matching_writer_is_read_without_its_own_windows() {
        let presence = trusted(
            &record(1_000),
            2_000,
            OTHER_PROCESS,
            |pid| pid == 7,
            |_| Some(99),
        )
        .unwrap();
        assert_eq!(
            presence.endpoint.as_deref(),
            Some("http://127.0.0.1:5000/mcp")
        );
        assert_eq!(presence.remote_endpoint, None);
        assert!(!presence.published_by_this_process);
        assert_eq!(
            presence.clients,
            [
                HostClient {
                    name: "codex-mcp-client".into(),
                    remote: false,
                    session_id: Some("stdio-0001".into()),
                    version: Some("0.44.0".into()),
                    connected_at_unix_ms: Some(500),
                    ..HostClient::default()
                },
                HostClient {
                    name: "Claude".into(),
                    remote: true,
                    session_id: Some("relay-7".into()),
                    grant_known: true,
                    linked_app_id: None,
                    version: Some("1.2".into()),
                    device: Some("Windows".into()),
                    browser: Some("Chrome".into()),
                    ip_address: Some("192.0.2.4".into()),
                    country: Some("BG".into()),
                    city: Some("Sofia".into()),
                    connected_at_unix_ms: Some(100),
                    last_seen_at_unix_ms: Some(900),
                }
            ]
        );
    }

    #[test]
    fn only_a_record_this_process_wrote_offers_its_local_sessions_in_process() {
        let own = trusted(&record(1_000), 2_000, 7, |_| true, |_| Some(99)).unwrap();
        assert!(own.published_by_this_process);
    }

    #[test]
    fn malformed_metadata_is_dropped_field_by_field_never_shown() {
        let bytes = serde_json::to_vec(&json!({
            "processId": 7, "processCreatedAt": 99, "publishedAtUnixMs": 1_000,
            "status": {"clients": [{"name": "Codex", "kind": "localMcp", "metadata": {
                "sessionId": "has space", "version": "1\n2", "city": " ",
                "connectedAtUnixMs": 0, "lastSeenAtUnixMs": "soon",
                "device": "x".repeat(200)
            }}]}
        }))
        .unwrap();
        let presence = trusted(&bytes, 2_000, OTHER_PROCESS, |_| true, |_| Some(99)).unwrap();
        assert_eq!(
            presence.clients,
            [HostClient {
                name: "Codex".into(),
                device: Some("x".repeat(MAX_FIELD_CHARS)),
                ..HostClient::default()
            }]
        );
    }

    #[test]
    fn linked_apps_are_read_with_their_sessions_and_a_failed_list_is_kept_apart() {
        let bytes = serde_json::to_vec(&json!({
            "processId": 7, "processCreatedAt": 99, "publishedAtUnixMs": 1_000,
            "status": {
                "clients": [
                    {"name": "Claude", "kind": "remoteMcp", "metadata": {"sessionId": "r-1", "linkedAppId": "app-1"}},
                    {"name": "Codex", "kind": "localMcp", "metadata": {"sessionId": "l-1", "linkedAppId": "app-1"}}
                ],
                "linkedApps": {"loaded": true, "apps": [
                    {"entryId": "app-1", "name": "Claude", "access": "Control access",
                        "authorizedFrom": "Windows Chrome 140", "city": "Sofia", "country": "BG",
                        "authorizedAtUnixMs": 10, "lastActiveAtUnixMs": 20, "appVersion": "1.2"},
                    {"entryId": "has space", "name": "Bad id"},
                    {"entryId": "app-3", "name": "\n"}
                ]}
            }
        }))
        .unwrap();
        let presence = trusted(&bytes, 2_000, OTHER_PROCESS, |_| true, |_| Some(99)).unwrap();
        assert_eq!(
            presence.linked_apps,
            Some(LinkedApps {
                loaded: true,
                apps: vec![LinkedApp {
                    entry_id: "app-1".into(),
                    name: "Claude".into(),
                    access: Some("Control access".into()),
                    authorized_from: Some("Windows Chrome 140".into()),
                    city: Some("Sofia".into()),
                    country: Some("BG".into()),
                    authorized_at_unix_ms: Some(10),
                    last_active_at_unix_ms: Some(20),
                    app_version: Some("1.2".into()),
                }],
            })
        );
        let placed: Vec<_> = presence
            .clients
            .iter()
            .map(|client| client.linked_app_id.as_deref())
            .collect();
        assert_eq!(
            placed,
            [Some("app-1"), None],
            "only remote sessions are placed"
        );
        let failed = serde_json::to_vec(&json!({
            "processId": 7, "processCreatedAt": 99, "publishedAtUnixMs": 1_000,
            "status": {"clients": [], "linkedApps": {"loaded": false, "apps": []}}
        }))
        .unwrap();
        let presence = trusted(&failed, 2_000, OTHER_PROCESS, |_| true, |_| Some(99)).unwrap();
        assert_eq!(
            presence.linked_apps,
            Some(LinkedApps {
                loaded: false,
                apps: vec![]
            })
        );
        // An older host publishes no list at all.
        let older = trusted(&record(1_000), 2_000, OTHER_PROCESS, |_| true, |_| Some(99)).unwrap();
        assert_eq!(older.linked_apps, None);
    }

    #[test]
    fn only_a_well_formed_remote_grant_offers_remote_unlink() {
        let bytes = serde_json::to_vec(&json!({
            "processId": 7, "processCreatedAt": 99, "publishedAtUnixMs": 1_000,
            "status": {"clients": [
                {"name": "A", "kind": "remoteMcp", "metadata": {"sessionId": "r-1", "grantId": "GrantAAAA1"}},
                {"name": "B", "kind": "remoteMcp", "metadata": {"sessionId": "r-2"}},
                {"name": "C", "kind": "remoteMcp", "metadata": {"sessionId": "r-3", "grantId": "no:grant"}},
                {"name": "D", "kind": "localMcp", "metadata": {"sessionId": "l-1", "grantId": "GrantAAAA1"}}
            ]}
        }))
        .unwrap();
        let presence = trusted(&bytes, 2_000, OTHER_PROCESS, |_| true, |_| Some(99)).unwrap();
        let known: Vec<_> = presence
            .clients
            .iter()
            .map(|client| (client.name.as_str(), client.grant_known))
            .collect();
        assert_eq!(
            known,
            [("A", true), ("B", false), ("C", false), ("D", false)]
        );
    }

    #[test]
    fn untrusted_text_loses_bidi_overrides_and_is_refused_with_controls() {
        assert_eq!(
            display_text("Cla\u{202E}edu\u{2066}x\u{2069}").as_deref(),
            Some("Claedux")
        );
        assert_eq!(
            display_text("\u{200F} Codex \u{200E}").as_deref(),
            Some("Codex")
        );
        assert_eq!(display_text("Co\u{7}dex"), None);
        assert_eq!(display_text("Co\ndex"), None);
        assert_eq!(display_text("\u{202E}\u{202C}"), None);
        assert_eq!(
            display_text(&"y".repeat(500)).map(|text| text.chars().count()),
            Some(MAX_FIELD_CHARS)
        );
        let bytes = serde_json::to_vec(&json!({
            "processId": 7, "processCreatedAt": 99, "publishedAtUnixMs": 1_000,
            "status": {"clients": [
                {"name": "Evil\u{202E}gnp.exe", "kind": "localMcp"},
                {"name": "bad\u{1b}[31m", "kind": "localMcp"}
            ]}
        }))
        .unwrap();
        let presence = trusted(&bytes, 2_000, OTHER_PROCESS, |_| true, |_| Some(99)).unwrap();
        assert_eq!(
            presence.clients,
            [HostClient {
                name: "Evilgnp.exe".into(),
                ..HostClient::default()
            }]
        );
    }

    #[test]
    fn stale_future_dead_and_recycled_writers_are_not_trusted() {
        let read = |bytes: &[u8], now: u64, alive: bool, created: Option<u64>| {
            trusted(bytes, now, OTHER_PROCESS, |_| alive, |_| created)
        };
        assert!(read(&record(1_000), 3_999, true, Some(99)).is_some());
        assert!(
            read(&record(1_000), 4_000, true, Some(99)).is_none(),
            "stale"
        );
        assert!(
            read(&record(1_000), 999, true, Some(99)).is_none(),
            "future"
        );
        assert!(
            read(&record(1_000), 2_000, false, Some(99)).is_none(),
            "dead"
        );
        assert!(
            read(&record(1_000), 2_000, true, Some(100)).is_none(),
            "recycled pid"
        );
        assert!(
            read(&record(1_000), 2_000, true, None).is_none(),
            "unknown creation"
        );
        assert!(read(b"{}", 2_000, true, Some(99)).is_none());
        let zero = serde_json::to_vec(&json!({"processId": 0, "processCreatedAt": 99,
            "publishedAtUnixMs": 1000, "status": {}}))
        .unwrap();
        assert!(read(&zero, 2_000, true, Some(99)).is_none());
    }

    #[test]
    fn a_missing_file_reads_as_no_host() {
        let directory =
            std::env::temp_dir().join(format!("qc-broker-host-status-{}", std::process::id()));
        assert_eq!(read(&directory), None);
    }
}
