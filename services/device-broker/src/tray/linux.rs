//! Linux StatusNotifierItem adapter for the broker's shared tray model.
//!
//! `ksni::TrayMethods::spawn` returns only after registration with a live
//! StatusNotifierWatcher. Later watcher loss and recovery update the same
//! `Shared::registered` bit that controls the device idle hold.

use super::model::{self, MenuAction, MenuRow, Snapshot};
use super::state::{Notify, Shared};
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Sender, Arc, Mutex, OnceLock};
use std::time::Instant;

const ITEM_ID: &str = "qc-remote-device-broker";
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
    let watcher_online = Arc::new(AtomicBool::new(false));
    let tray = LinuxTray {
        shared: Arc::clone(&shared),
        actions,
        watcher_online: Arc::clone(&watcher_online),
        sync_frame: 0,
    };
    // Do not assume SNI support and do not publish an active marker if the
    // desktop has no StatusNotifierHost. The successful spawn includes the
    // Watcher's RegisterStatusNotifierItem acknowledgement.
    let handle = tray
        .spawn()
        .map_err(|error| format!("StatusNotifierItem registration failed: {error}"))?;
    watcher_online.store(true, Ordering::Release);
    shared.registered.store(true, Ordering::Release);
    *ACTIVE_HANDLE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle.clone());
    Ok(Box::new(move || {
        let updated = handle.update(LinuxTray::advance_frame).is_some();
        update_registration(&shared, &watcher_online, updated);
    }))
}

pub fn remove_icon_now() {
    let handle = ACTIVE_HANDLE.get().and_then(|active| {
        active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    });
    if let Some(handle) = handle {
        // Queue withdrawal, then let the broker's exit path continue without
        // waiting on the desktop's D-Bus loop. Process exit closes the SNI
        // connection even if the host is no longer answering.
        let _ = handle.shutdown();
    }
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
        for pixel in data.chunks_exact_mut(4) {
            pixel.rotate_right(1); // RGBA -> ARGB, as required by SNI.
        }
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

fn update_registration(shared: &Shared, watcher_online: &AtomicBool, update_succeeded: bool) {
    // ksni services this update on its D-Bus loop after attempting watcher
    // re-registration. Do not restore the device hold merely on its online
    // callback; restore it only after the live tray service handled an update.
    shared.registered.store(
        update_succeeded && watcher_online.load(Ordering::Acquire),
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

        update_registration(&shared, &watcher_online, true);
        assert!(!shared.registered.load(Ordering::Acquire));

        watcher_online.store(true, Ordering::Release);
        update_registration(&shared, &watcher_online, false);
        assert!(!shared.registered.load(Ordering::Acquire));

        update_registration(&shared, &watcher_online, true);
        assert!(shared.registered.load(Ordering::Acquire));

        watcher_online.store(false, Ordering::Release);
        update_registration(&shared, &watcher_online, true);
        assert!(!shared.registered.load(Ordering::Acquire));
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

        shared
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stale = true;
        assert_eq!(tray.status(), ksni::Status::NeedsAttention);

        shared.wanted.store(false, Ordering::Release);
        assert_eq!(tray.status(), ksni::Status::Passive);
        shared.wanted.store(true, Ordering::Release);
        assert_eq!(tray.status(), ksni::Status::NeedsAttention);
    }
}
