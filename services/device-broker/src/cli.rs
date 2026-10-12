//! The command line of `qc-device-broker.exe`. Two binaries call [`main`]:
//! the shipped one in apps/windows/src-tauri (`src/bin/qc-device-broker.rs`),
//! which passes the background MCP host as [`Binary::host`], and this crate's
//! own developer build, which passes none. Only the first ships.
//!
//! Modes, each chosen by its flag and otherwise unchanged from before the
//! resident host existed:
//! - `--broker-serve --descriptor-path <p>`: the shared broker. If the tray
//!   may draw here it also becomes the session's resident host while
//!   `trayEnabled` is on (see `crate::resident`).
//! - `--resident [--directory <dir>] [--keep-user-quit]`: the logon start.
//!   Joins the app's election, exits 0 if a broker already serves, never
//!   opens USB. Without `--keep-user-quit` it is a user's start and clears a
//!   recorded Quit first (`crate::user_quit`); with it, it never clears one
//!   and does not serve while one stands.
//! - `--stop-for-install`: the installer's helper; see `tray::install_stop`.
//! - `--control <status|release|connect|quit|start> [--directory <dir>]
//!   [--confirm]`: a short-lived client of the running broker for automation;
//!   see `crate::control`. Never a broker itself; only `start` spawns one,
//!   as `--resident`.
//! - `--self-check`: identifies the build; touches nothing.
//! - `--stdio`, `--verify-live`, `--probe`: never an icon, never a host,
//!   never the election.
//! - No arguments: usage on stderr, exit 1. Nothing starts and USB is never
//!   touched (it used to run a live probe, which a double-click reached).
use crate::resident::{self, Election, ResidentHost, Supervisor};
use crate::rpc::BrokerHandle;
use crate::tray;
use crate::worker::DeviceController;
use quadcortex::sync::RecoverPoison;
use quadcortex_hid as usb;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// What the calling binary contributes.
pub struct Binary {
    /// Reported by `--self-check`.
    pub version: &'static str,
    /// Where `--resident` meets the app when `--directory` is not given.
    pub default_directory: Option<fn() -> PathBuf>,
    /// The background MCP host, linked into the shipped binary only.
    pub host: Option<Arc<dyn ResidentHost>>,
    /// A string embedded in the shipped binary's image, read from the file by
    /// the release checks before they ever run it (tools/
    /// verify-packaged-broker.mjs). `--self-check` reports it too.
    pub build_marker: Option<&'static str>,
}

/// The host is stopped before exit, after clients were told; never longer.
const EXIT_HOST_STOP: Duration = Duration::from_secs(2);
/// An installer's request stops the host first, bounded.
#[cfg(any(windows, test))]
const INSTALL_HOST_STOP: Duration = Duration::from_secs(3);
/// After the host stopped, its connection may still be closing.
#[cfg(any(windows, test))]
const INSTALL_RELEASE_WAIT: Duration = Duration::from_secs(1);
/// Mirrors `DEVICE_BROKER_ELECTION_TIMEOUT` in the app.
const RESIDENT_ELECTION_BUDGET: Duration = Duration::from_secs(10);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProbeResult {
    present: bool,
    connected: bool,
    synchronized: bool,
    handshake_ms: Option<u128>,
    observed_message_counts: BTreeMap<u16, usize>,
    active_preset_name: Option<String>,
    detail: String,
}

fn probe() -> ProbeResult {
    let started = Instant::now();
    let session = Arc::new(std::sync::Mutex::new(
        quadcortex::device::owner::SessionRuntime::new(0),
    ));
    match usb::QcUsb::connect(
        Arc::clone(&session),
        &started,
        |_, _| {},
        |message| {
            if message.message_type == quadcortex::profile::MESSAGE_TYPE_MODEL_REPO {
                let catalog = quadcortex::state::parse_model_repo(&message.payload)
                    .map_err(|error| error.to_string());
                if let Some(ticket) = message.observation {
                    let _ = session.lock_recover().install_model_catalog(
                        &ticket,
                        catalog,
                        started.elapsed().as_millis() as u64,
                    );
                }
            }
        },
    ) {
        Ok(connection) => {
            let synchronized = session.lock_recover().transport.synchronized();
            ProbeResult {
                present: true,
                connected: true,
                synchronized,
                handshake_ms: Some(started.elapsed().as_millis()),
                observed_message_counts: connection.message_counts.into_iter().collect(),
                active_preset_name: connection
                    .latest_messages
                    .get(&quadcortex::profile::MESSAGE_TYPE_RECALL_PRESET)
                    .and_then(|message| usb::preset_name(&message.payload)),
                detail: if synchronized {
                    "Active preset received"
                } else {
                    "Handshake complete; active preset still pending"
                }
                .into(),
            }
        }
        Err(usb::UsbError::NotAvailable) => ProbeResult {
            present: false,
            connected: false,
            synchronized: false,
            handshake_ms: None,
            observed_message_counts: BTreeMap::new(),
            active_preset_name: None,
            detail: "Quad Cortex is not currently enumerable".into(),
        },
        Err(error) => ProbeResult {
            present: true,
            connected: false,
            synchronized: false,
            handshake_ms: Some(started.elapsed().as_millis()),
            observed_message_counts: BTreeMap::new(),
            active_preset_name: None,
            detail: error.to_string(),
        },
    }
}

/// Which mode the arguments select. Exact-match modes (self-check, the
/// installer helper) refuse to combine with anything, so a typo can never
/// fall through to the live probe from an installer.
#[derive(Debug, PartialEq, Eq)]
enum Mode {
    SelfCheck,
    StopForInstall,
    Control(crate::control::Invocation),
    /// `--control` with arguments that do not parse: a JSON report, exit 2.
    ControlUsage(String),
    Resident {
        directory: Option<PathBuf>,
        /// `--keep-user-quit`: a start that is not itself a user action (the
        /// app's startup reconcile, `--control start`, which cleared the record
        /// itself). It never clears a recorded Quit and does not serve while one
        /// stands. Without it (the sign-in Run value, the Settings switch) the
        /// entry is the start's one clear point.
        keep_user_quit: bool,
    },
    BrokerServe,
    Stdio,
    VerifyLive,
    Probe,
    Usage(String),
}

fn mode(args: &[String]) -> Mode {
    let has = |flag: &str| args.iter().any(|argument| argument == flag);
    match args {
        [flag] if flag == "--self-check" => return Mode::SelfCheck,
        [flag] if flag == "--stop-for-install" => return Mode::StopForInstall,
        _ => {}
    }
    if has("--self-check") || has("--stop-for-install") {
        return Mode::Usage("--self-check and --stop-for-install take no other arguments".into());
    }
    // A control client takes no other mode: anything it does not parse is
    // refused, never passed on to a broker or a probe.
    if has("--control") {
        return match crate::control::parse(args) {
            Ok(invocation) => Mode::Control(invocation),
            Err(message) => Mode::ControlUsage(message),
        };
    }
    if has("--resident") {
        // `--keep-user-quit` may only come last, once.
        let (args, keep_user_quit) = match args {
            [rest @ .., last] if last == KEEP_USER_QUIT_FLAG => (rest, true),
            _ => (args, false),
        };
        return match args {
            [flag] if flag == "--resident" => Mode::Resident {
                directory: None,
                keep_user_quit,
            },
            [flag, option, path]
                if flag == "--resident" && option == "--directory" && !path.is_empty() =>
            {
                Mode::Resident {
                    directory: Some(PathBuf::from(path)),
                    keep_user_quit,
                }
            }
            _ => Mode::Usage(
                "Usage: qc-device-broker --resident [--directory <app-data-dir>] [--keep-user-quit]"
                    .into(),
            ),
        };
    }
    if has("--stdio") && has("--broker-serve") {
        return Mode::Usage("--stdio and --broker-serve are mutually exclusive".into());
    }
    // Every remaining mode is checked argument by argument against what it
    // takes. An unknown or misspelled flag (`--broker-serv`, `--resdient`, or
    // one a newer caller passes to an older binary) used to fall through to
    // the default probe and open the Quad Cortex; it is a usage error now and
    // touches nothing. So is a bare invocation.
    let (selected, flags, valued): (Mode, &[&str], &[&str]) = if has("--broker-serve") {
        (
            Mode::BrokerServe,
            &[
                "--broker-serve",
                "--auto-connect",
                "--phase-events",
                tray::NO_TRAY_FLAG,
            ],
            &["--descriptor-path"],
        )
    } else if has("--stdio") {
        (
            Mode::Stdio,
            &["--stdio", "--auto-connect", "--phase-events"],
            &[],
        )
    } else if has("--verify-live") {
        (Mode::VerifyLive, &["--verify-live"], &[])
    } else if has("--probe") {
        (Mode::Probe, &["--probe"], &[])
    } else if args.is_empty() {
        // A bare invocation (a double-clicked exe, or a caller that lost its
        // arguments) used to start a live device probe. It is a usage error
        // now and touches nothing; the probe is `--probe`.
        return Mode::Usage(format!("No arguments given; nothing was started. {USAGE}"));
    } else {
        return Mode::Usage(format!(
            "Unknown argument {:?}; nothing was started. {USAGE}",
            args[0]
        ));
    };
    match unexpected_argument(args, flags, valued) {
        None => selected,
        Some(problem) => Mode::Usage(format!("{problem}; nothing was started. {USAGE}")),
    }
}

const USAGE: &str = "Usage: qc-device-broker --broker-serve --descriptor-path <path> \
    [--auto-connect] [--phase-events] [--no-tray] | --stdio [--auto-connect] [--phase-events] \
    | --resident [--directory <dir>] [--keep-user-quit] \
    | --control <command> [--directory <dir>] [--confirm] \
    | --self-check | --stop-for-install | --verify-live | --probe";

/// The first argument that is neither one of `flags` nor one of `valued`
/// followed by its value, described for a usage message.
fn unexpected_argument(args: &[String], flags: &[&str], valued: &[&str]) -> Option<String> {
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        if valued.contains(&argument.as_str()) {
            if arguments.next().is_none_or(|value| value.is_empty()) {
                return Some(format!("{argument} requires a value"));
            }
        } else if !flags.contains(&argument.as_str()) {
            return Some(format!("Unknown argument {argument:?}"));
        }
    }
    None
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}

pub fn main(binary: Binary) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Reported by `--control status` as the running broker's version.
    crate::rpc::set_binary_version(binary.version);
    match mode(&args) {
        Mode::Usage(message) => fail(message),
        Mode::SelfCheck => println!(
            "{}",
            serde_json::json!({
                "binary": "qc-device-broker",
                "version": binary.version,
                "selfCheck": true,
                "residentHost": binary.host.is_some(),
                "buildMarker": binary.build_marker,
                "controlSchema": crate::control::SCHEMA,
            })
        ),
        Mode::StopForInstall => stop_for_install(),
        Mode::Control(_) | Mode::ControlUsage(_) => {
            crate::control::main(&args, binary.version, binary.default_directory)
        }
        Mode::Resident {
            directory,
            keep_user_quit,
        } => {
            let directory = directory
                .or_else(|| binary.default_directory.map(|default| default()))
                .unwrap_or_else(|| fail("--resident requires --directory <app-data-dir>"));
            run_resident(&args, directory, keep_user_quit, binary.host)
        }
        Mode::BrokerServe => {
            let descriptor_path = args
                .windows(2)
                .find(|pair| pair[0] == "--descriptor-path")
                .map(|pair| PathBuf::from(&pair[1]))
                .unwrap_or_else(|| fail("--broker-serve requires --descriptor-path <path>"));
            #[cfg(target_os = "linux")]
            let election = {
                let directory = descriptor_path.parent().unwrap_or_else(|| {
                    fail("The broker descriptor needs an absolute parent directory")
                });
                crate::platform::paths::prepare_directory(directory)
                    .unwrap_or_else(|error| fail(error));
                match resident::elect(&descriptor_path, RESIDENT_ELECTION_BUDGET)
                    .unwrap_or_else(|error| fail(error))
                {
                    Election::AlreadyServed => return,
                    Election::Serve(guard) => Some(guard),
                }
            };
            #[cfg(not(target_os = "linux"))]
            let election = None;
            serve(
                &args,
                descriptor_path,
                args.iter().any(|argument| argument == "--auto-connect"),
                binary.host,
                election,
                false,
            )
        }
        Mode::Stdio => {
            #[cfg(target_os = "linux")]
            let signals =
                crate::platform::shutdown::Pending::new().unwrap_or_else(|error| fail(error));
            // A first broker starts idle so it cannot race its client's explicit
            // reconnect. A replacement spawned after a transport failure is the
            // opposite case: its client already had a session and has no step that
            // asks for another one, so without this it answers every device call
            // with "not connected" for the life of the process.
            let auto_connect = args.iter().any(|argument| argument == "--auto-connect");
            // Opt-in, so every other client of this stream keeps receiving only
            // the frames it already knows how to handle.
            let phase_events = args.iter().any(|argument| argument == "--phase-events");
            let controller = if auto_connect {
                DeviceController::start()
            } else {
                DeviceController::start_disconnected()
            }
            .unwrap_or_else(|error| fail(error));
            // No descriptor here, so the settings directory is the app's own.
            configure_updater_relay_directory(
                &controller,
                binary.default_directory.map(|default| default()),
            );
            #[cfg(target_os = "linux")]
            let _signal_watch = {
                let shutdown = controller.shutdown_handle();
                signals
                    .watch(move || {
                        let _ = shutdown.stop_and_join(crate::worker::WORKER_EXIT_BUDGET);
                        std::process::exit(0);
                    })
                    .unwrap_or_else(|error| {
                        let _ = controller
                            .shutdown_handle()
                            .stop_and_join(crate::worker::WORKER_EXIT_BUDGET);
                        fail(error)
                    })
            };
            if let Err(error) = crate::rpc::serve_stdio(controller, phase_events) {
                fail(error);
            }
        }
        Mode::VerifyLive => verify_live(binary.default_directory.map(|default| default())),
        Mode::Probe => println!(
            "{}",
            serde_json::to_string(&probe()).expect("probe result serializes")
        ),
    }
}

/// Signal this installation's broker (this very path) and wait for it. Always
/// exits 0: whether the broker stepped aside is the installer's unlock check
/// to judge, and the message it then shows names the reason.
fn stop_for_install() {
    #[cfg(windows)]
    {
        match std::env::current_exe() {
            Ok(executable) => match tray::install_stop::request_stop(
                &executable,
                Duration::from_secs(5),
                Duration::from_millis(500),
            ) {
                Ok(true) | Ok(false) => {}
                Err(error) => eprintln!("Could not ask the QC Remote broker to stop: {error}"),
            },
            Err(error) => eprintln!("Could not locate this executable: {error}"),
        }
    }
    #[cfg(not(windows))]
    eprintln!("--stop-for-install is supported on Windows");
}

fn tray_allowed(args: &[String]) -> Result<(), tray::Suppressed> {
    tray::allowed(
        args.iter().any(|argument| argument == tray::NO_TRAY_FLAG),
        std::env::var_os(tray::NO_TRAY_ENV),
        tray::interactive_session(),
    )
}

/// The last argument of a `--resident` start that is not itself a user
/// action; see [`Mode::Resident`].
pub(crate) const KEEP_USER_QUIT_FLAG: &str = "--keep-user-quit";

/// What a `--resident` entry does about a recorded user Quit
/// (`crate::user_quit`), before its election.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResidentEntry {
    /// Nothing stands, or this entry cleared it: go on to the election.
    Serve,
    /// `--keep-user-quit` and a recorded Quit stands: exit 0, never serve.
    Decline,
    /// The record could not be cleared: stop, exit 1, never serve.
    Fail(String),
}

/// The resident entry's one decision about a recorded Quit. Without
/// `keep_user_quit` the entry is a user's (the sign-in Run value, the Settings
/// switch turned on) and this is that start's one clear point, under the
/// election lock. With it (the app's startup reconcile, or `--control start`,
/// which cleared at its own one point before spawning this), a Quit recorded
/// since then stands: it is never cleared here and the start does not serve.
pub(crate) fn resident_entry(
    descriptor_path: &std::path::Path,
    keep_user_quit: bool,
) -> ResidentEntry {
    if keep_user_quit {
        return match crate::user_quit::read(descriptor_path) {
            None => ResidentEntry::Serve,
            Some(_) => ResidentEntry::Decline,
        };
    }
    match crate::user_quit::clear(descriptor_path) {
        Ok(_) => ResidentEntry::Serve,
        Err(error) => ResidentEntry::Fail(error),
    }
}

/// The logon start. Without the tray switch, a desktop, or the election, it
/// is not wanted: it exits 0 at once and never claims anything.
fn run_resident(
    args: &[String],
    directory: PathBuf,
    keep_user_quit: bool,
    host: Option<Arc<dyn ResidentHost>>,
) {
    if !crate::tray_settings::read_in(&directory).tray_enabled {
        return;
    }
    if let Err(reason) = tray_allowed(args) {
        eprintln!("No resident host here: {reason:?}");
        return;
    }
    let descriptor_path = directory.join(resident::DESCRIPTOR_FILE);
    match resident_entry(&descriptor_path, keep_user_quit) {
        ResidentEntry::Serve => {}
        ResidentEntry::Decline => {
            eprintln!("No resident host: QC Remote was quit and no user action has cleared it");
            return;
        }
        // Stops: serving under a Quit nobody cleared would be the autonomous
        // restart the record exists to prevent.
        ResidentEntry::Fail(error) => fail(format!("No resident host: {error}")),
    }
    match resident::elect(&descriptor_path, RESIDENT_ELECTION_BUDGET) {
        Ok(Election::AlreadyServed) => {}
        Ok(Election::Serve(guard)) => {
            // Admission, under the election lock this start now holds: a Quit
            // recorded since the entry (it needs this lock to be written)
            // refuses, on both paths, and is never cleared here.
            if !resident_admitted(&descriptor_path) {
                eprintln!("No resident host: QC Remote was quit while this start waited");
                drop(guard);
                return;
            }
            serve(args, descriptor_path, false, host, Some(guard), true)
        }
        Err(error) => fail(error),
    }
}

/// The resident's admission, run holding its `ElectionGuard` immediately
/// before it serves: `false` while any user Quit is recorded. Nothing is
/// cleared here; the entry ([`resident_entry`]) was the start's one clear
/// point, and a record standing now is a later Quit (or one a
/// `--keep-user-quit` entry already declined).
pub(crate) fn resident_admitted(descriptor_path: &std::path::Path) -> bool {
    crate::user_quit::read(descriptor_path).is_none()
}

fn serve(
    args: &[String],
    descriptor_path: PathBuf,
    auto_connect: bool,
    host: Option<Arc<dyn ResidentHost>>,
    election: Option<resident::ElectionGuard>,
    started_for_residence: bool,
) {
    let phase_events = phase_events_enabled(args, started_for_residence);
    #[cfg(target_os = "linux")]
    let signals = match crate::platform::shutdown::Pending::new() {
        Ok(signals) => signals,
        Err(error) => {
            drop(election);
            fail(error)
        }
    };
    #[cfg(target_os = "linux")]
    let mut signal_watch = None;
    // Always start disconnected here, regardless of `--auto-connect`: the
    // real USB handshake can take upwards of ten seconds against real
    // hardware, and this branch's job is to bind the TCP listener, claim the
    // descriptor slot, and publish before anything waits on that.
    // `serve_broker_tcp` defers the actual auto-connect attempt (via
    // `DeviceController::reconnect`) until after it has published, so a slow
    // or absent device can never delay publication past
    // `DEVICE_BROKER_PUBLISH_TIMEOUT` in the Windows host. A `--resident`
    // start never auto-connects at all: it has no client.
    let controller = match DeviceController::start_disconnected() {
        Ok(controller) => controller,
        Err(error) => {
            drop(election);
            fail(error)
        }
    };
    // The icon belongs to this process: it appears once this broker has won
    // the descriptor election and goes when it exits. Only an interactive
    // desktop gets one; `--stdio` and every probe never do. Without an icon
    // thread there is no residence, so no host either.
    let tray_allowed = tray_allowed(args);
    let supervisor: Arc<OnceLock<Arc<Supervisor>>> = Arc::new(OnceLock::new());
    #[cfg(target_os = "linux")]
    let exit_status = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let on_exit = {
        let supervisor = Arc::clone(&supervisor);
        #[cfg(target_os = "linux")]
        let exit_status = Arc::clone(&exit_status);
        Arc::new(move || {
            // Clients were already sent `broker.shuttingDown` and the device
            // was released; stop the host, bounded, then go.
            if let Some(supervisor) = supervisor.get() {
                supervisor.stop_for_exit(EXIT_HOST_STOP);
            }
            tray::remove_icon_now();
            #[cfg(target_os = "linux")]
            std::process::exit(exit_status.load(std::sync::atomic::Ordering::Acquire));
            #[cfg(not(target_os = "linux"))]
            std::process::exit(0)
        })
    };
    let published = {
        let supervisor = Arc::clone(&supervisor);
        #[cfg(target_os = "linux")]
        let signal_watch_slot = &mut signal_watch;
        move |handle: BrokerHandle| {
            // Published: a peer's election now attaches to this broker.
            drop(election);
            note_publication(&handle, started_for_residence);
            #[cfg(target_os = "linux")]
            {
                let shutdown = handle.clone();
                *signal_watch_slot = Some(
                    signals
                        .watch(move || shutdown.shutdown_from_os())
                        .unwrap_or_else(|error| {
                            eprintln!("{error}");
                            exit_status.store(1, std::sync::atomic::Ordering::Release);
                            handle.shutdown_from_os();
                            fail(error)
                        }),
                );
            }
            #[cfg(windows)]
            {
                let installer = handle.clone();
                let supervisor = Arc::clone(&supervisor);
                if let Err(error) = tray::install_stop::watch(move || {
                    install_release(&installer, supervisor.get().map(Arc::as_ref))
                }) {
                    eprintln!("The installer stop request is unavailable: {error}");
                }
            }
            match tray_allowed {
                Ok(()) => {
                    let tray_host = host.clone();
                    if let Some(host) = host {
                        match Supervisor::start(handle.clone(), host) {
                            Ok(started) => {
                                let _ = supervisor.set(started);
                            }
                            Err(error) => {
                                eprintln!(
                                    "The background MCP host supervisor could not start: {error}"
                                )
                            }
                        }
                    }
                    tray::start(handle, tray_host)
                }
                Err(reason) => eprintln!("Broker icon not shown: {reason:?}"),
            }
        }
    };
    if let Err(error) = crate::rpc::serve_broker_tcp_with_published_hook(
        Arc::new(controller),
        phase_events,
        descriptor_path,
        on_exit,
        auto_connect,
        published,
    ) {
        fail(error);
    }
}

/// Whether this TCP broker pushes `device.phase` to its session clients.
///
/// Opt-in for a spawned broker, so every other client of its stream keeps
/// receiving only the frames it already knows how to handle. A `--resident`
/// start has no caller to opt in for its clients (the tray's start line takes
/// no such flag), yet a window attached to it learns that the device it opened
/// became ready only from this stream: without it the window stayed on the
/// demo until someone clicked Reconnect. The host's gateway reader, which every
/// client in this tree attaches through, routes or drops a `device.phase` it
/// does not want, and a control connection never gets one.
fn phase_events_enabled(args: &[String], started_for_residence: bool) -> bool {
    started_for_residence || args.iter().any(|argument| argument == "--phase-events")
}

/// What a published broker records about how it was started. A `--resident`
/// start has nobody to ask it to connect, so the first participant's device
/// request opens the device instead.
fn note_publication(handle: &BrokerHandle, started_for_residence: bool) {
    if started_for_residence {
        handle.mark_started_disconnected();
    }
}

/// An installer asked this installation to step aside. The in-process host is
/// stopped first, so its own participant connection can never be what keeps
/// the broker; any other participant still refuses, exactly as before. Every
/// wait is bounded, so a wedged host costs at most `INSTALL_HOST_STOP` and the
/// installer then reports the broker in use rather than hanging.
#[cfg(any(windows, test))]
fn install_release(handle: &BrokerHandle, supervisor: Option<&Supervisor>) {
    install_release_with(
        || handle.participants_other_than_host(),
        || handle.release_for_install(),
        supervisor.map(|supervisor| supervisor as &dyn HostPause),
    );
}

/// What an installer stop needs of the host supervisor.
#[cfg(any(windows, test))]
trait HostPause {
    fn pause(&self, within: Duration) -> bool;
    fn resume(&self);
}

#[cfg(any(windows, test))]
impl HostPause for Supervisor {
    fn pause(&self, within: Duration) -> bool {
        Supervisor::pause(self, within)
    }
    fn resume(&self) {
        Supervisor::resume(self)
    }
}

/// A window or a bridge attached refuses the stop before the host is touched,
/// so a refused install (the helper repeats every 500 ms for 5 s) never drops
/// the host's AI sessions. Only with nobody but the host attached is the host
/// paused, and then the release is retried while its connection closes.
#[cfg(any(windows, test))]
fn install_release_with(
    others_attached: impl Fn() -> usize,
    release: impl Fn() -> bool,
    host: Option<&dyn HostPause>,
) -> bool {
    if others_attached() != 0 {
        return false;
    }
    if let Some(host) = host {
        host.pause(INSTALL_HOST_STOP);
    }
    let deadline = Instant::now() + INSTALL_RELEASE_WAIT;
    loop {
        if release() {
            return true;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Refused: someone attached meanwhile, or a reopen is running. Keep
    // serving, host included.
    if let Some(host) = host {
        host.resume();
    }
    false
}

/// Point the update relay of a broker that has no descriptor (`--stdio`,
/// `--verify-live`) at the directory of the app's broker settings, the one
/// every descriptor-backed broker reads, so the Settings switch "Internet for
/// the Quad Cortex" and `updaterRelayExtraDomains` apply in every mode that
/// can carry the unit's requests. Returns whether a directory was known. A
/// build that names none (the developer build) keeps the relay's defaults:
/// on, `neuraldsp.com` only, and no way to change them.
fn configure_updater_relay_directory(
    controller: &DeviceController,
    directory: Option<PathBuf>,
) -> bool {
    match directory {
        Some(directory) => {
            controller.configure_updater_relay(directory);
            true
        }
        None => {
            eprintln!(
                "No settings directory is known to this build; the update relay runs on its \
                 defaults and the Settings switch cannot reach it"
            );
            false
        }
    }
}

/// `process::exit` runs no destructors, so every early exit drops the
/// controller first: its Drop waits for the USB worker's device teardown.
fn verify_live(settings_directory: Option<PathBuf>) {
    let controller = DeviceController::start().unwrap_or_else(|error| fail(error));
    configure_updater_relay_directory(&controller, settings_directory);
    let initial = controller.wait_for_ready(Duration::from_millis(
        quadcortex::profile::READY_WAIT_TIMEOUT_MS,
    ));
    let Some(original) = initial.active_scene else {
        println!(
            "{}",
            serde_json::json!({"verified": false, "detail": "QC did not report an active scene", "initial": initial})
        );
        drop(controller);
        std::process::exit(1);
    };
    let target = (original + 1) % 8;
    if let Err(error) = controller.switch_scene(target) {
        println!(
            "{}",
            serde_json::json!({"verified": false, "detail": error, "initial": initial})
        );
        drop(controller);
        std::process::exit(1);
    }
    let changed = controller.wait_for_scene(target, Duration::from_secs(3));
    let changed_ok = changed.active_scene == Some(target);
    let restored_ok = if controller.switch_scene(original).is_ok() {
        controller
            .wait_for_scene(original, Duration::from_secs(3))
            .active_scene
            == Some(original)
    } else {
        false
    };
    println!(
        "{}",
        serde_json::json!({
            "verified": changed_ok && restored_ok,
            "preset": initial.active_preset_name,
            "originalScene": original,
            "targetScene": target,
            "deviceEchoedTarget": changed_ok,
            "restoredOriginal": restored_ok,
            "handshakeMs": initial.handshake_ms
        })
    );
    if !(changed_ok && restored_ok) {
        drop(controller);
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Mode {
        mode(
            &args
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn installer_and_self_check_modes_are_exact_and_never_fall_through_to_a_probe() {
        assert_eq!(parse(&["--self-check"]), Mode::SelfCheck);
        assert_eq!(parse(&["--stop-for-install"]), Mode::StopForInstall);
        for args in [
            vec!["--stop-for-install", "--self-check"],
            vec!["--stop-for-install", "--broker-serve"],
            vec!["--self-check", "--stdio"],
        ] {
            assert!(matches!(parse(&args), Mode::Usage(_)), "{args:?}");
        }
    }

    #[test]
    fn resident_takes_only_its_directory() {
        assert_eq!(
            parse(&["--resident"]),
            Mode::Resident {
                directory: None,
                keep_user_quit: false
            }
        );
        assert_eq!(
            parse(&["--resident", "--directory", "C:\\data"]),
            Mode::Resident {
                directory: Some("C:\\data".into()),
                keep_user_quit: false
            }
        );
        assert_eq!(
            parse(&["--resident", "--directory", "C:\\data", "--keep-user-quit"]),
            Mode::Resident {
                directory: Some("C:\\data".into()),
                keep_user_quit: true
            }
        );
        assert_eq!(
            parse(&["--resident", "--keep-user-quit"]),
            Mode::Resident {
                directory: None,
                keep_user_quit: true
            }
        );
        for args in [
            vec!["--resident", "--directory"],
            vec!["--resident", "--directory", ""],
            vec!["--resident", "--broker-serve"],
            vec!["--resident", "--stdio"],
            vec!["--broker-serve", "--resident"],
            vec!["--resident", "--keep-user-quit", "--directory", "C:\\data"],
            vec![
                "--resident",
                "--directory",
                "C:\\data",
                "--keep-user-quit",
                "--keep-user-quit",
            ],
        ] {
            assert!(matches!(parse(&args), Mode::Usage(_)), "{args:?}");
        }
    }

    #[test]
    fn control_takes_no_other_mode_and_refuses_what_it_does_not_parse() {
        use crate::control::{Command, Invocation};
        assert_eq!(
            parse(&["--control", "status"]),
            Mode::Control(Invocation {
                command: Command::Status,
                directory: None,
                confirm: false
            })
        );
        assert_eq!(
            parse(&["--control", "quit", "--confirm", "--directory", "C:\\data"]),
            Mode::Control(Invocation {
                command: Command::Quit,
                directory: Some("C:\\data".into()),
                confirm: true
            })
        );
        for args in [
            vec!["--control"],
            vec!["--control", "kill"],
            vec!["--control", "status", "--confirm"],
            vec!["--control", "release", "--confirm"],
            vec!["--control", "quit", "--confirm", "--confirm"],
            vec!["--control", "status", "--directory"],
            vec!["--control", "status", "--directory", ""],
            vec![
                "--control",
                "status",
                "--directory",
                "a",
                "--directory",
                "b",
            ],
            vec!["--control", "status", "--token", "x"],
            vec!["--control", "status", "--broker-serve"],
            vec!["--control", "start", "--resident"],
            vec!["--broker-serve", "--control", "status"],
            vec!["--resident", "--control", "start"],
            vec!["--stdio", "--control", "status"],
            vec!["--probe", "--control", "status"],
        ] {
            assert!(matches!(parse(&args), Mode::ControlUsage(_)), "{args:?}");
        }
        // The exact-match modes still refuse first.
        assert!(matches!(
            parse(&["--control", "status", "--self-check"]),
            Mode::Usage(_)
        ));
        assert!(matches!(
            parse(&["--stop-for-install", "--control", "quit"]),
            Mode::Usage(_)
        ));
    }

    #[test]
    fn the_existing_modes_keep_their_flags() {
        assert_eq!(
            parse(&["--broker-serve", "--descriptor-path", "x", "--auto-connect"]),
            Mode::BrokerServe
        );
        assert_eq!(parse(&["--stdio", "--phase-events"]), Mode::Stdio);
        assert_eq!(parse(&["--verify-live"]), Mode::VerifyLive);
        assert_eq!(parse(&["--probe"]), Mode::Probe);
        assert!(matches!(
            parse(&["--stdio", "--broker-serve"]),
            Mode::Usage(_)
        ));
    }

    /// An unknown or misspelled flag used to fall through to
    /// the default probe, which opens the Quad Cortex. It is a usage error
    /// (exit 1 from `main`) and selects no mode that touches a device.
    #[test]
    fn an_unknown_or_misspelled_flag_is_a_usage_error_never_a_probe() {
        for args in [
            vec!["--broker-serv"],
            vec!["--resdient"],
            vec!["--stdo"],
            vec!["--prob"],
            vec!["--verify-lve"],
            vec!["--self-chek"],
            vec!["--auto-connect"],
            vec!["--phase-events"],
            vec!["--descriptor-path", "x"],
            vec!["--no-tray"],
            vec!["probe"],
            vec![""],
            vec!["--broker-serve", "--descriptor-path", "x", "--auto-conect"],
            vec!["--broker-serve", "--descriptor-path"],
            vec!["--broker-serve", "--descriptor-path", ""],
            vec!["--stdio", "--phase-event"],
            vec!["--stdio", "--descriptor-path", "x"],
            vec!["--stdio", "--no-tray"],
            vec!["--probe", "--auto-connect"],
            vec!["--verify-live", "--stdio-ish"],
            vec!["--a-flag-from-a-newer-caller", "--stdio"],
        ] {
            let selected = parse(&args);
            assert!(
                matches!(&selected, Mode::Usage(message) if message.contains("nothing was started")),
                "{args:?} selected {selected:?}"
            );
        }
    }

    /// A bare invocation (a user double-clicking the shipped exe) used to
    /// start a live device probe that opened the Quad Cortex. It prints usage
    /// to stderr and exits 1 from `main`, the same as any other usage error,
    /// and selects no mode that touches a device. The probe is `--probe`.
    #[test]
    fn a_bare_invocation_prints_usage_and_never_probes() {
        match parse(&[]) {
            Mode::Usage(message) => {
                assert!(message.contains("nothing was started"), "{message}");
                assert!(message.contains(USAGE), "{message}");
                assert!(message.contains("--probe"), "{message}");
            }
            selected => panic!("a bare invocation selected {selected:?}"),
        }
        assert_eq!(parse(&["--probe"]), Mode::Probe);
    }

    #[test]
    fn every_flag_the_shipped_callers_pass_is_still_accepted() {
        for args in [
            vec![
                "--broker-serve",
                "--descriptor-path",
                "C:\\d\\device-broker.json",
            ],
            vec![
                "--broker-serve",
                "--descriptor-path",
                "C:\\d\\device-broker.json",
                "--phase-events",
                "--auto-connect",
            ],
            vec!["--broker-serve", "--no-tray", "--descriptor-path", "x"],
        ] {
            assert_eq!(parse(&args), Mode::BrokerServe, "{args:?}");
        }
        for args in [
            vec!["--stdio"],
            vec!["--stdio", "--phase-events"],
            vec!["--stdio", "--phase-events", "--auto-connect"],
            vec!["--auto-connect", "--stdio"],
        ] {
            assert_eq!(parse(&args), Mode::Stdio, "{args:?}");
        }
    }

    struct CountingHost {
        paused: std::sync::atomic::AtomicUsize,
        resumed: std::sync::atomic::AtomicUsize,
    }
    impl HostPause for CountingHost {
        fn pause(&self, _within: Duration) -> bool {
            self.paused
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        }
        fn resume(&self) {
            self.resumed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn a_refused_install_never_touches_the_host_and_an_honoured_one_pauses_it_first() {
        use std::sync::atomic::Ordering::SeqCst;
        let host = CountingHost {
            paused: 0.into(),
            resumed: 0.into(),
        };
        // A window is attached: refused at once, the host keeps its sessions,
        // however often the installer repeats the request.
        for _ in 0..10 {
            assert!(!install_release_with(
                || 1,
                || panic!("never asked"),
                Some(&host)
            ));
        }
        assert_eq!(
            (host.paused.load(SeqCst), host.resumed.load(SeqCst)),
            (0, 0)
        );
        // Only the host: paused, then released once its connection is gone.
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        assert!(install_release_with(
            || 0,
            || attempts.fetch_add(1, SeqCst) >= 2,
            Some(&host)
        ));
        assert_eq!(
            (host.paused.load(SeqCst), host.resumed.load(SeqCst)),
            (1, 0)
        );
        // Someone arrived meanwhile: bounded, and the host is resumed.
        let started = Instant::now();
        assert!(!install_release_with(|| 0, || false, Some(&host)));
        assert!(started.elapsed() < INSTALL_RELEASE_WAIT + Duration::from_millis(500));
        assert_eq!(
            (host.paused.load(SeqCst), host.resumed.load(SeqCst)),
            (2, 1)
        );
    }

    #[test]
    fn only_a_resident_start_is_recorded_as_started_disconnected() {
        let resident = crate::rpc::test_support::handle();
        note_publication(&resident, true);
        assert!(crate::rpc::test_support::idle_released(&resident));
        let spawned = crate::rpc::test_support::handle();
        note_publication(&spawned, false);
        assert!(!crate::rpc::test_support::idle_released(&spawned));
    }

    #[test]
    fn a_resident_start_publishes_device_phases_without_the_flag() {
        fn owned(args: &[&str]) -> Vec<String> {
            args.iter().map(|value| value.to_string()).collect()
        }
        // The tray's own start line, which `mode` refuses to extend with the
        // flag: the window attached to it must still see the device go ready.
        let resident = owned(&["--resident", "--directory", "C:\\data"]);
        assert!(!matches!(mode(&resident), Mode::Usage(_)));
        assert!(phase_events_enabled(&resident, true));
        // A spawned broker is unchanged: on only when its caller asks.
        let opted_in = owned(&["--broker-serve", "--phase-events"]);
        assert!(phase_events_enabled(&opted_in, false));
        let spawned = owned(&["--broker-serve", "--descriptor-path", "x"]);
        assert!(!phase_events_enabled(&spawned, false));
    }

    #[test]
    fn the_installer_glue_counts_every_client_but_the_host() {
        use crate::rpc::test_support;
        // Only the in-process host attached, and no supervisor to stop it: the
        // glue gets past its own check (the host is not "another" client), but
        // release_for_install still counts every participant, the host
        // included, so it is refused within its bound. There is no installer
        // bypass.
        let handle = test_support::handle();
        let host = test_support::attach(&handle, true);
        let started = Instant::now();
        assert!(!install_release_with(
            || handle.participants_other_than_host(),
            || handle.release_for_install(),
            None
        ));
        assert!(started.elapsed() < INSTALL_RELEASE_WAIT + Duration::from_millis(500));
        // The host leaves (as its supervisor's pause makes it): honoured.
        test_support::detach(&handle, host);
        install_release(&handle, None);
        assert!(handle.shutting_down());
        // A window attached: refused at once, nothing committed.
        let busy = test_support::handle();
        let _window = test_support::attach(&busy, false);
        install_release(&busy, None);
        assert!(!busy.shutting_down());
        test_support::finish(&busy);
    }

    #[test]
    fn a_broker_without_a_descriptor_reads_the_relay_switch_from_the_app_directory() {
        // `--stdio` and `--verify-live` have no descriptor path, so nothing
        // used to tell their update relay where the Settings switch lives and
        // it ran on its defaults, on, whatever the user had chosen.
        let directory =
            std::env::temp_dir().join(format!("qc-cli-relay-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        // Removed even when an assertion fails; the controller reading the
        // switch from it is declared later, so it drops first.
        let _directory = crate::device_lease::testing::OwnedTestDirectory(directory.clone());
        let controller = DeviceController::start_disconnected().unwrap();
        assert!(
            controller.status().updater_relay.enabled,
            "the default is on"
        );

        // A build that names no directory changes nothing.
        assert!(!configure_updater_relay_directory(&controller, None));
        assert!(controller.status().updater_relay.enabled);

        // The user clears the switch: the stdio broker follows it.
        crate::tray_settings::set_updater_relay_enabled_in(&directory, false).unwrap();
        assert!(configure_updater_relay_directory(
            &controller,
            Some(directory.clone())
        ));
        assert!(!controller.status().updater_relay.enabled);

        // A settings file without the key, or no file at all, means on.
        std::fs::write(
            directory.join(crate::tray_settings::SETTINGS_FILE),
            br#"{"trayEnabled": true}"#,
        )
        .unwrap();
        assert!(configure_updater_relay_directory(
            &controller,
            Some(directory.clone())
        ));
        assert!(controller.status().updater_relay.enabled);
        std::fs::remove_file(directory.join(crate::tray_settings::SETTINGS_FILE)).unwrap();
        assert!(configure_updater_relay_directory(
            &controller,
            Some(directory.clone())
        ));
        assert!(controller.status().updater_relay.enabled);

        // And the extra domains reach it too.
        std::fs::write(
            directory.join(crate::tray_settings::SETTINGS_FILE),
            br#"{"updaterRelayExtraDomains": ["downloads.example.net"]}"#,
        )
        .unwrap();
        assert!(configure_updater_relay_directory(
            &controller,
            Some(directory.clone())
        ));
        assert_eq!(
            controller.status().updater_relay.allowed_domains,
            ["neuraldsp.com", "downloads.example.net"]
        );
    }
}
