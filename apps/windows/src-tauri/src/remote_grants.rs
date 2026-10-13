//! The account's linked AI apps (one per OAuth grant), held by the process
//! that runs the relay, for the broker icon's remote list and its Unlink.
//!
//! Fetched with the call the Remote panel's list uses
//! (`qc_relay_client::connectors`, under this computer's device credential)
//! when this process's relay connects (retried a bounded number of times while
//! it stays connected) and around every remote Unlink, never on a timer. The
//! icon is given an opaque entry id per grant and the display fields the
//! connector list really carries, each capped in length; the grant id stays
//! here, and an Unlink naming an entry is resolved against this list only.
//!
//! Every fetch takes a generation number when it starts, and a result is kept
//! only if no newer one was kept already, so a slow old answer never replaces
//! a newer list. The list records which pairing (endpoint and device id, no
//! secret) it was fetched with: a list from an earlier pairing resolves no
//! Unlink, and a new pairing starts with fresh entry ids.
use crate::tray_presence::{Presence, TrayClientKind, TrayLinkedApp, TrayLinkedApps};
use qc_relay_client::{Credentials, RemoteConnector};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// One list read. An Unlink reads twice around its revoke, so this bounds it.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// The broker icon shows at most this many host rows; publishing more only
/// grows the status record.
pub(crate) const MAX_APPS: usize = 32;
/// Every published text field is cut to this many characters, the icon's own
/// display bound, so no one grant's details can push the status record past
/// its size limit.
pub(crate) const MAX_FIELD_CHARS: usize = 80;
/// Pauses before the second and third connect-time attempts.
const CONNECT_RETRY_PAUSES: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];

#[derive(Clone, Debug, PartialEq, Eq)]
struct LinkedApp {
    entry_id: String,
    grant_id: String,
    connector: TrayLinkedApp,
}

#[derive(Default)]
struct Roster {
    /// `None` before the first fetch; `Some(Err)` after a failed one.
    apps: Option<Result<Vec<LinkedApp>, ()>>,
    /// grant -> entry id, kept while the grant stays listed.
    ids: BTreeMap<String, String>,
    /// grant -> latest live-session activity seen here since the fetch.
    seen: BTreeMap<String, u64>,
    /// The pairing the list was fetched with.
    identity: Option<String>,
    /// The newest generation kept (or cleared at); older results are dropped.
    generation: u64,
}

impl Roster {
    /// Keep a fetch result unless a newer one was kept. Returns whether kept.
    fn store(
        &mut self,
        generation: u64,
        identity: &str,
        fetched: Result<Vec<RemoteConnector>, String>,
        new_id: &mut dyn FnMut() -> String,
    ) -> bool {
        if generation <= self.generation {
            return false;
        }
        self.generation = generation;
        if self.identity.as_deref() != Some(identity) {
            // Another pairing: nothing of the old list may resolve again.
            self.ids.clear();
            self.seen.clear();
            self.identity = Some(identity.to_owned());
        }
        let Ok(connectors) = fetched else {
            self.apps = Some(Err(()));
            return true;
        };
        let mut ids = BTreeMap::new();
        let apps: Vec<LinkedApp> = connectors
            .into_iter()
            .filter(|connector| qc_relay_client::is_valid_grant_id(&connector.grant_id))
            .take(MAX_APPS)
            .map(|connector| {
                let entry_id = self
                    .ids
                    .get(&connector.grant_id)
                    .cloned()
                    .unwrap_or_else(&mut *new_id);
                ids.insert(connector.grant_id.clone(), entry_id.clone());
                LinkedApp {
                    entry_id: entry_id.clone(),
                    grant_id: connector.grant_id.clone(),
                    connector: display(&connector, entry_id),
                }
            })
            .collect();
        self.seen.retain(|grant, _| ids.contains_key(grant));
        self.ids = ids;
        self.apps = Some(Ok(apps));
        true
    }

    /// Forget the list; results of fetches started before this are dropped.
    fn clear(&mut self, floor: u64) {
        *self = Roster {
            generation: self.generation.max(floor),
            ..Roster::default()
        };
    }

    /// The grant and the pairing it was listed under, for `entry_id`.
    fn grant_of_entry(&self, entry_id: &str) -> Option<(String, String)> {
        match (&self.apps, &self.identity) {
            (Some(Ok(apps)), Some(identity)) => apps
                .iter()
                .find(|app| app.entry_id == entry_id)
                .map(|app| (app.grant_id.clone(), identity.clone())),
            _ => None,
        }
    }

    /// Publish the list and nest each live remote session under its app.
    fn annotate(&mut self, presence: &mut Presence) {
        let Some(fetched) = &self.apps else {
            return;
        };
        let Ok(apps) = fetched else {
            presence.linked_apps = Some(TrayLinkedApps {
                loaded: false,
                apps: Vec::new(),
            });
            return;
        };
        for client in &mut presence.clients {
            if client.kind != TrayClientKind::RemoteMcp {
                continue;
            }
            let Some(metadata) = client.metadata.as_mut() else {
                continue;
            };
            let Some(grant) = metadata.grant_id.as_deref() else {
                continue;
            };
            if let Some(app) = apps.iter().find(|app| app.grant_id == grant) {
                metadata.linked_app_id = Some(app.entry_id.clone());
                if let Some(seen) = metadata.last_seen_at_unix_ms {
                    let latest = self.seen.entry(app.grant_id.clone()).or_insert(seen);
                    *latest = (*latest).max(seen);
                }
            }
        }
        presence.linked_apps = Some(TrayLinkedApps {
            loaded: true,
            apps: apps
                .iter()
                .map(|app| {
                    let mut shown = app.connector.clone();
                    if let Some(seen) = self.seen.get(&app.grant_id) {
                        shown.last_active_at_unix_ms =
                            Some(shown.last_active_at_unix_ms.unwrap_or(0).max(*seen));
                    }
                    shown
                })
                .collect(),
        });
    }
}

fn cap(text: &str) -> String {
    text.chars().take(MAX_FIELD_CHARS).collect()
}

fn cap_option(text: Option<&str>) -> Option<String> {
    text.map(cap)
}

/// The fields the Remote panel's connector row shows, minus the IP address
/// and the OAuth client id, which the icon does not show. Each is capped.
fn display(connector: &RemoteConnector, entry_id: String) -> TrayLinkedApp {
    let device = [connector.device.as_deref(), connector.browser.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    TrayLinkedApp {
        entry_id,
        name: cap(&connector.name),
        access: if connector.scopes.iter().any(|scope| scope == "qc:control") {
            "Control access".into()
        } else {
            "Read-only access".into()
        },
        authorized_from: if device.is_empty() {
            cap_option(connector.authorized_from.as_deref())
        } else {
            Some(cap(&device))
        },
        city: cap_option(connector.city.as_deref()),
        country: cap_option(connector.country.as_deref()),
        authorized_at_unix_ms: connector.authorized_at,
        last_active_at_unix_ms: connector.last_active_at,
        app_version: cap_option(connector.app_version.as_deref()),
    }
}

fn roster() -> &'static Mutex<Roster> {
    static VALUE: OnceLock<Mutex<Roster>> = OnceLock::new();
    VALUE.get_or_init(Mutex::default)
}

fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn new_entry_id() -> String {
    format!("app-{:032x}", rand::random::<u128>())
}

/// Which pairing a credential is: endpoint and device id, never the secret.
fn identity_of(credentials: &Credentials) -> String {
    format!("{}|{}", credentials.endpoint, credentials.device_id)
}

/// The pairing this computer holds now, if any.
pub(crate) async fn current_identity() -> Option<String> {
    crate::relay::load_credentials()
        .await
        .ok()
        .flatten()
        .map(|credentials| identity_of(&credentials))
}

/// Read the list once, bounded, keep it (or the failure) unless a newer read
/// was kept meanwhile, and return the grant ids it listed.
pub(crate) async fn fetch() -> Result<Vec<String>, String> {
    let generation = next_generation();
    let credentials = crate::relay::load_credentials()
        .await?
        .ok_or_else(|| "Pair this computer with QC Remote Cloud first".to_string())?;
    let fetched = tokio::time::timeout(FETCH_TIMEOUT, qc_relay_client::connectors(&credentials))
        .await
        .unwrap_or_else(|_| Err("QC Remote Cloud did not answer in time".into()));
    let grants = fetched
        .as_ref()
        .map(|list| {
            list.iter()
                .map(|connector| connector.grant_id.clone())
                .collect()
        })
        .map_err(Clone::clone);
    roster().lock().unwrap_or_else(|p| p.into_inner()).store(
        generation,
        &identity_of(&credentials),
        fetched,
        &mut new_entry_id,
    );
    grants
}

/// The connect-time read: up to three attempts, the later two only while
/// `connected` still holds. After that the list stays "could not be loaded"
/// until the next connect or Unlink.
pub(crate) async fn refresh_on_connect(connected: impl Fn() -> bool) {
    refresh_with_retries(connected, fetch, &CONNECT_RETRY_PAUSES).await;
}

async fn refresh_with_retries<C, F, Fut>(connected: C, mut fetch: F, pauses: &[Duration]) -> usize
where
    C: Fn() -> bool,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>, String>>,
{
    let mut attempts = 0;
    for pause in std::iter::once(None).chain(pauses.iter().copied().map(Some)) {
        if let Some(pause) = pause {
            tokio::time::sleep(pause).await;
            if !connected() {
                break;
            }
        }
        attempts += 1;
        if fetch().await.is_ok() {
            break;
        }
    }
    attempts
}

/// Forget the list (the relay or its pairing changed, or none remains).
pub(crate) fn clear() {
    let floor = next_generation();
    roster()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear(floor);
}

/// The grant this process's own list holds for `entry_id`, with the pairing
/// that list was fetched under.
pub(crate) fn grant_of_entry(entry_id: &str) -> Option<(String, String)> {
    roster()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .grant_of_entry(entry_id)
}

pub(crate) fn annotate(presence: &mut Presence) {
    roster()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .annotate(presence);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray_presence::{TrayClient, TrayClientMetadata};
    use std::cell::Cell;

    const PAIRING: &str = "https://relay.example.test|device-1";

    fn connector(grant: &str, name: &str, authorized_at: u64) -> RemoteConnector {
        serde_json::from_value(serde_json::json!({
            "grantId": grant, "clientId": "oauth-client-claude", "name": name,
            "scopes": ["qc:read", "qc:control"], "authorizedAt": authorized_at,
            "device": "Windows", "browser": "Chrome 140", "city": "Sofia", "country": "BG",
            "ipAddress": "192.0.2.4", "lastActiveAt": 50
        }))
        .unwrap()
    }

    fn session(id: &str, grant: Option<&str>, seen: u64) -> TrayClient {
        TrayClient {
            name: "Claude".into(),
            kind: TrayClientKind::RemoteMcp,
            process_id: None,
            metadata: Some(TrayClientMetadata {
                session_id: Some(id.into()),
                grant_id: grant.map(str::to_owned),
                last_seen_at_unix_ms: Some(seen),
                ..TrayClientMetadata::default()
            }),
        }
    }

    fn ids() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("app-{next}")
        }
    }

    fn grant(roster: &Roster, entry: &str) -> Option<String> {
        roster.grant_of_entry(entry).map(|(grant, _)| grant)
    }

    #[test]
    fn each_grant_is_one_entry_and_sessions_nest_under_their_own() {
        let mut roster = Roster::default();
        let mut id = ids();
        roster.store(
            1,
            PAIRING,
            Ok(vec![
                connector("GrantAAAA1", "Claude", 10),
                connector("GrantBBBB2", "Claude", 20),
                connector("bad grant", "Broken", 30),
            ]),
            &mut id,
        );
        let mut presence = Presence {
            clients: vec![
                session("relay-a", Some("GrantAAAA1"), 90),
                session("relay-x", Some("GrantZZZZ9"), 95),
                session("relay-old", None, 99),
            ],
            ..Presence::default()
        };
        roster.annotate(&mut presence);
        let linked = presence.linked_apps.clone().unwrap();
        assert!(linked.loaded);
        let entries: Vec<_> = linked
            .apps
            .iter()
            .map(|app| (app.entry_id.as_str(), app.last_active_at_unix_ms))
            .collect();
        assert_eq!(
            entries,
            [("app-1", Some(90)), ("app-2", Some(50))],
            "malformed grants are not listed; live activity raises the relay's"
        );
        let nested: Vec<_> = presence
            .clients
            .iter()
            .map(|client| client.metadata.as_ref().unwrap().linked_app_id.as_deref())
            .collect();
        assert_eq!(nested, [Some("app-1"), None, None]);
        assert_eq!(
            linked.apps[0].authorized_from.as_deref(),
            Some("Windows Chrome 140")
        );
        assert_eq!(linked.apps[0].access, "Control access");
        let published = serde_json::to_string(&presence.linked_apps).unwrap();
        assert!(!published.contains("Grant"), "no grant id in the list");
        assert!(
            !published.contains("192.0.2.4"),
            "no IP address in the list"
        );
        assert_eq!(
            roster.grant_of_entry("app-2"),
            Some(("GrantBBBB2".into(), PAIRING.into()))
        );
        assert_eq!(roster.grant_of_entry("app-9"), None);
    }

    #[test]
    fn entry_ids_survive_a_refresh_and_a_dropped_grant_takes_its_id_away() {
        let mut roster = Roster::default();
        let mut id = ids();
        roster.store(
            1,
            PAIRING,
            Ok(vec![
                connector("GrantAAAA1", "A", 1),
                connector("GrantBBBB2", "B", 2),
            ]),
            &mut id,
        );
        roster.store(
            2,
            PAIRING,
            Ok(vec![connector("GrantBBBB2", "B", 2)]),
            &mut id,
        );
        assert_eq!(grant(&roster, "app-1"), None, "unlinked grant is gone");
        assert_eq!(grant(&roster, "app-2").as_deref(), Some("GrantBBBB2"));
    }

    #[test]
    fn an_older_answer_never_replaces_a_newer_list() {
        let mut roster = Roster::default();
        let mut id = ids();
        assert!(roster.store(
            5,
            PAIRING,
            Ok(vec![connector("GrantBBBB2", "B", 2)]),
            &mut id
        ));
        assert!(!roster.store(
            4,
            PAIRING,
            Ok(vec![connector("GrantAAAA1", "A", 1)]),
            &mut id
        ));
        assert!(!roster.store(4, PAIRING, Err("late failure".into()), &mut id));
        assert_eq!(grant(&roster, "app-1").as_deref(), Some("GrantBBBB2"));
        // A fetch that started before a clear is dropped too.
        roster.clear(9);
        assert!(!roster.store(
            8,
            PAIRING,
            Ok(vec![connector("GrantAAAA1", "A", 1)]),
            &mut id
        ));
        assert_eq!(roster.grant_of_entry("app-1"), None);
    }

    #[test]
    fn a_new_pairing_gets_new_entry_ids_and_its_list_names_its_pairing() {
        let mut roster = Roster::default();
        let mut id = ids();
        roster.store(
            1,
            PAIRING,
            Ok(vec![connector("GrantAAAA1", "A", 1)]),
            &mut id,
        );
        assert_eq!(
            roster
                .grant_of_entry("app-1")
                .map(|(_, pairing)| pairing)
                .as_deref(),
            Some(PAIRING)
        );
        roster.store(
            2,
            "https://relay.example.test|device-2",
            Ok(vec![connector("GrantAAAA1", "A", 1)]),
            &mut id,
        );
        assert_eq!(
            roster.grant_of_entry("app-1"),
            None,
            "old ids resolve nothing"
        );
        assert_eq!(
            roster
                .grant_of_entry("app-2")
                .map(|(_, pairing)| pairing)
                .as_deref(),
            Some("https://relay.example.test|device-2")
        );
    }

    #[test]
    fn a_failed_fetch_publishes_no_entries_and_resolves_nothing() {
        let mut roster = Roster::default();
        let mut id = ids();
        roster.store(
            1,
            PAIRING,
            Ok(vec![connector("GrantAAAA1", "A", 1)]),
            &mut id,
        );
        roster.store(
            2,
            PAIRING,
            Err("Could not reach QC Remote Cloud".into()),
            &mut id,
        );
        let mut presence = Presence {
            clients: vec![session("relay-a", Some("GrantAAAA1"), 9)],
            ..Presence::default()
        };
        roster.annotate(&mut presence);
        assert_eq!(
            presence.linked_apps,
            Some(TrayLinkedApps {
                loaded: false,
                apps: Vec::new()
            })
        );
        assert_eq!(
            presence.clients[0].metadata.as_ref().unwrap().linked_app_id,
            None
        );
        assert_eq!(roster.grant_of_entry("app-1"), None);
        // Before any fetch nothing is published at all.
        let mut fresh = Presence::default();
        Roster::default().annotate(&mut fresh);
        assert_eq!(fresh.linked_apps, None);
    }

    #[test]
    fn an_oversized_list_keeps_the_status_record_small_and_the_clients_listed() {
        let huge = "x".repeat(20_000);
        let connectors: Vec<RemoteConnector> = (0..200)
            .map(|index| {
                serde_json::from_value(serde_json::json!({
                    "grantId": format!("Grant{index:05}x"), "clientId": huge, "name": huge,
                    "scopes": ["qc:read"], "authorizedAt": 1, "authorizedFrom": huge,
                    "device": huge, "browser": huge, "city": huge, "country": huge,
                    "appVersion": huge, "ipAddress": huge
                }))
                .unwrap()
            })
            .collect();
        let mut roster = Roster::default();
        roster.store(1, PAIRING, Ok(connectors), &mut ids());
        let mut presence = Presence {
            clients: vec![session("relay-a", Some("Grant00001x"), 9)],
            ..Presence::default()
        };
        roster.annotate(&mut presence);
        let apps = &presence.linked_apps.as_ref().unwrap().apps;
        assert_eq!(apps.len(), MAX_APPS, "the same bound the icon shows");
        assert!(apps
            .iter()
            .all(|app| app.name.chars().count() == MAX_FIELD_CHARS));
        let bytes = serde_json::to_vec(&presence).unwrap().len();
        assert!(
            bytes < 64 * 1024,
            "{bytes} bytes; the record's limit is 128 KiB"
        );
        assert_eq!(presence.clients.len(), 1, "the other clients stay listed");
    }

    #[test]
    fn a_failed_connect_read_is_retried_at_most_twice_and_only_while_connected() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let pauses = [Duration::from_millis(1), Duration::from_millis(1)];
        let calls = Cell::new(0);
        let attempts = runtime.block_on(refresh_with_retries(
            || true,
            || {
                calls.set(calls.get() + 1);
                async { Err::<Vec<String>, String>("down".into()) }
            },
            &pauses,
        ));
        assert_eq!((attempts, calls.get()), (3, 3), "three tries, then give up");
        let calls = Cell::new(0);
        let attempts = runtime.block_on(refresh_with_retries(
            || true,
            || {
                calls.set(calls.get() + 1);
                let ok = calls.get() == 2;
                async move {
                    if ok {
                        Ok(vec![])
                    } else {
                        Err("down".into())
                    }
                }
            },
            &pauses,
        ));
        assert_eq!(attempts, 2, "stops at the first success");
        let attempts = runtime.block_on(refresh_with_retries(
            || false,
            || async { Err::<Vec<String>, String>("down".into()) },
            &pauses,
        ));
        assert_eq!(attempts, 1, "no retry once disconnected");
    }
}
