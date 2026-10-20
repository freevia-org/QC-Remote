//! What the broker's icon shows, as plain data. No window, no lock, no I/O:
//! the state thread builds a [`Snapshot`], and the UI thread renders
//! [`presentation`] and [`icon_rgba`] from it.
use super::host_status::{display_text, HostClient, HostPresence, LinkedApp};
use crate::generated_broker_surface::TrayReading;
use crate::tray_settings::IdlePolicy;
use crate::worker::BrokerStatus;

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// The broker's admitted target, never a window's selected UI tab.
    /// Left unset until a provider is registered and selected by the broker.
    pub served_source: Option<crate::target::TargetSource>,
    pub reading: TrayReading,
    pub device_name: Option<String>,
    pub preset_name: Option<String>,
    pub preset_slot: Option<String>,
    pub quarantine: Option<String>,
    /// Providers of the authenticated participants, from the broker's own
    /// connection table: who is actually using the device. This process's own
    /// background MCP host is never among them
    /// ([`crate::rpc::BrokerHandle::participants_without_host`]): it is the
    /// broker's own machinery, and the AI apps it serves are listed from
    /// `host` instead.
    pub participants: Vec<String>,
    /// The background MCP host's status, or `None` when no live host proved
    /// its identity.
    pub host: Option<HostPresence>,
    pub idle: IdlePolicy,
    pub action_detail: String,
    /// Set by the UI thread when the state thread has stopped refreshing this
    /// snapshot: the broker is wedged somewhere the icon can still report.
    pub stale: bool,
    /// Wall-clock time the snapshot was taken, for "connected 5 min ago".
    /// Zero leaves every relative time out rather than guessing one.
    pub observed_at_unix_ms: u64,
}

/// How old a snapshot may get before the icon says the broker stopped
/// answering. The state thread refreshes every quarter second.
pub const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            served_source: None,
            reading: TrayReading::Disconnected,
            device_name: None,
            preset_name: None,
            preset_slot: None,
            quarantine: None,
            participants: Vec::new(),
            host: None,
            idle: IdlePolicy::Minutes(15),
            action_detail: String::new(),
            stale: false,
            observed_at_unix_ms: 0,
        }
    }
}

impl Snapshot {
    /// Device fields from the broker's cached status. Reading it sends nothing
    /// to the Quad Cortex.
    pub fn apply_status(&mut self, status: &BrokerStatus) {
        self.reading =
            TrayReading::from_phase(&status.phase, status.connected, status.synchronized);
        self.device_name = status.device_name.clone();
        self.preset_name = status.active_preset_name.clone();
        self.preset_slot = status.active_preset_location.clone();
        self.quarantine = status.writes_quarantined.then(|| {
            status
                .write_quarantine_reason
                .clone()
                .unwrap_or_else(|| "Writes quarantined".to_owned())
        });
    }

    fn connected(&self) -> bool {
        matches!(self.reading, TrayReading::Ready | TrayReading::Syncing)
    }
}

/// Every command the menu can issue. The UI thread only forwards these; the
/// state thread runs them against the broker.
///
/// A client's action names its session by the host's id, never by its row
/// position: the menu is rebuilt from a fresh snapshot on every open, and a
/// click must reach the client the user saw even if the list moved since.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MenuAction {
    OpenApp,
    Connect,
    Disconnect,
    SetIdle(Option<u32>),
    ReleaseAndExit,
    /// End one local MCP session held by this process's background host. It
    /// drops the host's record of that session (its presence lease, which is
    /// what counts it as a client) and stops the bridge's heartbeat from
    /// listing it again. It adds no block: that chat's next request links it
    /// again.
    EndLocalSession(String),
    /// Revoke the OAuth grant of one remote MCP session, after the user
    /// confirms. Names the session only: the host in this process resolves the
    /// grant from its own live relay presence. `app` is display text for the
    /// confirmation, never an address.
    UnlinkRemoteSession {
        session_id: String,
        app: String,
    },
    /// Revoke the OAuth grant of one linked remote app, after the user
    /// confirms, whether or not it has a live session. Names the host's opaque
    /// entry id only: the host resolves the grant from its own linked-apps
    /// list. `app` is display text for the confirmation, never an address.
    UnlinkLinkedApp {
        entry_id: String,
        app: String,
    },
}

/// The question a remote Unlink asks, one paragraph per line break.
pub fn unlink_question(app: &str) -> String {
    [
        format!("Unlink {app}?"),
        String::new(),
        "This ends this app's authorization to use your Quad Cortex through QC Remote Cloud. \
         It will need to sign in again before it can use QC Remote, and every session that \
         uses this authorization ends with it."
            .to_owned(),
        String::new(),
        "Other linked apps, your account and this computer's pairing are not affected.".to_owned(),
    ]
    .join("\n")
}

/// A question the icon asks natively before it forwards an action.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Confirmation {
    pub title: String,
    pub text: String,
}

/// The confirmation `action` needs, if any. Only a remote Unlink asks: it
/// cannot be undone from here, and the app must sign in again afterwards.
pub fn confirmation(action: &MenuAction) -> Option<Confirmation> {
    match action {
        MenuAction::UnlinkRemoteSession { app, .. } | MenuAction::UnlinkLinkedApp { app, .. } => {
            Some(Confirmation {
                title: "Unlink remote app".into(),
                text: unlink_question(app),
            })
        }
        _ => None,
    }
}

/// Forward a chosen menu action, asking first when it needs confirmation.
/// A declined confirmation forwards nothing.
pub fn dispatch(
    action: MenuAction,
    confirm: impl FnOnce(&Confirmation) -> bool,
    send: impl FnOnce(MenuAction),
) {
    if confirmation(&action).is_none_or(|question| confirm(&question)) {
        send(action);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MenuRow {
    Text(String),
    Action {
        label: String,
        enabled: bool,
        action: MenuAction,
    },
    Choices {
        label: String,
        choices: Vec<(String, bool, MenuAction)>,
    },
    /// A nested menu: one per client.
    Submenu {
        label: String,
        rows: Vec<MenuRow>,
    },
    Separator,
}

/// The actions of `rows` in the order the UI numbers them (depth first,
/// choices in order). A menu command id is the 1-based index into this list,
/// rebuilt with the menu on every open, so ids are unique within one menu and
/// no handler outlives it.
pub fn commands(rows: &[MenuRow]) -> Vec<MenuAction> {
    let mut out = Vec::new();
    for row in rows {
        match row {
            MenuRow::Action { action, .. } => out.push(action.clone()),
            MenuRow::Choices { choices, .. } => {
                out.extend(choices.iter().map(|(_, _, action)| action.clone()))
            }
            MenuRow::Submenu { rows, .. } => out.extend(commands(rows)),
            MenuRow::Text(_) | MenuRow::Separator => {}
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Presentation {
    pub tooltip: String,
    pub rows: Vec<MenuRow>,
}

/// The shell truncates a notification tooltip at 127 UTF-16 units.
pub const TOOLTIP_UNITS: usize = 127;

pub fn status_text(snapshot: &Snapshot) -> String {
    if snapshot.stale {
        return "QC Remote broker is not responding".into();
    }
    if !snapshot.connected() && snapshot.served_source == Some(crate::target::TargetSource::Demo) {
        return "QC Remote demo device ready (local simulation)".into();
    }
    let device = snapshot
        .device_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Quad Cortex");
    match snapshot.reading {
        TrayReading::Ready => {
            let preset = [
                snapshot.preset_slot.as_deref(),
                snapshot.preset_name.as_deref(),
            ]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            if preset.is_empty() {
                format!("{device} ready")
            } else {
                format!("{device} ready, Preset: {preset}")
            }
        }
        TrayReading::Syncing => format!("{device} Syncing…"),
        TrayReading::NoDevicePresent => format!("{device} not found; the broker keeps looking"),
        TrayReading::Disconnected | TrayReading::BrokerRunning | TrayReading::BrokerNotRunning => {
            format!("{device} Disconnected")
        }
    }
}

fn truncate_units(text: &str, units: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for character in text.chars() {
        let width = character.len_utf16();
        if used + width > units {
            break;
        }
        used += width;
        out.push(character);
    }
    out
}

fn client_label(name: &str) -> &str {
    match name {
        "codex-mcp-client" => "Codex",
        _ => name,
    }
}

/// Where a listed client was observed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClientSource {
    /// An authenticated broker participant other than this process's own
    /// host: the QC Remote window, or an AI app's bridge serving MCP itself.
    Participant { provider: String },
    /// A session the background MCP host published.
    Host(Box<HostClient>),
}

/// One menu entry: every session of one app, local and remote kept apart.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientGroup {
    label: String,
    remote: bool,
    sessions: Vec<ClientSource>,
}

/// Every client the menu lists, grouped.
///
/// What counts (the header and the entries agree by construction):
/// - **local**: each authenticated broker participant except this process's
///   own background MCP host (the window, or a bridge hosting itself), plus
///   each `localMcp` session the host published;
/// - **remote**: each `remoteMcp` session the host published (live relay
///   presence leases);
/// - never this process: the broker, its icon and its in-process MCP host are
///   machinery, not clients. `qcRemoteApp` rows of the status file are not
///   read either, since the window is already a participant.
///
/// Aggregation: sessions that show the same label on the same side
/// (local/remote) are one entry, in order of first appearance, so four Codex
/// bridges are one "Codex (4 sessions)" entry rather than four identical
/// rows. Nothing is merged away: the header counts sessions, the entry says
/// how many it holds, and each session keeps its own details and Unlink.
fn client_groups(snapshot: &Snapshot) -> Vec<ClientGroup> {
    let mut groups: Vec<ClientGroup> = Vec::new();
    let mut add = |label: String, remote: bool, source: ClientSource| match groups
        .iter_mut()
        .find(|group| group.label == label && group.remote == remote)
    {
        Some(group) => group.sessions.push(source),
        None => groups.push(ClientGroup {
            label,
            remote,
            sessions: vec![source],
        }),
    };
    for provider in &snapshot.participants {
        // The provider is whatever the client sent in its hello: shown only
        // through the same sanitizer as the host's record. A connection whose
        // name cannot be shown is still a client and is still counted.
        let provider = display_text(provider).unwrap_or_default();
        let label = match provider.as_str() {
            "" => "Unidentified client".to_owned(),
            name => client_label(name).to_owned(),
        };
        add(label, false, ClientSource::Participant { provider });
    }
    let linked = placing_linked_apps(snapshot);
    if let Some(host) = &snapshot.host {
        for client in &host.clients {
            // A remote session placed under a linked app is listed there.
            if linked.is_some_and(|apps| placed_in(client, apps).is_some()) {
                continue;
            }
            add(
                client_label(&client.name).to_owned(),
                client.remote,
                ClientSource::Host(Box::new(client.clone())),
            );
        }
    }
    for group in &mut groups {
        // Oldest first; a session with no known start keeps its place last.
        group.sessions.sort_by_key(|source| match source {
            ClientSource::Host(client) => client.connected_at_unix_ms.unwrap_or(u64::MAX),
            ClientSource::Participant { .. } => u64::MAX,
        });
    }
    groups
}

/// "2 local and 4 remote clients active". A side with nobody is left out.
/// The noun is singular only when every number shown is one ("1 local
/// client", "1 local and 1 remote client"): it is read as one client of each
/// kind. Any larger number makes it plural ("2 local and 1 remote clients"),
/// which reads as the total it is.
pub fn clients_summary(local: usize, remote: usize) -> String {
    let noun = |one: bool| if one { "client" } else { "clients" };
    match (local, remote) {
        (0, 0) => "No clients active".into(),
        (local, 0) => format!("{local} local {} active", noun(local == 1)),
        (0, remote) => format!("{remote} remote {} active", noun(remote == 1)),
        (local, remote) => format!(
            "{local} local and {remote} remote {} active",
            noun(local == 1 && remote == 1)
        ),
    }
}

/// The summary when the list may be incomplete. This process's own host is
/// never listed, so the list is complete only while that host's record is
/// fresh and its own: otherwise the host may be serving AI apps (and holding
/// the device) that the menu cannot see, and "No clients active" would be a
/// claim nobody measured.
pub const CLIENT_LIST_UNAVAILABLE: &str = "Client list unavailable";

/// Whether the remote clients' details show the IP address the relay saw.
/// Hidden by default: the tray is
/// visible over a shoulder or in a screen share, and the address is the
/// provider route's, not the user's. City and country stay. The app's
/// Remote panel shows it either way. Flip this one line to show it.
pub const SHOW_REMOTE_IP_ADDRESS: bool = false;

/// "just now", "4 min ago", "3 h ago", "2 days ago"; `None` when either
/// instant is unknown or the start is in the future.
fn ago(now_ms: u64, then_ms: Option<u64>) -> Option<String> {
    let then_ms = then_ms?;
    if now_ms == 0 || then_ms > now_ms {
        return None;
    }
    let seconds = (now_ms - then_ms) / 1_000;
    Some(match seconds {
        0..=59 => "just now".into(),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        _ => match seconds / 86_400 {
            1 => "1 day ago".into(),
            days => format!("{days} days ago"),
        },
    })
}

const REMOTE_UNLINK_HINT: &str = "Unlink remote apps in QC Remote's Remote panel";
/// A remote session the relay published without its grant: the cloud
/// predates remote Unlink from the icon. Never fall back to the client id.
const REMOTE_UNLINK_NEEDS_UPDATE: &str =
    "Unlink here needs a QC Remote Cloud update; use the Remote panel";

/// The disabled detail lines and the actions of one session. Only fields
/// the snapshot carries are shown; a missing one is left out, never guessed.
fn session_rows(source: &ClientSource, snapshot: &Snapshot) -> Vec<MenuRow> {
    session_rows_with(source, snapshot, SHOW_REMOTE_IP_ADDRESS)
}

fn session_rows_with(
    source: &ClientSource,
    snapshot: &Snapshot,
    show_remote_ip_address: bool,
) -> Vec<MenuRow> {
    let now = snapshot.observed_at_unix_ms;
    let mut details: Vec<String> = Vec::new();
    let mut actions: Vec<MenuRow> = Vec::new();
    match source {
        ClientSource::Participant { provider } => {
            details.push("Attached to the Quad Cortex".into());
            if !provider.is_empty() && client_label(provider) != provider {
                details.push(format!("Client: {provider}"));
            }
        }
        ClientSource::Host(client) if !client.remote => {
            details.push("Local MCP client on this PC".into());
            if client_label(&client.name) != client.name {
                details.push(format!("Client: {}", client.name));
            }
            if let Some(version) = &client.version {
                details.push(format!("Version {version}"));
            }
            if let Some(since) = ago(now, client.connected_at_unix_ms) {
                details.push(format!("Connected {since}"));
            }
            // Only this process's own host can end the session, and only
            // one it named.
            let in_process = snapshot
                .host
                .as_ref()
                .is_some_and(|host| host.published_by_this_process);
            if let (true, Some(session)) = (in_process, &client.session_id) {
                actions.push(MenuRow::Action {
                    label: "Unlink".into(),
                    enabled: true,
                    action: MenuAction::EndLocalSession(session.clone()),
                });
            }
        }
        ClientSource::Host(client) => {
            details.push("Remote MCP client".into());
            if let Some(version) = &client.version {
                details.push(format!("App {version}"));
            }
            let route = [client.device.as_deref(), client.browser.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            if !route.is_empty() {
                details.push(format!("Provider route: {route}"));
            }
            let location = [client.city.as_deref(), client.country.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            if !location.is_empty() {
                details.push(location);
            }
            if show_remote_ip_address {
                if let Some(address) = &client.ip_address {
                    details.push(address.clone());
                }
            }
            if let Some(since) = ago(now, client.connected_at_unix_ms) {
                details.push(format!("Connected {since}"));
            }
            if let Some(seen) = ago(now, client.last_seen_at_unix_ms) {
                details.push(format!("Last active {seen}"));
            }
            // Remote Unlink revokes the session's OAuth grant, which only
            // this process's own host can resolve (from its live relay
            // presence) and only when the relay named it.
            let in_process = snapshot
                .host
                .as_ref()
                .is_some_and(|host| host.published_by_this_process);
            match (in_process, client.grant_known, &client.session_id) {
                (true, true, Some(session)) => actions.push(MenuRow::Action {
                    label: "Unlink".into(),
                    enabled: true,
                    action: MenuAction::UnlinkRemoteSession {
                        session_id: session.clone(),
                        app: remote_app_text(client),
                    },
                }),
                (true, false, Some(_)) => details.push(REMOTE_UNLINK_NEEDS_UPDATE.into()),
                _ => details.push(REMOTE_UNLINK_HINT.into()),
            }
        }
    }
    let mut rows: Vec<MenuRow> = details.into_iter().map(MenuRow::Text).collect();
    if !actions.is_empty() {
        rows.push(MenuRow::Separator);
        rows.extend(actions);
    }
    rows
}

/// "Claude (remote, Sofia, BG)": how the confirmation names the app.
fn remote_app_text(client: &HostClient) -> String {
    let location = [client.city.as_deref(), client.country.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    let name = client_label(&client.name);
    if location.is_empty() {
        format!("{name} (remote)")
    } else {
        format!("{name} (remote, {location})")
    }
}

fn session_title(index: usize, source: &ClientSource, snapshot: &Snapshot) -> String {
    let since = match source {
        ClientSource::Host(client) => {
            ago(snapshot.observed_at_unix_ms, client.connected_at_unix_ms)
        }
        ClientSource::Participant { .. } => None,
    };
    match since {
        Some(since) => format!("Session {}, connected {since}", index + 1),
        None => format!("Session {}", index + 1),
    }
}

fn client_rows(snapshot: &Snapshot) -> (usize, usize, Vec<MenuRow>) {
    let groups = client_groups(snapshot);
    let count = |remote: bool| {
        groups
            .iter()
            .filter(|group| group.remote == remote)
            .map(|group| group.sessions.len())
            .sum::<usize>()
    };
    let mut rows: Vec<MenuRow> = groups
        .iter()
        .map(|group| {
            let name = if group.remote {
                format!("{} (remote)", group.label)
            } else {
                group.label.clone()
            };
            match group.sessions.as_slice() {
                [only] => MenuRow::Submenu {
                    label: name,
                    rows: session_rows(only, snapshot),
                },
                sessions => MenuRow::Submenu {
                    label: format!("{name} ({} sessions)", sessions.len()),
                    rows: sessions
                        .iter()
                        .enumerate()
                        .map(|(index, source)| MenuRow::Submenu {
                            label: session_title(index, source, snapshot),
                            rows: session_rows(source, snapshot),
                        })
                        .collect(),
                },
            }
        })
        .collect();
    let mut remote = count(true);
    if let Some(apps) = loaded_linked_apps(snapshot) {
        let activity = activity_known(snapshot);
        for app in apps {
            let sessions = linked_app_sessions(snapshot, app);
            remote += sessions.len();
            rows.push(MenuRow::Submenu {
                label: linked_app_label(app, apps, snapshot),
                rows: linked_app_rows(app, apps, &sessions, activity, snapshot),
            });
        }
    }
    (count(false), remote, rows)
}

/// Shown when the host could not fetch the account's linked apps: the
/// remote entries are then the live sessions alone, as before the list.
pub const LINKED_APPS_UNAVAILABLE: &str =
    "Linked remote apps could not be loaded; showing active sessions only";

/// Shown when live remote sessions do not name their grant (a relay deployed
/// before `grantId`): which linked app is in use cannot be told, so no entry
/// claims to be idle. Known sessions stay under their app; unmapped ones
/// retain their own rows rather than invalidating positive known activity.
pub const LINKED_ACTIVITY_NEEDS_UPDATE: &str =
    "Some active sessions cannot be matched to linked apps; showing them separately";

/// The account's linked remote apps, when the host has a current list.
fn loaded_linked_apps(snapshot: &Snapshot) -> Option<&[LinkedApp]> {
    snapshot
        .host
        .as_ref()?
        .linked_apps
        .as_ref()
        .filter(|linked| linked.loaded)
        .map(|linked| linked.apps.as_slice())
}

/// Whether every live remote session names its grant, so that a linked app
/// with no session placed under it is really idle. True with no remote
/// session at all: then every linked app is idle, whatever the relay.
fn activity_known(snapshot: &Snapshot) -> bool {
    snapshot
        .host
        .iter()
        .flat_map(|host| host.clients.iter())
        .filter(|client| client.remote)
        .all(|client| client.grant_known && client.linked_app_id.is_some())
}

/// The linked apps sessions are placed under, when placement can be trusted.
fn placing_linked_apps(snapshot: &Snapshot) -> Option<&[LinkedApp]> {
    loaded_linked_apps(snapshot)
}

/// The linked app a remote session was placed under by the host, if listed.
fn placed_in<'a>(client: &HostClient, apps: &'a [LinkedApp]) -> Option<&'a LinkedApp> {
    let id = client.linked_app_id.as_deref().filter(|_| client.remote)?;
    apps.iter().find(|app| app.entry_id == id)
}

/// The live sessions of one linked app, oldest first.
fn linked_app_sessions(snapshot: &Snapshot, app: &LinkedApp) -> Vec<HostClient> {
    let mut sessions: Vec<HostClient> = snapshot
        .host
        .iter()
        .flat_map(|host| host.clients.iter())
        .filter(|client| {
            client.remote && client.linked_app_id.as_deref() == Some(app.entry_id.as_str())
        })
        .cloned()
        .collect();
    sessions.sort_by_key(|client| client.connected_at_unix_ms.unwrap_or(u64::MAX));
    sessions
}

/// "Claude (remote)"; when two linked apps share a name, what tells them
/// apart: where each was authorized and when.
fn linked_app_base_label(app: &LinkedApp, apps: &[LinkedApp], snapshot: &Snapshot) -> String {
    let name = client_label(&app.name);
    if apps.iter().filter(|other| other.name == app.name).count() < 2 {
        return format!("{name} (remote)");
    }
    let linked = ago(snapshot.observed_at_unix_ms, app.authorized_at_unix_ms)
        .map(|since| format!("linked {since}"));
    let detail = [app.authorized_from.clone(), linked]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    if detail.is_empty() {
        format!("{name} (remote)")
    } else {
        format!("{name} (remote, {detail})")
    }
}

/// The entry's menu label, unique within the menu: two entries that would
/// still read the same get the shortest unique suffix (at least four characters)
/// (stable for as long as the grant stays listed).
fn linked_app_label(app: &LinkedApp, apps: &[LinkedApp], snapshot: &Snapshot) -> String {
    let base = linked_app_base_label(app, apps, snapshot);
    let twins = apps
        .iter()
        .filter(|other| linked_app_base_label(other, apps, snapshot) == base)
        .count();
    if twins < 2 {
        return base;
    }
    let chars: Vec<char> = app.entry_id.chars().collect();
    let peers: Vec<Vec<char>> = apps
        .iter()
        .filter(|other| {
            other.entry_id != app.entry_id && linked_app_base_label(other, apps, snapshot) == base
        })
        .map(|other| other.entry_id.chars().collect())
        .collect();
    let width = (4..=chars.len().max(4))
        .find(|width| {
            let suffix = &chars[chars.len().saturating_sub(*width)..];
            peers
                .iter()
                .all(|other| suffix != &other[other.len().saturating_sub(*width)..])
        })
        .unwrap_or(chars.len());
    let tag: String = chars[chars.len().saturating_sub(width)..].iter().collect();
    format!("{base} #{tag}")
}

/// The details of one linked app, its live sessions, and its Unlink.
fn linked_app_rows(
    app: &LinkedApp,
    apps: &[LinkedApp],
    sessions: &[HostClient],
    activity_known: bool,
    snapshot: &Snapshot,
) -> Vec<MenuRow> {
    let now = snapshot.observed_at_unix_ms;
    let mut details: Vec<String> = Vec::new();
    details.push(match (activity_known, sessions.len()) {
        // No claim either way: its sessions cannot be told from others'.
        (false, 0) => "Linked".into(),
        (true, 0) => "Linked, idle".into(),
        (_, 1) => "Active now".into(),
        (_, live) => format!("Active now ({live} sessions)"),
    });
    if let Some(version) = &app.app_version {
        details.push(format!("App {version}"));
    }
    if activity_known && sessions.is_empty() {
        if let Some(seen) = ago(now, app.last_active_at_unix_ms) {
            details.push(format!("Last active {seen}"));
        }
    }
    if let Some(from) = &app.authorized_from {
        details.push(format!("Authorized from {from}"));
    }
    let location = [app.city.as_deref(), app.country.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    if !location.is_empty() {
        details.push(location);
    }
    if let Some(access) = &app.access {
        details.push(access.clone());
    }
    if let Some(since) = ago(now, app.authorized_at_unix_ms) {
        details.push(format!("Linked {since}"));
    }
    let mut rows: Vec<MenuRow> = details.into_iter().map(MenuRow::Text).collect();
    if !sessions.is_empty() {
        rows.push(MenuRow::Separator);
        rows.extend(sessions.iter().enumerate().map(|(index, client)| {
            let source = ClientSource::Host(Box::new(client.clone()));
            MenuRow::Submenu {
                label: session_title(index, &source, snapshot),
                // A session's own details; the app's Unlink sits on the app.
                rows: session_rows(&source, snapshot)
                    .into_iter()
                    .take_while(|row| *row != MenuRow::Separator)
                    .filter(|row| {
                        !matches!(row, MenuRow::Text(text)
                            if text == REMOTE_UNLINK_HINT || text == REMOTE_UNLINK_NEEDS_UPDATE)
                    })
                    .collect(),
            }
        }));
    }
    let in_process = snapshot
        .host
        .as_ref()
        .is_some_and(|host| host.published_by_this_process);
    if in_process {
        rows.push(MenuRow::Separator);
        rows.push(MenuRow::Action {
            label: "Unlink".into(),
            enabled: true,
            action: MenuAction::UnlinkLinkedApp {
                entry_id: app.entry_id.clone(),
                app: linked_app_label(app, apps, snapshot),
            },
        });
    } else {
        rows.push(MenuRow::Text(REMOTE_UNLINK_HINT.into()));
    }
    rows
}

/// What the summary does not say about remote apps: how many linked apps
/// are idle (listed, never counted as active), that the list is missing, or
/// that the relay cannot yet say which linked app is active.
fn linked_apps_notes(snapshot: &Snapshot) -> Vec<MenuRow> {
    let Some(linked) = snapshot
        .host
        .as_ref()
        .and_then(|host| host.linked_apps.as_ref())
    else {
        return Vec::new();
    };
    if !linked.loaded {
        return vec![MenuRow::Text(LINKED_APPS_UNAVAILABLE.into())];
    }
    if !activity_known(snapshot) {
        return vec![MenuRow::Text(LINKED_ACTIVITY_NEEDS_UPDATE.into())];
    }
    let idle = linked
        .apps
        .iter()
        .filter(|app| linked_app_sessions(snapshot, app).is_empty())
        .count();
    match idle {
        0 => Vec::new(),
        1 => vec![MenuRow::Text("1 linked remote app idle".into())],
        idle => vec![MenuRow::Text(format!("{idle} linked remote apps idle"))],
    }
}

/// The idle-release submenu. The period runs from the moment the last
/// attached client disconnects; every attached client holds the device,
/// including an AI app's bridge that is connected but idle (it keeps a live
/// presence lease). "Idle" never meant "no request lately", so the label
/// says what actually starts the clock.
const IDLE_RELEASE_LABEL: &str = "Release QC after last client";

pub fn idle_label(minutes: Option<u32>) -> String {
    match minutes {
        None => "Never".into(),
        Some(0) => "Immediately".into(),
        Some(1) => "1 minute".into(),
        Some(60) => "1 hour".into(),
        Some(480) => "8 hours".into(),
        Some(1440) => "1 day".into(),
        Some(minutes) if minutes % 60 == 0 => format!("{} hours", minutes / 60),
        Some(minutes) => format!("{minutes} minutes"),
    }
}

const IDLE_CHOICES: [Option<u32>; 7] = [
    Some(0),
    Some(15),
    Some(30),
    Some(60),
    Some(480),
    Some(1440),
    None,
];

fn idle_choices(current: Option<u32>) -> Vec<(String, bool, MenuAction)> {
    let mut choices: Vec<Option<u32>> = IDLE_CHOICES.to_vec();
    // A custom period set in the app stays selectable rather than vanishing.
    if !choices.contains(&current) {
        let at = choices
            .iter()
            .position(|choice| choice.is_none_or(|minutes| current < Some(minutes)))
            .unwrap_or(choices.len());
        choices.insert(at, current);
    }
    choices
        .into_iter()
        .map(|minutes| {
            (
                idle_label(minutes),
                minutes == current,
                MenuAction::SetIdle(minutes),
            )
        })
        .collect()
}

pub fn presentation(snapshot: &Snapshot) -> Presentation {
    let status = status_text(snapshot);
    let mut rows = vec![MenuRow::Action {
        label: status.clone(),
        enabled: true,
        action: MenuAction::OpenApp,
    }];
    rows.push(MenuRow::Text(match &snapshot.host {
        Some(HostPresence {
            remote_endpoint: Some(url),
            ..
        })
        | Some(HostPresence {
            endpoint: Some(url),
            ..
        }) => format!("MCP endpoint: {url}"),
        Some(_) => "MCP endpoint: unavailable".into(),
        // Only the background host publishes a status file. While the app
        // window serves MCP itself there is none, so say unknown, not off.
        None => "MCP status unavailable".into(),
    }));
    rows.push(if snapshot.connected() {
        // No count here: the summary below says who is active, and a count
        // of broker participants alone undercounted the AI apps.
        MenuRow::Action {
            label: "Disconnect Quad Cortex".into(),
            enabled: true,
            action: MenuAction::Disconnect,
        }
    } else {
        MenuRow::Action {
            label: "Connect Quad Cortex".into(),
            enabled: true,
            action: MenuAction::Connect,
        }
    });
    let mut notices: Vec<&str> = Vec::new();
    if let Some(quarantine) = snapshot.quarantine.as_deref() {
        notices.push(quarantine);
    }
    notices.push(&snapshot.action_detail);
    notices.retain(|text| !text.trim().is_empty());
    notices.dedup();
    rows.extend(
        notices
            .into_iter()
            .map(|text| MenuRow::Text(text.to_owned())),
    );

    rows.push(MenuRow::Separator);
    let (local, remote, clients) = client_rows(snapshot);
    let complete = snapshot
        .host
        .as_ref()
        .is_some_and(|host| host.published_by_this_process);
    rows.push(MenuRow::Text(if complete {
        clients_summary(local, remote)
    } else {
        CLIENT_LIST_UNAVAILABLE.into()
    }));
    rows.extend(linked_apps_notes(snapshot));
    rows.extend(clients);

    rows.push(MenuRow::Separator);
    let current = snapshot.idle.minutes();
    rows.push(MenuRow::Choices {
        label: format!("{IDLE_RELEASE_LABEL}: {}", idle_label(current)),
        choices: idle_choices(current),
    });
    rows.push(MenuRow::Action {
        label: "Quit".into(),
        enabled: true,
        action: MenuAction::ReleaseAndExit,
    });
    Presentation {
        tooltip: truncate_units(&status, TOOLTIP_UNITS),
        rows,
    }
}

pub use super::icon::{
    icon_color, icon_rgba, sync_brightness, tone_rgba, IconTone, ICON_SIZE, SYNC_FRAME_COUNT,
};

pub fn icon_tone(snapshot: &Snapshot) -> IconTone {
    if snapshot.stale {
        return IconTone::Error;
    }
    if !snapshot.connected() && snapshot.served_source == Some(crate::target::TargetSource::Demo) {
        return IconTone::Demo;
    }
    // A stale quarantine reason can outlive the USB link; with no device there
    // is nothing to act on, so absent is inactive gray, not attention amber.
    if !snapshot.connected() {
        return IconTone::Inactive;
    }
    if snapshot
        .quarantine
        .as_deref()
        .is_some_and(|reason| !reason.trim().is_empty())
    {
        return IconTone::Attention;
    }
    match snapshot.reading {
        TrayReading::Ready => IconTone::Ready,
        _ => IconTone::Syncing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_palette_requires_the_brokers_demo_target_and_yields_to_physical() {
        let mut snapshot = Snapshot::default();
        assert_eq!(icon_tone(&snapshot), IconTone::Inactive);
        snapshot.served_source = Some(crate::target::TargetSource::Demo);
        assert_eq!(icon_tone(&snapshot), IconTone::Demo);
        assert_eq!(icon_color(IconTone::Demo), [166, 112, 232, 255]);
        assert_eq!(tone_rgba(IconTone::Demo, 0), tone_rgba(IconTone::Demo, 3));
        snapshot.reading = TrayReading::Ready;
        assert_eq!(icon_tone(&snapshot), IconTone::Ready);
        snapshot.stale = true;
        assert_eq!(icon_tone(&snapshot), IconTone::Error);
    }
    use crate::tray::host_status::LinkedApps;

    fn labels(model: &Presentation) -> Vec<String> {
        model
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Action { label, .. } | MenuRow::Text(label) => Some(label.clone()),
                MenuRow::Choices { label, .. } | MenuRow::Submenu { label, .. } => {
                    Some(label.clone())
                }
                MenuRow::Separator => None,
            })
            .collect()
    }

    fn submenu<'a>(model: &'a Presentation, label: &str) -> &'a [MenuRow] {
        model
            .rows
            .iter()
            .find_map(|row| match row {
                MenuRow::Submenu { label: name, rows } if name == label => Some(rows.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {label}"))
    }

    const NOW: u64 = 10_000_000;

    fn local(name: &str, session: &str, connected_at: Option<u64>) -> HostClient {
        HostClient {
            name: name.into(),
            remote: false,
            session_id: Some(session.into()),
            connected_at_unix_ms: connected_at,
            ..HostClient::default()
        }
    }

    fn remote_claude() -> HostClient {
        HostClient {
            name: "Claude".into(),
            remote: true,
            session_id: Some("relay-1".into()),
            // As a relay that predates grants publishes it.
            grant_known: false,
            linked_app_id: None,
            version: Some("1.0.3".into()),
            device: Some("Windows".into()),
            browser: Some("Chrome".into()),
            ip_address: Some("192.0.2.7".into()),
            country: Some("BG".into()),
            city: Some("Sofia".into()),
            connected_at_unix_ms: Some(NOW - 600_000),
            last_seen_at_unix_ms: Some(NOW - 5_000),
        }
    }

    /// A typical case: the window, this process's own host (filtered out by
    /// the broker before the snapshot), four Codex bridges and one remote
    /// Claude.
    fn busy() -> Snapshot {
        Snapshot {
            participants: vec!["QC Remote window".into()],
            host: Some(HostPresence {
                endpoint: Some("http://127.0.0.1:5000/mcp".into()),
                remote_endpoint: None,
                clients: vec![
                    local("codex-mcp-client", "stdio-d", Some(NOW - 60_000)),
                    local("codex-mcp-client", "stdio-a", Some(NOW - 3_600_000)),
                    local("codex-mcp-client", "stdio-b", None),
                    local("codex-mcp-client", "stdio-c", Some(NOW - 120_000)),
                    remote_claude(),
                ],
                linked_apps: None,
                published_by_this_process: true,
            }),
            observed_at_unix_ms: NOW,
            ..ready()
        }
    }

    /// A fresh record this process's own host wrote.
    fn own_host(clients: Vec<HostClient>) -> HostPresence {
        HostPresence {
            clients,
            published_by_this_process: true,
            ..HostPresence::default()
        }
    }

    fn ready() -> Snapshot {
        Snapshot {
            reading: TrayReading::Ready,
            preset_slot: Some("7F".into()),
            preset_name: Some("Example preset".into()),
            ..Snapshot::default()
        }
    }

    #[test]
    fn status_reads_the_brokers_own_status_without_a_round_trip() {
        let mut snapshot = Snapshot::default();
        let mut status = BrokerStatus {
            phase: "ready".into(),
            connected: true,
            synchronized: true,
            device_name: Some("Stage QC".into()),
            active_preset_name: Some("Lead".into()),
            active_preset_location: Some("2B".into()),
            ..BrokerStatus::default()
        };
        snapshot.apply_status(&status);
        assert_eq!(snapshot.reading, TrayReading::Ready);
        assert_eq!(status_text(&snapshot), "Stage QC ready, Preset: 2B Lead");
        status.synchronized = false;
        snapshot.apply_status(&status);
        assert_eq!(status_text(&snapshot), "Stage QC Syncing…");
        status.phase = "disconnected".into();
        status.connected = false;
        status.writes_quarantined = true;
        snapshot.apply_status(&status);
        assert_eq!(snapshot.reading, TrayReading::Disconnected);
        assert_eq!(snapshot.quarantine.as_deref(), Some("Writes quarantined"));
        assert_eq!(icon_tone(&snapshot), IconTone::Inactive);
    }

    #[test]
    fn one_connect_or_disconnect_row_without_a_client_count() {
        let mut snapshot = ready();
        let model = presentation(&snapshot);
        assert_eq!(
            model.tooltip,
            "Quad Cortex ready, Preset: 7F Example preset"
        );
        assert_eq!(
            model.rows[0],
            MenuRow::Action {
                label: model.tooltip.clone(),
                enabled: true,
                action: MenuAction::OpenApp
            }
        );
        assert!(model.rows.contains(&MenuRow::Action {
            label: "Disconnect Quad Cortex".into(),
            enabled: true,
            action: MenuAction::Disconnect
        }));
        snapshot.participants = vec!["QC Remote window".into(), "codex-mcp-client".into()];
        let model = presentation(&snapshot);
        assert!(model.rows.contains(&MenuRow::Action {
            label: "Disconnect Quad Cortex".into(),
            enabled: true,
            action: MenuAction::Disconnect
        }));
        assert!(!labels(&model)
            .iter()
            .any(|label| label.starts_with("Disconnect Quad Cortex (")));
        assert!(labels(&model).contains(&"Codex".to_owned()));
        assert!(labels(&model).contains(&CLIENT_LIST_UNAVAILABLE.to_owned()));
        snapshot.host = Some(own_host(vec![]));
        assert!(labels(&presentation(&snapshot)).contains(&"2 local clients active".to_owned()));
        snapshot.reading = TrayReading::Disconnected;
        let model = presentation(&snapshot);
        assert!(model.rows.contains(&MenuRow::Action {
            label: "Connect Quad Cortex".into(),
            enabled: true,
            action: MenuAction::Connect
        }));
        assert_eq!(
            labels(&model)
                .iter()
                .filter(|label| label.contains("onnect Quad Cortex"))
                .count(),
            1
        );
    }

    #[test]
    fn the_last_row_is_quit_and_still_releases_and_exits() {
        let mut snapshot = ready();
        for participants in [vec![], vec!["QC Remote window".to_owned()]] {
            snapshot.participants = participants;
            let model = presentation(&snapshot);
            assert_eq!(
                model.rows.last(),
                Some(&MenuRow::Action {
                    label: "Quit".into(),
                    enabled: true,
                    action: MenuAction::ReleaseAndExit
                })
            );
        }
    }

    #[test]
    fn host_rows_say_so_when_no_live_host_proved_itself() {
        let mut snapshot = ready();
        assert!(labels(&presentation(&snapshot)).contains(&"MCP status unavailable".to_owned()));
        assert!(labels(&presentation(&snapshot)).contains(&CLIENT_LIST_UNAVAILABLE.to_owned()));
        snapshot.host = Some(HostPresence {
            endpoint: Some("http://127.0.0.1:5000/mcp".into()),
            remote_endpoint: None,
            clients: vec![
                HostClient {
                    name: "Claude".into(),
                    remote: true,
                    ..HostClient::default()
                },
                HostClient {
                    name: "codex-mcp-client".into(),
                    remote: false,
                    ..HostClient::default()
                },
            ],
            linked_apps: None,
            published_by_this_process: false,
        });
        let shown = labels(&presentation(&snapshot));
        assert!(shown.contains(&"MCP endpoint: http://127.0.0.1:5000/mcp".to_owned()));
        assert!(shown.contains(&"Claude (remote)".to_owned()));
        assert!(shown.contains(&"Codex".to_owned()));
        snapshot.host.as_mut().unwrap().remote_endpoint = Some("https://relay.example/mcp".into());
        assert!(labels(&presentation(&snapshot))
            .contains(&"MCP endpoint: https://relay.example/mcp".to_owned()));
    }

    #[test]
    fn idle_choices_mark_the_current_period_and_keep_a_custom_one() {
        let mut snapshot = ready();
        let choices = |snapshot: &Snapshot| {
            presentation(snapshot)
                .rows
                .into_iter()
                .find_map(|row| match row {
                    MenuRow::Choices { label, choices } => Some((label, choices)),
                    _ => None,
                })
                .unwrap()
        };
        let (label, rows) = choices(&snapshot);
        assert_eq!(label, "Release QC after last client: 15 minutes");
        assert_eq!(
            rows.iter()
                .filter(|(_, checked, _)| *checked)
                .collect::<Vec<_>>(),
            [&("15 minutes".to_owned(), true, MenuAction::SetIdle(Some(15)))]
        );
        assert_eq!(rows.first().unwrap().2, MenuAction::SetIdle(Some(0)));
        assert_eq!(rows.last().unwrap().2, MenuAction::SetIdle(None));
        snapshot.idle = IdlePolicy::Minutes(23);
        let (label, rows) = choices(&snapshot);
        assert_eq!(label, "Release QC after last client: 23 minutes");
        let names: Vec<_> = rows.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Immediately",
                "15 minutes",
                "23 minutes",
                "30 minutes",
                "1 hour",
                "8 hours",
                "1 day",
                "Never"
            ]
        );
        snapshot.idle = IdlePolicy::Never;
        assert_eq!(choices(&snapshot).0, "Release QC after last client: Never");
        assert_eq!(idle_label(Some(120)), "2 hours");
    }

    #[test]
    fn notices_appear_only_when_present_and_the_tooltip_fits_the_shell() {
        let mut snapshot = ready();
        let plain = presentation(&snapshot).rows.len();
        snapshot.quarantine = Some("Confirm recovery".into());
        snapshot.action_detail = "Waiting for the device".into();
        let model = presentation(&snapshot);
        assert_eq!(model.rows.len(), plain + 2);
        assert_eq!(icon_tone(&snapshot), IconTone::Attention);
        snapshot.preset_name = Some("x".repeat(400));
        let tooltip = presentation(&snapshot).tooltip;
        assert_eq!(tooltip.encode_utf16().count(), TOOLTIP_UNITS);
        assert_eq!(truncate_units("ab😀", 3), "ab");
    }

    #[test]
    fn the_summary_counts_local_and_remote_sessions_with_agreement() {
        assert_eq!(clients_summary(0, 0), "No clients active");
        assert_eq!(clients_summary(1, 0), "1 local client active");
        assert_eq!(clients_summary(3, 0), "3 local clients active");
        assert_eq!(clients_summary(0, 1), "1 remote client active");
        assert_eq!(clients_summary(0, 2), "2 remote clients active");
        assert_eq!(clients_summary(0, 3), "3 remote clients active");
        assert_eq!(clients_summary(2, 4), "2 local and 4 remote clients active");
        assert_eq!(clients_summary(1, 1), "1 local and 1 remote client active");
        assert_eq!(clients_summary(2, 1), "2 local and 1 remote clients active");
        assert_eq!(clients_summary(1, 3), "1 local and 3 remote clients active");
    }

    #[test]
    fn a_missing_stale_or_foreign_record_never_reads_as_no_clients() {
        // The own host is never listed, so without its fresh record the menu
        // cannot know whether it serves anyone (and holds the device).
        let mut snapshot = ready();
        assert!(
            snapshot.host.is_none(),
            "missing or stale: read() said None"
        );
        let shown = labels(&presentation(&snapshot));
        assert!(shown.contains(&CLIENT_LIST_UNAVAILABLE.to_owned()));
        assert!(!shown.contains(&"No clients active".to_owned()));
        snapshot.participants = vec!["QC Remote window".into()];
        let shown = labels(&presentation(&snapshot));
        assert!(shown.contains(&CLIENT_LIST_UNAVAILABLE.to_owned()));
        assert!(
            shown.contains(&"QC Remote window".to_owned()),
            "what is known is still listed"
        );
        snapshot.host = Some(HostPresence {
            published_by_this_process: false,
            ..own_host(vec![])
        });
        assert!(labels(&presentation(&snapshot)).contains(&CLIENT_LIST_UNAVAILABLE.to_owned()));
        snapshot.participants.clear();
        snapshot.host = Some(own_host(vec![]));
        assert!(labels(&presentation(&snapshot)).contains(&"No clients active".to_owned()));
    }

    #[test]
    fn participant_names_go_through_the_shared_sanitizer() {
        let mut snapshot = ready();
        snapshot.host = Some(own_host(vec![]));
        snapshot.participants = vec![
            "QC \u{202E}Remote window".into(),
            "bad\u{1b}[2J".into(),
            "z".repeat(400),
        ];
        let shown = labels(&presentation(&snapshot));
        assert!(shown.contains(&"3 local clients active".to_owned()));
        assert!(shown.contains(&"QC Remote window".to_owned()));
        assert!(
            shown.contains(&"Unidentified client".to_owned()),
            "an unshowable name is still a client and still counted"
        );
        assert!(shown.contains(&"z".repeat(super::super::host_status::MAX_FIELD_CHARS)));
        assert!(!format!("{shown:?}").contains('\u{1b}'));
        assert!(!format!("{shown:?}").contains('\u{202E}'));
    }

    #[test]
    fn the_owners_case_lists_each_client_once_without_titles_or_its_own_process() {
        let model = presentation(&busy());
        assert_eq!(
            labels(&model),
            [
                "Quad Cortex ready, Preset: 7F Example preset",
                "MCP endpoint: http://127.0.0.1:5000/mcp",
                "Disconnect Quad Cortex",
                "5 local and 1 remote clients active",
                "QC Remote window",
                "Codex (4 sessions)",
                "Claude (remote)",
                "Release QC after last client: 15 minutes",
                "Quit",
            ]
        );
        for gone in [
            "Using the Quad Cortex",
            "AI apps",
            "No client attached",
            "No AI app active",
            "QC Remote background host",
        ] {
            assert!(
                !format!("{model:?}").contains(gone),
                "{gone} must not appear anywhere in the menu"
            );
        }
    }

    #[test]
    fn each_client_is_a_submenu_of_details_and_local_ones_end_by_session() {
        let model = presentation(&busy());
        assert_eq!(
            submenu(&model, "QC Remote window"),
            [MenuRow::Text("Attached to the Quad Cortex".into())],
            "the window holds the device through the broker; no host session to end"
        );
        let codex = submenu(&model, "Codex (4 sessions)");
        let titles: Vec<_> = codex
            .iter()
            .map(|row| match row {
                MenuRow::Submenu { label, .. } => label.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            titles,
            [
                "Session 1, connected 1 h ago",
                "Session 2, connected 2 min ago",
                "Session 3, connected 1 min ago",
                "Session 4"
            ],
            "oldest first, a session with no known start last"
        );
        let MenuRow::Submenu { rows: oldest, .. } = &codex[0] else {
            unreachable!()
        };
        assert_eq!(
            oldest,
            &[
                MenuRow::Text("Local MCP client on this PC".into()),
                MenuRow::Text("Client: codex-mcp-client".into()),
                MenuRow::Text("Connected 1 h ago".into()),
                MenuRow::Separator,
                MenuRow::Action {
                    label: "Unlink".into(),
                    enabled: true,
                    action: MenuAction::EndLocalSession("stdio-a".into())
                },
            ]
        );
        assert_eq!(
            submenu(&model, "Claude (remote)"),
            [
                MenuRow::Text("Remote MCP client".into()),
                MenuRow::Text("App 1.0.3".into()),
                MenuRow::Text("Provider route: Windows Chrome".into()),
                MenuRow::Text("Sofia, BG".into()),
                MenuRow::Text("Connected 10 min ago".into()),
                MenuRow::Text("Last active just now".into()),
                MenuRow::Text(REMOTE_UNLINK_NEEDS_UPDATE.into()),
            ],
            "no grant, no remote Unlink (never by client id), and the IP hidden by default"
        );
    }

    /// Two linked devices of one app, each under its own grant.
    fn two_remote_grants() -> Snapshot {
        let mut snapshot = ready();
        snapshot.observed_at_unix_ms = NOW;
        snapshot.host = Some(own_host(vec![
            HostClient {
                session_id: Some("relay-1".into()),
                grant_known: true,
                ..remote_claude()
            },
            HostClient {
                session_id: Some("relay-2".into()),
                grant_known: true,
                city: Some("Plovdiv".into()),
                connected_at_unix_ms: Some(NOW - 60_000),
                ..remote_claude()
            },
        ]));
        snapshot
    }

    #[test]
    fn each_remote_grant_gets_its_own_unlink_routed_by_its_session() {
        let model = presentation(&two_remote_grants());
        let claude = submenu(&model, "Claude (remote) (2 sessions)");
        assert_eq!(claude.len(), 2, "one submenu per session");
        let unlinks: Vec<_> = commands(&model.rows)
            .into_iter()
            .filter(|action| matches!(action, MenuAction::UnlinkRemoteSession { .. }))
            .collect();
        assert_eq!(
            unlinks,
            [
                MenuAction::UnlinkRemoteSession {
                    session_id: "relay-1".into(),
                    app: "Claude (remote, Sofia, BG)".into(),
                },
                MenuAction::UnlinkRemoteSession {
                    session_id: "relay-2".into(),
                    app: "Claude (remote, Plovdiv, BG)".into(),
                },
            ]
        );
        let MenuRow::Submenu { rows: first, .. } = &claude[0] else {
            unreachable!()
        };
        assert_eq!(
            first.last(),
            Some(&MenuRow::Action {
                label: "Unlink".into(),
                enabled: true,
                action: unlinks[0].clone(),
            })
        );
        assert!(!format!("{model:?}").contains(REMOTE_UNLINK_NEEDS_UPDATE));
        // A record another process wrote: details and the panel hint only.
        let mut foreign = two_remote_grants();
        foreign.host.as_mut().unwrap().published_by_this_process = false;
        let model = presentation(&foreign);
        assert!(!commands(&model.rows)
            .iter()
            .any(|action| matches!(action, MenuAction::UnlinkRemoteSession { .. })));
        assert!(format!("{model:?}").contains(REMOTE_UNLINK_HINT));
    }

    #[test]
    fn a_remote_unlink_asks_first_and_a_declined_question_sends_nothing() {
        let action = MenuAction::UnlinkRemoteSession {
            session_id: "relay-2".into(),
            app: "Claude (remote, Plovdiv, BG)".into(),
        };
        let question = confirmation(&action).expect("remote Unlink asks");
        assert_eq!(question.title, "Unlink remote app");
        assert_eq!(
            question.text,
            "Unlink Claude (remote, Plovdiv, BG)?\n\nThis ends this app's authorization to use \
             your Quad Cortex through QC Remote Cloud. It will need to sign in again before it \
             can use QC Remote, and every session that uses this authorization ends with it.\n\n\
             Other linked apps, your account and this computer's pairing are not affected."
        );
        assert!(!question.text.contains("  "), "no runs of spaces");
        let linked = MenuAction::UnlinkLinkedApp {
            entry_id: "app-2".into(),
            app: "Claude (remote, Plovdiv, BG)".into(),
        };
        assert_eq!(
            confirmation(&linked),
            Some(question.clone()),
            "same question"
        );
        let mut sent = Vec::new();
        dispatch(action.clone(), |_| false, |action| sent.push(action));
        assert!(sent.is_empty(), "declined: no call");
        dispatch(action.clone(), |_| true, |action| sent.push(action));
        assert_eq!(sent, [action]);
        // Nothing else asks.
        let mut sent = Vec::new();
        dispatch(
            MenuAction::EndLocalSession("stdio-a".into()),
            |_| panic!("a local Unlink does not ask"),
            |action| sent.push(action),
        );
        assert_eq!(sent.len(), 1);
        assert_eq!(confirmation(&MenuAction::Disconnect), None);
    }

    fn linked(entry: &str, name: &str, from: &str, authorized_ago_ms: u64) -> LinkedApp {
        LinkedApp {
            entry_id: entry.into(),
            name: name.into(),
            access: Some("Control access".into()),
            authorized_from: Some(from.into()),
            city: Some("Sofia".into()),
            country: Some("BG".into()),
            authorized_at_unix_ms: Some(NOW - authorized_ago_ms),
            last_active_at_unix_ms: Some(NOW - 5_400_000),
            app_version: None,
        }
    }

    /// A typical case: one app linked from two devices (two grants), one of
    /// them with two live sessions, plus an idle linked app with none.
    fn linked_devices() -> Snapshot {
        let mut snapshot = ready();
        snapshot.observed_at_unix_ms = NOW;
        let session = |id: &str, entry: &str, age: u64| HostClient {
            session_id: Some(id.into()),
            grant_known: true,
            linked_app_id: Some(entry.into()),
            connected_at_unix_ms: Some(NOW - age),
            ..remote_claude()
        };
        snapshot.host = Some(HostPresence {
            clients: vec![
                local("codex-mcp-client", "stdio-a", Some(NOW - 60_000)),
                session("relay-1", "app-1", 600_000),
                session("relay-2", "app-1", 60_000),
                // Live, under a grant the list does not hold (linked after
                // the last read): shown as a session, as before.
                HostClient {
                    session_id: Some("relay-9".into()),
                    name: "ChatGPT web".into(),
                    grant_known: true,
                    linked_app_id: Some("app-9".into()),
                    ..remote_claude()
                },
            ],
            linked_apps: Some(LinkedApps {
                loaded: true,
                apps: vec![
                    linked("app-1", "Claude", "Windows Chrome 140", 7_200_000),
                    linked("app-2", "Claude", "Android Chrome 139", 3_600_000),
                    linked("app-3", "Gemini web", "Mac Safari 18", 1_800_000),
                ],
            }),
            published_by_this_process: true,
            ..HostPresence::default()
        });
        snapshot
    }

    #[test]
    fn linked_activity_keeps_known_sessions_but_cannot_infer_idle_from_an_unmapped_grant() {
        let mut snapshot = linked_devices();
        snapshot
            .host
            .as_mut()
            .unwrap()
            .clients
            .last_mut()
            .unwrap()
            .linked_app_id = None;
        // The last active remote session has a known grant but no linked-app id.
        assert!(
            !activity_known(&snapshot),
            "grant-known alone cannot map activity"
        );
        let model = presentation(&snapshot);
        let all = labels(&model);
        assert!(all.contains(&"1 local and 3 remote clients active".into()));
        assert!(!all.iter().any(|label| label.ends_with("remote apps idle")));
        let known = submenu(
            &model,
            "Claude (remote, Windows Chrome 140, linked 2 h ago)",
        );
        assert_eq!(known[0], MenuRow::Text("Active now (2 sessions)".into()));
        let uncertain = submenu(
            &model,
            "Claude (remote, Android Chrome 139, linked 1 h ago)",
        );
        assert_eq!(uncertain[0], MenuRow::Text("Linked".into()));
        assert!(
            all.contains(&"ChatGPT web (remote)".into()),
            "unmapped session stays visible"
        );
    }

    #[test]
    fn linked_app_labels_disambiguate_ids_with_the_same_four_character_suffix() {
        let snapshot = ready();
        let apps = vec![
            linked("first-aaaa", "Claude", "Android", 0),
            linked("second-aaaa", "Claude", "Android", 0),
        ];
        let first = linked_app_label(&apps[0], &apps, &snapshot);
        let second = linked_app_label(&apps[1], &apps, &snapshot);
        assert_ne!(
            first, second,
            "same trailing four characters do not identify different grants"
        );
        assert_ne!(
            confirmation(&MenuAction::UnlinkLinkedApp {
                entry_id: apps[0].entry_id.clone(),
                app: first
            }),
            confirmation(&MenuAction::UnlinkLinkedApp {
                entry_id: apps[1].entry_id.clone(),
                app: second
            })
        );
    }

    #[test]
    fn each_linked_app_is_one_entry_with_its_live_sessions_and_its_own_unlink() {
        let model = presentation(&linked_devices());
        let all = labels(&model);
        assert!(
            all.contains(&"1 local and 3 remote clients active".to_owned()),
            "active counts live sessions only: {all:?}"
        );
        assert!(
            all.contains(&"2 linked remote apps idle".to_owned()),
            "{all:?}"
        );
        let first = submenu(
            &model,
            "Claude (remote, Windows Chrome 140, linked 2 h ago)",
        );
        assert_eq!(first[0], MenuRow::Text("Active now (2 sessions)".into()));
        assert!(first.contains(&MenuRow::Text("Authorized from Windows Chrome 140".into())));
        assert!(first.contains(&MenuRow::Text("Control access".into())));
        assert!(first.contains(&MenuRow::Text("Linked 2 h ago".into())));
        let nested: Vec<_> = first
            .iter()
            .filter_map(|row| match row {
                MenuRow::Submenu { label, rows } => Some((label.as_str(), rows)),
                _ => None,
            })
            .collect();
        assert_eq!(
            nested.len(),
            2,
            "the two live sessions sit under their grant"
        );
        assert_eq!(nested[0].0, "Session 1, connected 10 min ago");
        assert!(
            nested
                .iter()
                .all(|(_, rows)| !rows.iter().any(|row| matches!(row, MenuRow::Action { .. }))),
            "a session has details only; Unlink belongs to the app"
        );
        let idle = submenu(
            &model,
            "Claude (remote, Android Chrome 139, linked 1 h ago)",
        );
        assert_eq!(idle[0], MenuRow::Text("Linked, idle".into()));
        assert!(idle.contains(&MenuRow::Text("Last active 1 h ago".into())));
        let unlinks: Vec<_> = commands(&model.rows)
            .into_iter()
            .filter(|action| {
                matches!(
                    action,
                    MenuAction::UnlinkLinkedApp { .. } | MenuAction::UnlinkRemoteSession { .. }
                )
            })
            .collect();
        assert_eq!(
            unlinks,
            [
                MenuAction::UnlinkRemoteSession {
                    session_id: "relay-9".into(),
                    app: "ChatGPT web (remote, Sofia, BG)".into(),
                },
                MenuAction::UnlinkLinkedApp {
                    entry_id: "app-1".into(),
                    app: "Claude (remote, Windows Chrome 140, linked 2 h ago)".into(),
                },
                MenuAction::UnlinkLinkedApp {
                    entry_id: "app-2".into(),
                    app: "Claude (remote, Android Chrome 139, linked 1 h ago)".into(),
                },
                MenuAction::UnlinkLinkedApp {
                    entry_id: "app-3".into(),
                    app: "Gemini web (remote)".into(),
                },
            ],
            "one Unlink per grant, idle ones included; the unlisted session keeps its own"
        );
    }

    #[test]
    fn with_a_relay_that_names_no_grants_no_linked_app_claims_to_be_idle() {
        // The deployed relay before grantId: no session can be placed.
        let mut snapshot = linked_devices();
        for client in &mut snapshot.host.as_mut().unwrap().clients {
            if client.remote {
                client.grant_known = false;
                client.linked_app_id = None;
            }
        }
        let model = presentation(&snapshot);
        let all = labels(&model);
        assert!(
            all.contains(&"1 local and 3 remote clients active".to_owned()),
            "{all:?}"
        );
        assert!(
            all.contains(&LINKED_ACTIVITY_NEEDS_UPDATE.to_owned()),
            "{all:?}"
        );
        assert!(
            !all.iter().any(|label| label.contains("idle")),
            "no idle count: {all:?}"
        );
        for label in [
            "Claude (remote, Windows Chrome 140, linked 2 h ago)",
            "Claude (remote, Android Chrome 139, linked 1 h ago)",
            "Gemini web (remote)",
        ] {
            let rows = submenu(&model, label);
            assert_eq!(rows[0], MenuRow::Text("Linked".into()), "{label}");
            assert!(!rows.iter().any(|row| matches!(row, MenuRow::Text(text)
                if text.starts_with("Last active") || text.starts_with("Active now")
                    || text == "Linked, idle")));
            assert!(!rows
                .iter()
                .any(|row| matches!(row, MenuRow::Submenu { .. })));
        }
        // The live sessions keep their own rows, with the update note.
        let sessions = submenu(&model, "Claude (remote) (2 sessions)");
        assert_eq!(sessions.len(), 2);
        assert!(submenu(&model, "ChatGPT web (remote)")
            .contains(&MenuRow::Text(REMOTE_UNLINK_NEEDS_UPDATE.into())));
        // Unlink per linked app still works: it needs only the list.
        assert_eq!(
            commands(&model.rows)
                .iter()
                .filter(|action| matches!(action, MenuAction::UnlinkLinkedApp { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn one_idle_linked_app_reads_in_the_singular() {
        let mut snapshot = ready();
        snapshot.observed_at_unix_ms = NOW;
        snapshot.host = Some(HostPresence {
            linked_apps: Some(LinkedApps {
                loaded: true,
                apps: vec![linked("app-1", "Claude", "Windows Chrome 140", 60_000)],
            }),
            published_by_this_process: true,
            ..HostPresence::default()
        });
        let all = labels(&presentation(&snapshot));
        assert!(all.contains(&"No clients active".to_owned()), "{all:?}");
        assert!(
            all.contains(&"1 linked remote app idle".to_owned()),
            "{all:?}"
        );
    }

    #[test]
    fn linked_apps_that_would_read_the_same_get_a_stable_tag() {
        let mut snapshot = ready();
        snapshot.observed_at_unix_ms = NOW;
        snapshot.host = Some(HostPresence {
            linked_apps: Some(LinkedApps {
                loaded: true,
                apps: vec![
                    linked("app-0000aa11", "Claude", "Windows Chrome 140", 3_600_000),
                    linked("app-0000bb22", "Claude", "Windows Chrome 140", 3_700_000),
                    linked("app-0000cc33", "Claude", "Android Chrome 139", 3_600_000),
                ],
            }),
            published_by_this_process: true,
            ..HostPresence::default()
        });
        let model = presentation(&snapshot);
        let all = labels(&model);
        for label in [
            "Claude (remote, Windows Chrome 140, linked 1 h ago) #aa11",
            "Claude (remote, Windows Chrome 140, linked 1 h ago) #bb22",
            "Claude (remote, Android Chrome 139, linked 1 h ago)",
        ] {
            assert!(all.contains(&label.to_owned()), "{label} in {all:?}");
        }
        let questions: Vec<_> = commands(&model.rows)
            .iter()
            .filter_map(confirmation)
            .map(|question| question.text)
            .collect();
        assert_eq!(questions.len(), 3);
        assert!(questions[0] != questions[1], "the dialogs differ too");
    }

    #[test]
    fn a_failed_list_falls_back_to_sessions_with_a_note_and_a_foreign_record_offers_no_unlink() {
        let mut snapshot = linked_devices();
        snapshot.host.as_mut().unwrap().linked_apps = Some(LinkedApps {
            loaded: false,
            apps: vec![],
        });
        let model = presentation(&snapshot);
        let all = labels(&model);
        assert!(all.contains(&LINKED_APPS_UNAVAILABLE.to_owned()), "{all:?}");
        assert!(all.contains(&"1 local and 3 remote clients active".to_owned()));
        let claude = submenu(&model, "Claude (remote) (2 sessions)");
        assert_eq!(claude.len(), 2, "per session, as before the list");
        assert!(!commands(&model.rows)
            .iter()
            .any(|action| matches!(action, MenuAction::UnlinkLinkedApp { .. })));
        // No list published at all: no note, sessions as before.
        let mut none = linked_devices();
        none.host.as_mut().unwrap().linked_apps = None;
        let model = presentation(&none);
        assert!(!labels(&model)
            .iter()
            .any(|label| label.contains("linked remote app")));
        // Another process's record: details and the panel hint, no Unlink.
        let mut foreign = linked_devices();
        foreign.host.as_mut().unwrap().published_by_this_process = false;
        let model = presentation(&foreign);
        assert!(!commands(&model.rows).iter().any(|action| matches!(
            action,
            MenuAction::UnlinkLinkedApp { .. } | MenuAction::UnlinkRemoteSession { .. }
        )));
        assert!(submenu(&model, "Gemini web (remote)")
            .contains(&MenuRow::Text(REMOTE_UNLINK_HINT.into())));
    }

    #[test]
    fn the_remote_ip_address_is_one_switch_and_hidden_by_default() {
        // Hidden by default; showing it changes this pin too.
        const { assert!(!SHOW_REMOTE_IP_ADDRESS) };
        let snapshot = busy();
        let remote = ClientSource::Host(Box::new(remote_claude()));
        let hidden = session_rows_with(&remote, &snapshot, false);
        let shown = session_rows_with(&remote, &snapshot, true);
        let address = MenuRow::Text("192.0.2.7".into());
        assert!(!hidden.contains(&address));
        assert!(shown.contains(&address));
        assert!(hidden.contains(&MenuRow::Text("Sofia, BG".into())));
        assert_eq!(shown.len(), hidden.len() + 1, "nothing else changes");
        assert_eq!(session_rows(&remote, &snapshot), hidden);
    }

    #[test]
    fn unlink_routes_to_the_session_it_was_shown_for_and_ids_never_collide() {
        let model = presentation(&busy());
        let actions = commands(&model.rows);
        let unlinks: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                MenuAction::EndLocalSession(session) => Some(session.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(unlinks, ["stdio-a", "stdio-c", "stdio-d", "stdio-b"]);
        assert!(!actions.is_empty(), "the busy menu must expose actions");
        for (index, action) in actions.iter().enumerate() {
            assert!(
                !actions[index + 1..].contains(action),
                "{action:?} has two menu ids"
            );
        }
        // A rebuilt menu numbers the same snapshot the same way.
        assert_eq!(commands(&presentation(&busy()).rows), actions);
        // A session that ended between two opens takes only its own item
        // away; the others keep naming their own sessions.
        let mut fewer = busy();
        fewer.host.as_mut().unwrap().clients.remove(1);
        let after = commands(&presentation(&fewer).rows);
        assert!(!after.contains(&MenuAction::EndLocalSession("stdio-a".into())));
        assert!(after.contains(&MenuAction::EndLocalSession("stdio-d".into())));
    }

    #[test]
    fn a_single_session_is_a_flat_submenu_and_a_foreign_host_offers_no_unlink() {
        let mut snapshot = ready();
        snapshot.observed_at_unix_ms = NOW;
        snapshot.host = Some(HostPresence {
            clients: vec![HostClient {
                version: Some("0.9".into()),
                ..local("Claude Desktop", "stdio-x", None)
            }],
            published_by_this_process: true,
            ..HostPresence::default()
        });
        let model = presentation(&snapshot);
        assert!(labels(&model).contains(&"1 local client active".to_owned()));
        assert_eq!(
            submenu(&model, "Claude Desktop"),
            [
                MenuRow::Text("Local MCP client on this PC".into()),
                MenuRow::Text("Version 0.9".into()),
                MenuRow::Separator,
                MenuRow::Action {
                    label: "Unlink".into(),
                    enabled: true,
                    action: MenuAction::EndLocalSession("stdio-x".into())
                },
            ]
        );
        // A record another process wrote names sessions this process's host
        // does not hold: details only.
        snapshot.host.as_mut().unwrap().published_by_this_process = false;
        let model = presentation(&snapshot);
        assert!(!commands(&model.rows)
            .iter()
            .any(|action| matches!(action, MenuAction::EndLocalSession(_))));
        // Without a clock no relative time is guessed.
        snapshot.observed_at_unix_ms = 0;
        snapshot.host.as_mut().unwrap().clients[0].connected_at_unix_ms = Some(NOW);
        assert!(!format!("{:?}", presentation(&snapshot)).contains("Connected"));
    }

    #[test]
    fn local_and_remote_sessions_of_one_name_stay_apart() {
        let mut snapshot = ready();
        snapshot.host = Some(HostPresence {
            clients: vec![
                local("Claude", "stdio-1", None),
                HostClient {
                    name: "Claude".into(),
                    remote: true,
                    ..HostClient::default()
                },
            ],
            ..HostPresence::default()
        });
        snapshot.host.as_mut().unwrap().published_by_this_process = true;
        snapshot.participants = vec![" ".into()];
        let shown = labels(&presentation(&snapshot));
        assert!(shown.contains(&"2 local and 1 remote clients active".to_owned()));
        assert!(shown.contains(&"Claude".to_owned()));
        assert!(shown.contains(&"Claude (remote)".to_owned()));
        assert!(shown.contains(&"Unidentified client".to_owned()));
    }

    #[test]
    fn relative_times_round_down_and_refuse_the_future() {
        assert_eq!(ago(NOW, Some(NOW)), Some("just now".into()));
        assert_eq!(ago(NOW, Some(NOW - 59_999)), Some("just now".into()));
        assert_eq!(ago(NOW, Some(NOW - 60_000)), Some("1 min ago".into()));
        assert_eq!(ago(NOW, Some(NOW - 7_200_000)), Some("2 h ago".into()));
        assert_eq!(
            ago(200_000_000, Some(100_000_000)),
            Some("1 day ago".into())
        );
        assert_eq!(ago(NOW, Some(NOW + 1)), None);
        assert_eq!(ago(NOW, None), None);
        assert_eq!(ago(0, Some(1)), None);
    }

    #[test]
    fn tones_follow_the_shared_led_palette() {
        let mut snapshot = ready();
        assert_eq!(icon_tone(&snapshot), IconTone::Ready);
        snapshot.reading = TrayReading::Syncing;
        assert_eq!(icon_tone(&snapshot), IconTone::Syncing);
        snapshot.reading = TrayReading::NoDevicePresent;
        assert_eq!(icon_tone(&snapshot), IconTone::Inactive);
        assert_eq!(icon_color(IconTone::Ready), [64, 196, 118, 255]);
        assert_eq!(icon_color(IconTone::Syncing), [91, 164, 231, 255]);
        assert_eq!(icon_color(IconTone::Attention), [240, 184, 60, 255]);
        assert_eq!(icon_color(IconTone::Inactive), [140, 145, 151, 255]);
        assert_eq!(icon_color(IconTone::Error), [226, 73, 73, 255]);
        // A wedged state thread outranks every device reading.
        snapshot.reading = TrayReading::Ready;
        snapshot.stale = true;
        assert_eq!(icon_tone(&snapshot), IconTone::Error);
        assert_eq!(status_text(&snapshot), "QC Remote broker is not responding");
        snapshot.stale = false;
        assert_eq!(icon_tone(&snapshot), IconTone::Ready);
    }

    #[test]
    fn the_icon_is_the_qc_housing_and_only_sync_brightness_animates() {
        let pixel = |rgba: &[u8], x: usize, y: usize| -> [u8; 4] {
            rgba[(y * ICON_SIZE + x) * 4..(y * ICON_SIZE + x) * 4 + 4]
                .try_into()
                .unwrap()
        };
        let ready = tone_rgba(IconTone::Ready, 0);
        let dim = tone_rgba(IconTone::Syncing, 0);
        let bright = tone_rgba(IconTone::Syncing, 2);
        assert_eq!(ready.len(), ICON_SIZE * ICON_SIZE * 4);
        assert_eq!(pixel(&ready, 0, 0)[3], 0);
        assert_eq!(pixel(&ready, 16, 0)[3], 255);
        assert_eq!(pixel(&dim, 4, 4), pixel(&bright, 4, 4));
        let center = pixel(&ready, 16, 16);
        assert!(center[1] > center[0] && center[1] > center[2]);
        assert!(pixel(&bright, 16, 16)[2] > pixel(&dim, 16, 16)[2]);
        assert_eq!(
            tone_rgba(IconTone::Ready, 0),
            tone_rgba(IconTone::Ready, 3),
            "only syncing animates"
        );
        let inactive = pixel(&tone_rgba(IconTone::Inactive, 0), 16, 16);
        assert_eq!(&inactive[..3], &[98, 99, 103]);
    }
}
