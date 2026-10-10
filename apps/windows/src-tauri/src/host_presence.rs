//! Live MCP sessions. Installation history never enters this registry.
use crate::tray_presence::{Presence, TrayClient, TrayClientKind, TrayClientMetadata};
use qc_relay_client::McpClientPresence;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

const BRIDGE_LEASE_MS: u64 = 12_000;
const MAX_CLIENTS: usize = 128;
/// The header a native stdio bridge (`qc-remote.exe --local-mcp-bridge`) puts
/// on its tool calls to the loopback `/rpc`: the same per-process
/// `stdio-<random>` id its heartbeat announces. It only says which session the
/// request belongs to; the bearer token alone authorizes it. A host older than
/// the header ignores it, and a request without it is handled as before.
pub(crate) const BRIDGE_SESSION_HEADER: &str = "x-qc-remote-bridge-session";
/// At most this many unlinked ids are held. Past it the one unlinked longest
/// ago is dropped, which only lets its row come back on its next heartbeat.
const MAX_UNLINKED: usize = MAX_CLIENTS;
#[derive(Clone)]
struct Entry {
    name: String,
    /// The client's own `clientInfo.version`, when it sent one.
    version: Option<String>,
    /// When this session id was first recorded. A heartbeat renews the lease,
    /// never this.
    connected_at: u64,
    expires_at: Option<u64>,
}
/// A session the tray unlinked while its bridge may still be running. Its
/// heartbeats are answered as usual but not recorded, so its row stays off the
/// list until that chat makes a new tool call. In memory only: a broker
/// restart forgets every one.
struct Unlinked {
    name: String,
    version: Option<String>,
    unlinked_at: u64,
    /// Renewed by each ignored heartbeat exactly as the row's lease would
    /// be; once it lapses the bridge has gone and the id is forgotten.
    expires_at: u64,
}
#[derive(Default)]
struct Registry {
    local: BTreeMap<String, Entry>,
    /// Bounded by [`MAX_UNLINKED`] and by the bridge lease.
    unlinked: BTreeMap<String, Unlinked>,
}
impl Registry {
    /// A native bridge's heartbeat. False when it was not recorded because
    /// the tray unlinked this session; the caller answers it the same way.
    fn heartbeat(&mut self, id: &str, name: &str, version: Option<&str>, now: u64) -> bool {
        self.forget_departed(now);
        if let Some(entry) = self.unlinked.get_mut(id) {
            entry.expires_at = now.saturating_add(BRIDGE_LEASE_MS);
            return false;
        }
        self.record(
            id,
            name,
            version,
            Some(now.saturating_add(BRIDGE_LEASE_MS)),
            now,
        );
        true
    }
    /// A tool call that named its bridge session. If the tray had unlinked
    /// that session, the chat's new request links it again: the row returns
    /// now, as a new session under a fresh lease its heartbeats then renew.
    /// Any other id changes nothing. Never delays or refuses the request.
    fn request(&mut self, id: &str, now: u64) -> bool {
        self.forget_departed(now);
        let Some(entry) = self.unlinked.remove(id) else {
            return false;
        };
        self.record(
            id,
            &entry.name,
            entry.version.as_deref(),
            Some(now.saturating_add(BRIDGE_LEASE_MS)),
            now,
        );
        true
    }
    fn forget_departed(&mut self, now: u64) {
        self.unlinked.retain(|_, entry| entry.expires_at > now);
    }
    fn record(
        &mut self,
        id: &str,
        name: &str,
        version: Option<&str>,
        expires_at: Option<u64>,
        now: u64,
    ) {
        self.local
            .retain(|_, row| row.expires_at.is_none_or(|deadline| deadline > now));
        if !valid_session_id(id)
            || !valid_name(name)
            || (!self.local.contains_key(id) && self.local.len() >= MAX_CLIENTS)
        {
            return;
        }
        let connected_at = self.local.get(id).map_or(now, |row| row.connected_at);
        self.local.insert(
            id.into(),
            Entry {
                name: name.into(),
                version: version
                    .filter(|version| valid_name(version))
                    .map(str::to_owned),
                connected_at,
                expires_at,
            },
        );
    }
    /// The tray's Unlink: the row goes now, and the session's heartbeats stop
    /// bringing it back until that chat makes a new tool call. Nothing is
    /// refused or blocked, and no other session is touched. A loopback HTTP
    /// session sends no heartbeat, so for it this only removes the row; its
    /// next request records it again, as before.
    fn end(&mut self, id: &str, now: u64) -> bool {
        let Some(row) = self.local.remove(id) else {
            return false;
        };
        self.forget_departed(now);
        if !self.unlinked.contains_key(id) && self.unlinked.len() >= MAX_UNLINKED {
            if let Some(oldest) = self
                .unlinked
                .iter()
                .min_by_key(|(_, entry)| entry.unlinked_at)
                .map(|(id, _)| id.clone())
            {
                self.unlinked.remove(&oldest);
            }
        }
        self.unlinked.insert(
            id.into(),
            Unlinked {
                name: row.name,
                version: row.version,
                unlinked_at: now,
                expires_at: now.saturating_add(BRIDGE_LEASE_MS),
            },
        );
        true
    }
    /// The session ended for good (the bridge closed, or the HTTP session was
    /// dropped): its id never returns, so its row and any unlink go.
    fn close(&mut self, id: &str) {
        self.local.remove(id);
        self.unlinked.remove(id);
    }
    fn clients(&mut self, now: u64) -> Vec<TrayClient> {
        self.local
            .retain(|_, row| row.expires_at.is_none_or(|deadline| deadline > now));
        self.local
            .iter()
            .map(|(id, row)| TrayClient {
                name: row.name.clone(),
                kind: TrayClientKind::LocalMcp,
                process_id: None,
                // A bridge's heartbeat is a lease, not activity, so no
                // "last seen" is claimed for a local session.
                metadata: Some(TrayClientMetadata {
                    session_id: Some(id.clone()),
                    version: row.version.clone(),
                    connected_at_unix_ms: Some(row.connected_at),
                    ..TrayClientMetadata::default()
                }),
            })
            .collect()
    }
}
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}
/// Session ids are published to the broker icon, which addresses Unlink by
/// them, so they stay printable and short.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_graphic())
}
fn registry() -> &'static Mutex<Registry> {
    static VALUE: OnceLock<Mutex<Registry>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
type RemoteReceiver = watch::Receiver<Option<Vec<McpClientPresence>>>;
fn remote_endpoint() -> &'static Mutex<Option<String>> {
    static VALUE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
fn connector_endpoint(base: Option<&str>) -> Option<String> {
    base.and_then(|base| qc_relay_client::normalize_endpoint(base).ok())
        .map(|origin| format!("{origin}/mcp"))
}
/// Called only when the host refreshes its pairing settings, never per RPC.
pub(crate) fn set_remote_endpoint(base: Option<&str>) {
    *remote_endpoint().lock().unwrap_or_else(|p| p.into_inner()) = connector_endpoint(base);
}
fn remote_state_receiver(
) -> &'static Mutex<Option<watch::Receiver<qc_relay_client::ConnectionState>>> {
    static VALUE: OnceLock<Mutex<Option<watch::Receiver<qc_relay_client::ConnectionState>>>> =
        OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
pub(crate) fn set_remote_state(receiver: watch::Receiver<qc_relay_client::ConnectionState>) {
    *remote_state_receiver()
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(receiver);
}
pub(crate) fn remote_state() -> qc_relay_client::ConnectionState {
    remote_state_receiver()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|r| *r.borrow())
        .unwrap_or_default()
}
fn remote() -> &'static Mutex<Option<RemoteReceiver>> {
    static VALUE: OnceLock<Mutex<Option<RemoteReceiver>>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}
pub(crate) fn set_remote(receiver: RemoteReceiver) {
    *remote().lock().unwrap_or_else(|p| p.into_inner()) = Some(receiver);
}
/// A native bridge's heartbeat. It is answered the same way whether or not it
/// is recorded; one from a session the tray unlinked is not.
pub(crate) fn bridge_heartbeat(id: &str, name: &str, version: Option<&str>, now: u64) {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .heartbeat(id, name, version, now);
}
/// A tool call that carried [`BRIDGE_SESSION_HEADER`]: links that session
/// again if the tray had unlinked it. A map lookup under the registry lock,
/// which is a leaf: nothing else is locked or awaited while it is held.
pub(crate) fn bridge_request(id: &str, now: u64) {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .request(id, now);
}
/// The broker icon's Unlink on a local client, in this process: ends the
/// session this host published under `id`, and only that one, and stops its
/// heartbeats from listing it again until that chat's next tool call. False
/// when there is none (it ended meanwhile).
pub(crate) fn end_local_session(id: &str) -> bool {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .end(id, now_ms())
}
/// Whether the background host keeps its own broker connection (and so the
/// device) for its clients: any published client, local or remote. The host
/// decides this every second from its snapshot. When the broker icon's Unlink
/// ends the last local session, this turns false and the host lets its broker
/// connection go (`release_when_unused`), exactly as when a bridge closes or
/// its lease lapses. The unlinked bridge's heartbeats no longer turn it true
/// again: it stays false until that chat's next tool call links the session
/// again (the call itself reopens the connection it needs). With "Release QC
/// after last client" set, the broker applies that policy meanwhile, as it
/// does whenever the last client leaves.
pub(crate) fn host_in_use(clients: &[TrayClient]) -> bool {
    !clients.is_empty()
}
pub(crate) fn bridge_closed(id: &str) {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .close(id);
}
pub(crate) fn now_ms() -> u64 {
    quadcortex::time::unix_time_ms()
}
fn remote_activity(receiver: Option<&RemoteReceiver>) -> Option<Vec<McpClientPresence>> {
    receiver.and_then(|receiver| receiver.borrow().clone())
}
pub(crate) fn snapshot(endpoint: Option<String>) -> Presence {
    let now = now_ms();
    let mut clients = registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clients(now);
    let activity = remote_activity(remote().lock().unwrap_or_else(|p| p.into_inner()).as_ref());
    let remote_activity_available = activity.is_some();
    if let Some(rows) = activity {
        clients.extend(
            rows.iter()
                .filter(|row| row.expires_at_unix_ms > now && valid_name(&row.name))
                .take(MAX_CLIENTS)
                .map(|row| TrayClient {
                    name: row.name.clone(),
                    kind: TrayClientKind::RemoteMcp,
                    process_id: None,
                    metadata: Some(TrayClientMetadata {
                        session_id: Some(row.id.clone()),
                        grant_id: remote_grant(row),
                        linked_app_id: None,
                        version: row.version.clone(),
                        device: row.device.clone(),
                        browser: row.browser.clone(),
                        ip_address: row.ip_address.clone(),
                        country: row.country.clone(),
                        city: row.city.clone(),
                        connected_at_unix_ms: (row.connected_at_unix_ms > 0)
                            .then_some(row.connected_at_unix_ms),
                        last_seen_at_unix_ms: (row.last_seen_at_unix_ms > 0)
                            .then_some(row.last_seen_at_unix_ms),
                    }),
                }),
        );
    }
    Presence {
        endpoint,
        remote_endpoint: remote_endpoint()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
        remote_activity_available,
        clients,
        linked_apps: None,
    }
}

// Remote Unlink (broker icon). Kept together and apart from the local session
// code above. The grant is read from this process's own live relay presence,
// never from the published status file.
fn remote_grant(row: &McpClientPresence) -> Option<String> {
    row.grant_id
        .clone()
        .filter(|grant| qc_relay_client::is_valid_grant_id(grant))
}
/// What this process's live relay presence holds for the remote session `id`:
/// `None` when no such session is live (or the relay is not connected here),
/// `Some(None)` when it is live but the relay did not name its grant.
pub(crate) fn live_remote_grant(id: &str) -> Option<Option<String>> {
    let rows = remote_activity(remote().lock().unwrap_or_else(|p| p.into_inner()).as_ref())?;
    live_grant_in(&rows, id, now_ms())
}
fn live_grant_in(rows: &[McpClientPresence], id: &str, now: u64) -> Option<Option<String>> {
    rows.iter()
        .find(|row| row.id == id && row.expires_at_unix_ms > now)
        .map(remote_grant)
}

struct HttpSession {
    id: String,
    /// Display name and version, from the MCP initialize exchange.
    name: Mutex<Option<(String, String)>>,
}
impl qc_remote_mcp::ClientPresence for HttpSession {
    fn initialized(&self, client: &rmcp::model::Implementation) {
        *self.name.lock().unwrap_or_else(|p| p.into_inner()) = Some((
            client
                .title
                .as_deref()
                .filter(|title| valid_name(title))
                .unwrap_or(&client.name)
                .to_owned(),
            client.version.clone(),
        ));
        self.active();
    }
    fn active(&self) {
        if let Some((name, version)) = self.name.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
        {
            let now = now_ms();
            registry().lock().unwrap_or_else(|p| p.into_inner()).record(
                &self.id,
                name,
                Some(version),
                Some(now.saturating_add(15_000)),
                now,
            );
        }
    }
}
impl Drop for HttpSession {
    fn drop(&mut self) {
        bridge_closed(&self.id);
    }
}
pub(crate) fn http_session() -> Arc<dyn qc_remote_mcp::ClientPresence> {
    Arc::new(HttpSession {
        id: format!("http-{:032x}", rand::random::<u128>()),
        name: Mutex::new(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn remote_row(id: &str, grant: Option<&str>) -> McpClientPresence {
        McpClientPresence {
            id: id.into(),
            name: "Claude".into(),
            grant_id: grant.map(str::to_owned),
            version: None,
            device: None,
            browser: None,
            ip_address: None,
            country: None,
            city: None,
            connected_at_unix_ms: 1,
            last_seen_at_unix_ms: 1,
            expires_at_unix_ms: 1_000,
        }
    }
    #[test]
    fn remote_unlink_resolves_each_session_to_its_own_live_grant() {
        let rows = [
            remote_row("relay-a", Some("GrantAAAA1")),
            remote_row("relay-b", Some("GrantBBBB2")),
            remote_row("relay-old", None),
            remote_row("relay-bad", Some("bad grant")),
        ];
        assert_eq!(
            live_grant_in(&rows, "relay-a", 500),
            Some(Some("GrantAAAA1".into()))
        );
        assert_eq!(
            live_grant_in(&rows, "relay-b", 500),
            Some(Some("GrantBBBB2".into()))
        );
        assert_eq!(
            live_grant_in(&rows, "relay-old", 500),
            Some(None),
            "older relay"
        );
        assert_eq!(
            live_grant_in(&rows, "relay-bad", 500),
            Some(None),
            "malformed is absent"
        );
        assert_eq!(live_grant_in(&rows, "relay-gone", 500), None);
        assert_eq!(
            live_grant_in(&rows, "relay-a", 1_000),
            None,
            "an expired lease is not live"
        );
        assert_eq!(remote_grant(&rows[1]).as_deref(), Some("GrantBBBB2"));
    }
    #[test]
    fn unavailable_remote_activity_is_distinct_from_a_measured_empty_snapshot() {
        let (sender, receiver) = watch::channel(None);
        assert_eq!(remote_activity(None), None);
        assert_eq!(remote_activity(Some(&receiver)), None);
        sender.send_replace(Some(vec![]));
        assert_eq!(remote_activity(Some(&receiver)), Some(vec![]));
        sender.send_replace(None);
        assert_eq!(
            remote_activity(Some(&receiver)),
            None,
            "disconnect withdraws measured activity"
        );
    }
    #[test]
    fn connector_url_is_a_plain_https_mcp_address_and_unpair_clears_it() {
        assert_eq!(
            connector_endpoint(Some("https://relay.example.test/")),
            Some("https://relay.example.test/mcp".into())
        );
        assert_eq!(connector_endpoint(None), None);
        let rejected = [
            "http://relay.example.test",
            "https://relay.example.test:secret",
            "https://relay.example.test?token=secret",
            "https://relay.example.test/#secret",
        ];
        assert_eq!(rejected.len(), 4);
        for value in rejected {
            assert_eq!(connector_endpoint(Some(value)), None, "{value}");
        }
    }
    #[test]
    fn counts_sessions_not_provider_names_and_expires_only_leased_bridges() {
        let mut rows = Registry::default();
        rows.record("bridge-one", "Claude", None, Some(12_000), 0);
        rows.record("bridge-two", "Claude", None, Some(12_000), 0);
        rows.record("http", "Codex", Some("0.44.0"), None, 0);
        assert_eq!(rows.clients(1).len(), 3);
        assert_eq!(
            rows.clients(12_000),
            vec![TrayClient {
                name: "Codex".into(),
                kind: TrayClientKind::LocalMcp,
                process_id: None,
                metadata: Some(TrayClientMetadata {
                    session_id: Some("http".into()),
                    version: Some("0.44.0".into()),
                    connected_at_unix_ms: Some(0),
                    ..TrayClientMetadata::default()
                }),
            }]
        );
        rows.local.remove("http");
        assert!(rows.clients(12_001).is_empty());
    }
    #[test]
    fn ending_a_session_removes_only_that_lease_and_a_new_announcement_starts_afresh() {
        let mut rows = Registry::default();
        rows.record("stdio-a", "Codex", Some("1"), Some(12_000), 100);
        rows.record("stdio-b", "Codex", None, Some(12_000), 200);
        rows.record("http-c", "Claude", None, None, 300);
        // A heartbeat renews the lease and keeps the start.
        rows.record("stdio-b", "Codex", None, Some(17_000), 5_000);
        assert!(rows.end("stdio-b", 5_500));
        assert!(!rows.end("stdio-b", 5_500), "already ended");
        assert!(
            !rows.end("relay-1", 5_500),
            "remote presence is never in this registry"
        );
        let left: Vec<_> = rows
            .clients(6_000)
            .into_iter()
            .map(|row| row.metadata.unwrap().session_id.unwrap())
            .collect();
        assert_eq!(left, ["http-c", "stdio-a"]);
        // The same chat's next tool call links it again, as a new session.
        assert!(rows.request("stdio-b", 6_000));
        let again = rows
            .clients(6_000)
            .into_iter()
            .find(|row| row.metadata.as_ref().unwrap().session_id.as_deref() == Some("stdio-b"))
            .unwrap();
        assert_eq!(again.metadata.unwrap().connected_at_unix_ms, Some(6_000));
    }
    #[test]
    fn unlinking_the_last_session_lets_the_host_go_until_that_chat_asks_again() {
        // The trigger is unchanged: with no published client the host is not
        // in use and releases its broker connection. What changed is what
        // makes it in use again: the chat's next tool call, not a heartbeat.
        let mut rows = Registry::default();
        assert!(rows.heartbeat("stdio-only", "Codex", None, 0));
        assert!(host_in_use(&rows.clients(1_000)));
        assert!(rows.end("stdio-only", 1_000));
        assert!(!host_in_use(&rows.clients(1_000)));
        assert!(!rows.heartbeat("stdio-only", "Codex", None, 5_000));
        assert!(!host_in_use(&rows.clients(5_000)));
        assert!(rows.request("stdio-only", 7_000));
        assert!(host_in_use(&rows.clients(7_000)));
        // Ending one of two sessions keeps the host in use.
        assert!(rows.heartbeat("stdio-other", "Claude", None, 7_000));
        assert!(rows.end("stdio-only", 8_000));
        assert!(host_in_use(&rows.clients(8_000)));
    }
    fn listed(rows: &mut Registry, now: u64) -> Vec<String> {
        rows.clients(now)
            .into_iter()
            .map(|row| row.metadata.unwrap().session_id.unwrap())
            .collect()
    }
    #[test]
    fn an_unlinked_bridge_heartbeat_is_not_recorded_until_its_chat_makes_a_request() {
        let mut rows = Registry::default();
        assert!(rows.heartbeat("stdio-a", "Codex", Some("2"), 0));
        assert!(rows.heartbeat("stdio-b", "Claude", None, 0));
        assert!(rows.end("stdio-a", 1_000));
        // Heartbeats keep coming every 5 s; none lists it again, while the
        // other session is renewed as usual.
        for tick in 1..=20 {
            let now = 1_000 + tick * 5_000;
            assert!(!rows.heartbeat("stdio-a", "Codex", Some("2"), now));
            assert!(rows.heartbeat("stdio-b", "Claude", None, now));
            assert_eq!(listed(&mut rows, now), ["stdio-b"]);
        }
        // A request from an unrelated id, or one that never was unlinked,
        // changes nothing.
        assert!(!rows.request("stdio-zzz", 101_500));
        assert!(!rows.request("stdio-b", 101_500));
        assert_eq!(listed(&mut rows, 101_500), ["stdio-b"]);
        // The chat's next tool call relinks it with the name and version it
        // announced, and its heartbeats renew the row from then on.
        assert!(rows.request("stdio-a", 102_000));
        assert_eq!(listed(&mut rows, 102_000), ["stdio-a", "stdio-b"]);
        let row = rows
            .clients(102_000)
            .into_iter()
            .find(|row| row.metadata.as_ref().unwrap().session_id.as_deref() == Some("stdio-a"))
            .unwrap();
        assert_eq!(row.name, "Codex");
        assert_eq!(row.metadata.unwrap().version.as_deref(), Some("2"));
        assert!(rows.heartbeat("stdio-a", "Codex", Some("2"), 106_000));
        assert!(!rows.request("stdio-a", 107_000), "already linked");
        assert_eq!(listed(&mut rows, 110_000), ["stdio-a", "stdio-b"]);
    }
    /// A bridge from before this change heartbeats but never names its
    /// session on a tool call, so the host never sees `request` for it: after
    /// Unlink it stays hidden, and the host is not in use (so does not hold the
    /// device) for as long as that bridge process keeps announcing.
    #[test]
    fn an_old_bridge_stays_hidden_and_does_not_hold_the_host_after_unlink() {
        let mut rows = Registry::default();
        assert!(rows.heartbeat("stdio-old", "Codex", None, 0));
        assert!(host_in_use(&rows.clients(0)));
        assert!(rows.end("stdio-old", 1_000));
        // An hour of heartbeats every 5 s, its tool calls carrying no id.
        for tick in 1..=720u64 {
            let now = 1_000 + tick * 5_000;
            assert!(!rows.heartbeat("stdio-old", "Codex", None, now));
            assert!(rows.clients(now).is_empty());
            assert!(!host_in_use(&rows.clients(now)));
        }
        assert!(rows.unlinked.contains_key("stdio-old"));
    }
    #[test]
    fn a_heartbeat_gap_of_one_lease_forgets_the_unlink() {
        // Sleep and resume, or a stalled host: a gap of a whole lease reads
        // as the bridge having gone, and the next heartbeat lists it again.
        let mut rows = Registry::default();
        assert!(rows.heartbeat("stdio-sleepy", "Codex", None, 0));
        assert!(rows.end("stdio-sleepy", 1_000));
        assert!(!rows.heartbeat("stdio-sleepy", "Codex", None, 6_000));
        assert!(!rows.heartbeat("stdio-sleepy", "Codex", None, 6_000 + BRIDGE_LEASE_MS - 1));
        let resumed = 6_000 + 2 * BRIDGE_LEASE_MS;
        assert!(rows.heartbeat("stdio-sleepy", "Codex", None, resumed));
        assert_eq!(listed(&mut rows, resumed), ["stdio-sleepy"]);
    }
    #[test]
    fn an_unlinked_id_is_forgotten_once_its_bridge_stops_announcing() {
        let mut rows = Registry::default();
        assert!(rows.heartbeat("stdio-gone", "Codex", None, 0));
        assert!(rows.end("stdio-gone", 1_000));
        assert!(!rows.heartbeat("stdio-gone", "Codex", None, 5_000));
        // No heartbeat for a whole lease: the bridge has exited.
        rows.forget_departed(5_000 + BRIDGE_LEASE_MS);
        assert!(rows.unlinked.is_empty());
        assert!(!rows.request("stdio-gone", 5_000 + BRIDGE_LEASE_MS));
        // A bridge closing forgets its unlink at once.
        assert!(rows.heartbeat("stdio-closing", "Codex", None, 20_000));
        assert!(rows.end("stdio-closing", 20_000));
        rows.close("stdio-closing");
        assert!(rows.unlinked.is_empty());
    }
    #[test]
    fn the_unlinked_set_is_bounded() {
        let mut rows = Registry::default();
        let ids: Vec<_> = (0..MAX_UNLINKED + 5)
            .map(|index| format!("stdio-{index:04}"))
            .collect();
        for (index, id) in ids.iter().enumerate() {
            let now = index as u64;
            assert!(rows.heartbeat(id, "Codex", None, now));
            assert!(rows.end(id, now));
        }
        assert_eq!(rows.unlinked.len(), MAX_UNLINKED);
        // The earliest unlinks made room; those bridges may list again.
        assert!(!rows.unlinked.contains_key(&ids[0]));
        assert!(rows.unlinked.contains_key(ids.last().unwrap()));
        let now = ids.len() as u64;
        assert!(rows.heartbeat(&ids[0], "Codex", None, now));
    }
    #[test]
    fn an_http_session_unlink_only_removes_the_row_until_its_next_request() {
        // A loopback HTTP session (the Node bridge, a direct HTTP client)
        // sends no heartbeat: its own activity records it, unchanged.
        let mut rows = Registry::default();
        rows.record("http-x", "Claude", Some("1"), Some(15_000), 0);
        assert!(rows.end("http-x", 1_000));
        assert!(listed(&mut rows, 1_000).is_empty());
        rows.record("http-x", "Claude", Some("1"), Some(17_000), 2_000);
        assert_eq!(listed(&mut rows, 2_000), ["http-x"]);
    }
    #[test]
    fn unprintable_session_ids_are_not_recorded() {
        let mut rows = Registry::default();
        for id in ["", "has space", "line\nbreak", &"x".repeat(129)] {
            rows.record(id, "Codex", None, None, 0);
        }
        assert!(rows.clients(0).is_empty());
    }
    #[test]
    fn malformed_names_are_not_presented_as_clients() {
        let mut rows = Registry::default();
        for name in ["", "\nClaude", &"x".repeat(129)] {
            rows.record("bad", name, None, None, 0);
        }
        assert!(rows.clients(0).is_empty());
    }
    #[test]
    fn http_client_display_title_is_preserved_when_advertised() {
        let title = format!("Codex (test-{:032x})", rand::random::<u128>());
        let session = http_session();
        let mut client = rmcp::model::Implementation::new("codex-mcp-client", "1");
        client.title = Some(title.clone());
        session.initialized(&client);
        assert_eq!(
            snapshot(None)
                .clients
                .iter()
                .filter(|row| row.name == title)
                .count(),
            1
        );
        drop(session);
        assert!(!snapshot(None).clients.iter().any(|row| row.name == title));
    }
    #[test]
    fn http_initialization_uses_the_actual_name_and_drop_removes_the_session() {
        let name = format!("http-test-{:032x}", rand::random::<u128>());
        let session = http_session();
        assert!(snapshot(None).clients.iter().all(|row| row.name != name));
        session.initialized(&rmcp::model::Implementation::new(&name, "1"));
        assert_eq!(
            snapshot(None)
                .clients
                .iter()
                .filter(|row| row.name == name)
                .count(),
            1
        );
        session.active();
        assert_eq!(
            snapshot(None)
                .clients
                .iter()
                .filter(|row| row.name == name)
                .count(),
            1
        );
        drop(session);
        assert!(snapshot(None).clients.iter().all(|row| row.name != name));
    }
}
