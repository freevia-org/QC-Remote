//! Linux StatusNotifierItem adapter for the broker's shared tray model.
//!
//! `ksni::TrayMethods::spawn` can keep its service alive while the desktop's
//! StatusNotifierWatcher starts. Registration state controls the device idle
//! hold, so an unavailable watcher never creates a phantom hold.

use super::model::{self, MenuAction, MenuRow, Snapshot};
use super::state::{Notify, Shared};
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Sender, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const ITEM_ID: &str = "qc-remote-device-broker";
const STARTUP_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);
static ACTIVE_HANDLE: OnceLock<Mutex<Option<ksni::blocking::Handle<LinuxTray>>>> = OnceLock::new();

pub fn interactive_session() -> bool {
    let graphical = ["DISPLAY", "WAYLAND_DISPLAY"]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let session_bus =
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|value| !value.is_empty());
    graphical && session_bus
}

pub fn start(shared: Arc<Shared>, actions: Sender<MenuAction>) -> Result<Notify, String> {
    if !interactive_session() {
        return Err("No graphical D-Bus user session is available".into());
    }
    shared.registered.store(false, Ordering::Release);
    // ksni does not call watcher_online after an initially successful
    // registration. Start optimistic only during spawn so an initial
    // watcher_offline callback can distinguish ServiceUnknown from success.
    // The shared registration bit remains false until spawn has completed.
    let watcher_online = Arc::new(AtomicBool::new(true));
    let tray = LinuxTray {
        shared: Arc::clone(&shared),
        actions,
        watcher_online: Arc::clone(&watcher_online),
        sync_frame: 0,
    };
    let watcher_connection = Arc::new(
        zbus::blocking::connection::Builder::session()
            .map_err(|error| format!("Cannot connect to session D-Bus: {error}"))?
            .method_timeout(Duration::from_millis(500))
            .build()
            .map_err(|error| format!("Cannot connect to session D-Bus: {error}"))?,
    );
    // With this option, ksni treats a temporarily missing watcher as offline
    // and listens for its later appearance. Its service retries registration
    // on that transition and already handles later watcher loss/recovery.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .map_err(|error| format!("StatusNotifierItem registration failed: {error}"))?;
    let initially_registered = registration_is_active(
        watcher_online.load(Ordering::Acquire),
        true, // the watcher callback records whether the initial ACK was received
        host_is_registered(&watcher_connection),
    );
    shared
        .registered
        .store(initially_registered, Ordering::Release);
    let ever_registered = Arc::new(AtomicBool::new(initially_registered));
    let retired = Arc::new(AtomicBool::new(false));
    let started_at = Instant::now();
    *ACTIVE_HANDLE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle.clone());
    Ok(Box::new(move || {
        if retired.load(Ordering::Acquire) {
            return;
        }
        let updated = handle.update(LinuxTray::advance_frame).is_some();
        let host_registered =
            watcher_online.load(Ordering::Acquire) && host_is_registered(&watcher_connection);
        update_registration(&shared, &watcher_online, updated, host_registered);
        if shared.registered.load(Ordering::Acquire) {
            ever_registered.store(true, Ordering::Release);
        }
        if startup_registration_expired(
            ever_registered.load(Ordering::Acquire),
            started_at,
            Instant::now(),
        ) && !retired.swap(true, Ordering::AcqRel)
        {
            // No watcher appeared during startup. Withdraw this SNI service;
            // when ksni drops LinuxTray its action sender disconnects, so the
            // state loop releases residency and the hold through its existing
            // UI-disconnected path. Once registered even once, watcher loss
            // stays recoverable without a timeout.
            shared.registered.store(false, Ordering::Release);
            take_active_handle();
            let _ = handle.shutdown();
        }
    }))
}

pub fn remove_icon_now() {
    let handle = take_active_handle();
    if let Some(handle) = handle {
        // Queue withdrawal, then let the broker's exit path continue without
        // waiting on the desktop's D-Bus loop. Process exit closes the SNI
        // connection even if the host is no longer answering.
        let _ = handle.shutdown();
    }
}

fn take_active_handle() -> Option<ksni::blocking::Handle<LinuxTray>> {
    ACTIVE_HANDLE.get().and_then(|active| {
        active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    })
}

fn startup_registration_expired(ever_registered: bool, started_at: Instant, now: Instant) -> bool {
    !ever_registered && now.saturating_duration_since(started_at) >= STARTUP_REGISTRATION_TIMEOUT
}

struct LinuxTray {
    shared: Arc<Shared>,
    actions: Sender<MenuAction>,
    watcher_online: Arc<AtomicBool>,
    sync_frame: u8,
}

impl LinuxTray {
    fn snapshot(&self) -> Snapshot {
        let mut snapshot = self
            .shared
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let refreshed = *self
            .shared
            .refreshed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.stale = Instant::now().saturating_duration_since(refreshed) > model::STALE_AFTER;
        snapshot
    }

    fn icon(&self) -> Icon {
        let snapshot = self.snapshot();
        let mut data = model::tone_rgba(model::icon_tone(&snapshot), self.sync_frame);
        rgba_to_argb(&mut data);
        Icon {
            width: model::ICON_SIZE as i32,
            height: model::ICON_SIZE as i32,
            data,
        }
    }

    fn menu_rows(&self) -> Vec<MenuItem<Self>> {
        let snapshot = self.snapshot();
        let presentation = model::presentation(&snapshot);
        convert_rows(&presentation.rows, &self.actions)
    }

    fn advance_frame(&mut self) {
        self.sync_frame = if model::icon_tone(&self.snapshot()) == model::IconTone::Syncing {
            (self.sync_frame + 1) % model::SYNC_FRAME_COUNT
        } else {
            0
        };
    }
}

fn rgba_to_argb(data: &mut [u8]) {
    let (pixels, remainder) = data.as_chunks_mut::<4>();
    debug_assert!(remainder.is_empty(), "RGBA icon data has whole pixels");
    for pixel in pixels {
        pixel.rotate_right(1); // RGBA -> ARGB, as required by SNI.
    }
}

impl ksni::Tray for LinuxTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        ITEM_ID.into()
    }

    fn title(&self) -> String {
        "QC Remote".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn status(&self) -> ksni::Status {
        if !self.shared.wanted.load(Ordering::Acquire) {
            // Keep the SNI registered so the same resident host can restore
            // Active status when the preference is enabled again. Passive is
            // the protocol's hide request; it also releases the device hold.
            return ksni::Status::Passive;
        }
        match model::icon_tone(&self.snapshot()) {
            model::IconTone::Attention | model::IconTone::Error => ksni::Status::NeedsAttention,
            _ => ksni::Status::Active,
        }
    }

    fn icon_name(&self) -> String {
        // IconName takes precedence over IconPixmap on common SNI hosts. This
        // item draws its own live status glyph, so a theme name would replace
        // it with a generic sound-card icon.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon()]
    }

    fn tool_tip(&self) -> ToolTip {
        let presentation = model::presentation(&self.snapshot());
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: vec![self.icon()],
            title: "QC Remote".into(),
            description: escape_markup(&presentation.tooltip),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.menu_rows()
    }

    fn watcher_online(&self) {
        self.watcher_online.store(true, Ordering::Release);
        self.shared.registered.store(false, Ordering::Release);
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        self.watcher_online.store(false, Ordering::Release);
        self.shared.registered.store(false, Ordering::Release);
        true
    }
}

fn convert_rows(rows: &[MenuRow], actions: &Sender<MenuAction>) -> Vec<MenuItem<LinuxTray>> {
    rows.iter()
        .map(|row| match row {
            MenuRow::Text(label) => StandardItem {
                label: label.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuRow::Action {
                label,
                enabled,
                action,
            } => {
                let actions = actions.clone();
                let action = action.clone();
                StandardItem {
                    label: label.clone(),
                    enabled: *enabled,
                    activate: Box::new(move |_| dispatch(action.clone(), actions.clone())),
                    ..Default::default()
                }
                .into()
            }
            MenuRow::Choices { label, choices } => {
                let selected = choices
                    .iter()
                    .position(|(_, selected, _)| *selected)
                    .unwrap_or(0);
                let options = choices
                    .iter()
                    .map(|(label, _, _)| RadioItem {
                        label: label.clone(),
                        ..Default::default()
                    })
                    .collect();
                let actions_for_choice: Vec<_> = choices
                    .iter()
                    .map(|(_, _, action)| action.clone())
                    .collect();
                let sender = actions.clone();
                let group: MenuItem<LinuxTray> = RadioGroup {
                    selected,
                    options,
                    select: Box::new(move |_, index| {
                        if let Some(action) = actions_for_choice.get(index).cloned() {
                            dispatch(action, sender.clone());
                        }
                    }),
                }
                .into();
                SubMenu {
                    label: label.clone(),
                    submenu: vec![group],
                    ..Default::default()
                }
                .into()
            }
            MenuRow::Submenu { label, rows } => SubMenu {
                label: label.clone(),
                submenu: convert_rows(rows, actions),
                ..Default::default()
            }
            .into(),
            MenuRow::Separator => MenuItem::Separator,
        })
        .collect()
}

fn dispatch(action: MenuAction, actions: Sender<MenuAction>) {
    if model::confirmation(&action).is_none() {
        let _ = actions.send(action);
        return;
    }
    // Dialogs and destructive confirmations stay off ksni's D-Bus callback
    // thread. If no supported native dialog exists, fail closed.
    let _ = std::thread::Builder::new()
        .name("qc-broker-tray-confirm".into())
        .spawn(move || {
            model::dispatch(action, confirm_destructive, |confirmed| {
                let _ = actions.send(confirmed);
            });
        });
}

fn host_is_registered(connection: &zbus::blocking::Connection) -> bool {
    zbus::blocking::Proxy::new(
        connection,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )
    .and_then(|proxy| proxy.get_property::<bool>("IsStatusNotifierHostRegistered"))
    .unwrap_or(false)
}

fn registration_is_active(
    watcher_online: bool,
    update_succeeded: bool,
    host_registered: bool,
) -> bool {
    watcher_online && update_succeeded && host_registered
}

fn update_registration(
    shared: &Shared,
    watcher_online: &AtomicBool,
    update_succeeded: bool,
    host_registered: bool,
) {
    // ksni services this update on its D-Bus loop after attempting watcher
    // re-registration. The separate host property is required too. Neither a
    // watcher-online callback nor an item update alone restores the hold.
    shared.registered.store(
        registration_is_active(
            watcher_online.load(Ordering::Acquire),
            update_succeeded,
            host_registered,
        ),
        Ordering::Release,
    );
}

fn confirm_destructive(question: &model::Confirmation) -> bool {
    let mut zenity = Command::new("zenity");
    zenity
        .args([
            "--question",
            "--modal",
            "--no-markup",
            "--ok-label=Unlink",
            "--cancel-label=Cancel",
        ])
        .arg(format!("--title={}", question.title))
        .arg(format!("--text={}", question.text))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match zenity.status() {
        Ok(status) => return status.success(),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return false,
        Err(_) => {}
    }

    let mut kdialog = Command::new("kdialog");
    kdialog
        .arg("--yesno")
        .arg(&question.text)
        .arg("--title")
        .arg(&question.title)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    kdialog.status().is_ok_and(|status| status.success())
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    #[test]
    fn icon_pixels_are_reordered_from_rgba_to_argb_without_crossing_boundaries() {
        let mut pixels = [1, 2, 3, 4, 10, 20, 30, 40];
        rgba_to_argb(&mut pixels);
        assert_eq!(pixels, [4, 1, 2, 3, 40, 10, 20, 30]);
    }

    #[test]
    fn markup_from_device_status_is_not_interpreted_by_the_desktop() {
        assert_eq!(
            escape_markup("Amp <One> & Two"),
            "Amp &lt;One&gt; &amp; Two"
        );
    }

    #[test]
    fn model_actions_map_to_the_same_depth_first_order() {
        let snapshot = Snapshot::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let rows = model::presentation(&snapshot).rows;
        let mapped = convert_rows(&rows, &sender);
        assert_eq!(mapped.len(), rows.len());
        let shared = Arc::new(Shared {
            snapshot: std::sync::Mutex::new(snapshot),
            wanted: std::sync::atomic::AtomicBool::new(false),
            registered: std::sync::atomic::AtomicBool::new(false),
            refreshed: std::sync::Mutex::new(std::time::Instant::now()),
        });
        let mut tray = LinuxTray {
            shared,
            actions: sender.clone(),
            watcher_online: Arc::new(AtomicBool::new(true)),
            sync_frame: 0,
        };
        match &mapped[0] {
            MenuItem::Standard(item) => (item.activate)(&mut tray),
            _ => panic!("Open App should be an action row"),
        }
        assert_eq!(receiver.try_recv(), Ok(MenuAction::OpenApp));

        let choice_index = rows
            .iter()
            .position(|row| matches!(row, MenuRow::Choices { .. }))
            .expect("idle policy radio group");
        match &mapped[choice_index] {
            MenuItem::SubMenu(submenu) => match &submenu.submenu[0] {
                MenuItem::RadioGroup(group) => (group.select)(&mut tray, 0),
                _ => panic!("shared choices should use a registered radio group"),
            },
            _ => panic!("shared choices should be a labeled submenu"),
        }
        assert_eq!(receiver.try_recv(), Ok(MenuAction::SetIdle(Some(0))));
    }

    #[test]
    fn tray_hold_tracks_registration_loss_and_recovery() {
        let shared = Shared {
            snapshot: std::sync::Mutex::new(Snapshot::default()),
            wanted: std::sync::atomic::AtomicBool::new(true),
            registered: std::sync::atomic::AtomicBool::new(true),
            refreshed: std::sync::Mutex::new(std::time::Instant::now()),
        };
        let watcher_online = AtomicBool::new(false);

        update_registration(&shared, &watcher_online, true, true);
        assert!(!shared.registered.load(Ordering::Acquire));

        watcher_online.store(true, Ordering::Release);
        update_registration(&shared, &watcher_online, false, true);
        assert!(!shared.registered.load(Ordering::Acquire));

        update_registration(&shared, &watcher_online, true, true);
        assert!(shared.registered.load(Ordering::Acquire));

        watcher_online.store(false, Ordering::Release);
        update_registration(&shared, &watcher_online, true, true);
        assert!(!shared.registered.load(Ordering::Acquire));
    }

    #[test]
    fn host_property_and_registration_ack_are_both_required_for_hold() {
        let shared = Shared {
            snapshot: std::sync::Mutex::new(Snapshot::default()),
            wanted: AtomicBool::new(true),
            registered: AtomicBool::new(false),
            refreshed: std::sync::Mutex::new(Instant::now()),
        };
        let watcher_online = AtomicBool::new(true);

        update_registration(&shared, &watcher_online, true, false);
        assert!(!shared.registered.load(Ordering::Acquire));
        assert_eq!(
            crate::tray::hold_for(
                shared.registered.load(Ordering::Acquire),
                crate::tray_settings::TraySettings::default()
            ),
            None
        );

        update_registration(&shared, &watcher_online, false, true);
        assert!(!shared.registered.load(Ordering::Acquire));
        update_registration(&shared, &watcher_online, true, true);
        assert!(shared.registered.load(Ordering::Acquire));
    }

    #[test]
    fn missing_watcher_keeps_hold_off_until_late_registration_succeeds() {
        let shared = Arc::new(Shared {
            snapshot: std::sync::Mutex::new(Snapshot::default()),
            wanted: AtomicBool::new(true),
            registered: AtomicBool::new(false),
            refreshed: std::sync::Mutex::new(Instant::now()),
        });
        let watcher_online = Arc::new(AtomicBool::new(true));
        let (actions, _received) = std::sync::mpsc::channel();
        let tray = LinuxTray {
            shared: Arc::clone(&shared),
            actions,
            watcher_online: Arc::clone(&watcher_online),
            sync_frame: 0,
        };

        // Initial ServiceUnknown is a recoverable offline state. The
        // watcher callback alone must not create the device idle hold.
        assert!(tray.watcher_offline(ksni::OfflineReason::No));
        assert!(!shared.registered.load(Ordering::Acquire));
        assert_eq!(
            crate::tray::hold_for(
                shared.registered.load(Ordering::Acquire),
                crate::tray_settings::TraySettings::default()
            ),
            None
        );

        tray.watcher_online();
        assert!(!shared.registered.load(Ordering::Acquire));
        update_registration(&shared, &watcher_online, true, true);
        assert!(shared.registered.load(Ordering::Acquire));
        assert!(crate::tray::hold_for(
            shared.registered.load(Ordering::Acquire),
            crate::tray_settings::TraySettings::default()
        )
        .is_some());
    }

    #[test]
    fn absent_watcher_expires_startup_but_later_watcher_loss_remains_recoverable() {
        let started_at = Instant::now();
        let before_deadline = started_at + STARTUP_REGISTRATION_TIMEOUT - Duration::from_millis(1);
        let after_deadline = started_at + STARTUP_REGISTRATION_TIMEOUT;

        assert!(!startup_registration_expired(
            false,
            started_at,
            before_deadline
        ));
        assert!(startup_registration_expired(
            false,
            started_at,
            after_deadline
        ));
        assert!(!startup_registration_expired(
            true,
            started_at,
            after_deadline
        ));
    }

    #[test]
    fn dropping_the_sni_service_disconnects_the_state_actions() {
        let shared = Arc::new(Shared {
            snapshot: std::sync::Mutex::new(Snapshot::default()),
            wanted: AtomicBool::new(false),
            registered: AtomicBool::new(false),
            refreshed: std::sync::Mutex::new(Instant::now()),
        });
        let (actions, received) = std::sync::mpsc::channel();
        let tray = LinuxTray {
            shared,
            actions,
            watcher_online: Arc::new(AtomicBool::new(false)),
            sync_frame: 0,
        };

        drop(tray);
        assert!(matches!(
            received.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn disabled_status_hides_then_reenables_without_replacing_tray_item() {
        let shared = Arc::new(Shared {
            snapshot: std::sync::Mutex::new(Snapshot::default()),
            wanted: AtomicBool::new(false),
            registered: AtomicBool::new(true),
            refreshed: std::sync::Mutex::new(std::time::Instant::now()),
        });
        let (actions, _received) = std::sync::mpsc::channel();
        let tray = LinuxTray {
            shared: Arc::clone(&shared),
            actions,
            watcher_online: Arc::new(AtomicBool::new(true)),
            sync_frame: 0,
        };

        assert_eq!(tray.status(), ksni::Status::Passive);
        assert!(tray.icon_name().is_empty());
        assert!(tray.icon_pixmap().len() == 1);
        assert!(tray.tool_tip().icon_name.is_empty());

        shared.wanted.store(true, Ordering::Release);
        assert_eq!(tray.status(), ksni::Status::Active);

        *shared
            .refreshed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Instant::now() - model::STALE_AFTER - std::time::Duration::from_secs(1);
        assert_eq!(tray.status(), ksni::Status::NeedsAttention);

        shared.wanted.store(false, Ordering::Release);
        assert_eq!(tray.status(), ksni::Status::Passive);
        shared.wanted.store(true, Ordering::Release);
        assert_eq!(tray.status(), ksni::Status::NeedsAttention);
    }
}
