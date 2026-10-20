#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! The shipped `qc-device-broker.exe`: one process per user session that is
//! the device broker, the broker icon and the background MCP host. The three
//! cannot work independently, so the source code stays separate but the
//! running binary is one.
//!
//! The source stays separate: the broker, its icon and the residence rules are
//! `qc_device_broker` (services/device-broker), the MCP host is
//! `qc_remote_lib`'s background host. This file only links them. It replaces
//! the separate `qc-remote-tray.exe` of 0.1.176 and 0.1.177.
//!
//! The windows subsystem keeps a logon start from opening a console. Piped
//! standard handles (`--stdio`, `--self-check` from the release checks) work
//! the same either way; a developer who wants console output from `--probe`
//! uses the broker-only build in services/device-broker.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Identifies this build from its bytes alone: the release and dev scripts
/// read it out of the file before they run the file at all, so a stale
/// broker-only build (which would treat `--self-check` as a live device probe)
/// is never executed. `#[used]` keeps it in the image; `--self-check` reports
/// it as well.
#[used]
static RESIDENT_HOST_BUILD_MARKER: &str = concat!(
    "qc-remote-resident-host-build:v1:",
    env!("CARGO_PKG_VERSION"),
    ":end"
);

struct McpHost;

struct RunningMcpHost(qc_remote_lib::BackgroundMcpHost);

impl qc_device_broker::resident::RunningHost for RunningMcpHost {
    fn finished(&self) -> bool {
        self.0.finished()
    }
    fn stop(&mut self, within: Duration) {
        self.0.stop_within(within);
    }
}

impl qc_device_broker::resident::ResidentHost for McpHost {
    fn start(
        &self,
        directory: &Path,
        host_token: &str,
        readiness: qc_device_broker::resident::DeviceReadinessProbe,
    ) -> Result<Box<dyn qc_device_broker::resident::RunningHost>, String> {
        let readiness: qc_remote_lib::BrokerReadinessProbe =
            Arc::new(move || host_readiness(readiness.read()));
        qc_remote_lib::start_background_mcp_host(
            directory.to_path_buf(),
            host_token.to_owned(),
            readiness,
        )
        .map(|host| Box::new(RunningMcpHost(host)) as _)
    }

    /// The icon's Unlink on a local client. The host's session registry is
    /// process-wide, so this reaches the running host without a channel.
    fn end_local_session(&self, session_id: &str) -> Result<bool, String> {
        Ok(qc_remote_lib::end_local_mcp_session(session_id))
    }

    /// The icon's Unlink on a remote client: this process runs the relay, so
    /// it resolves the session's grant from its own live presence.
    fn unlink_remote_session(
        &self,
        session_id: &str,
    ) -> Result<qc_device_broker::resident::RemoteUnlink, String> {
        qc_remote_lib::unlink_remote_mcp_session(session_id).map(remote_unlink)
    }

    /// The icon's Unlink on a linked app: resolved from this process's own
    /// linked-apps list by the opaque entry id.
    fn unlink_linked_app(
        &self,
        entry_id: &str,
    ) -> Result<qc_device_broker::resident::RemoteUnlink, String> {
        qc_remote_lib::unlink_linked_remote_app(entry_id).map(remote_unlink)
    }
}

fn remote_unlink(
    outcome: qc_remote_lib::RemoteMcpUnlink,
) -> qc_device_broker::resident::RemoteUnlink {
    use qc_device_broker::resident::RemoteUnlink;
    use qc_remote_lib::RemoteMcpUnlink;
    match outcome {
        RemoteMcpUnlink::Unlinked => RemoteUnlink::Unlinked,
        RemoteMcpUnlink::NotListed => RemoteUnlink::NotListed,
        RemoteMcpUnlink::SessionEnded => RemoteUnlink::SessionEnded,
        RemoteMcpUnlink::GrantUnknown => RemoteUnlink::GrantUnknown,
        RemoteMcpUnlink::StaleList => RemoteUnlink::StaleList,
    }
}

/// The broker's account of its device, in the host's own terms. The host
/// crate does not depend on the broker crate, so this binary, which links
/// both, is the one place the two meet.
fn host_readiness(
    readiness: qc_device_broker::resident::DeviceReadiness,
) -> qc_remote_lib::BrokerDeviceReadiness {
    use qc_device_broker::resident::DeviceReadiness;
    use qc_remote_lib::BrokerDeviceReadiness;
    match readiness {
        DeviceReadiness::Unknown => BrokerDeviceReadiness::Unknown,
        DeviceReadiness::Ready => BrokerDeviceReadiness::Ready,
        DeviceReadiness::ReadyTarget {
            usb_connected,
            target,
        } => {
            match serde_json::to_value(target)
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
            {
                Some(target) => BrokerDeviceReadiness::ReadyTarget {
                    usb_connected,
                    target,
                },
                None => BrokerDeviceReadiness::Unavailable {
                    reason: "The broker target metadata was invalid".into(),
                },
            }
        }
        DeviceReadiness::NotReady { phase } => BrokerDeviceReadiness::NotReady { phase },
        DeviceReadiness::Unavailable(reason) => BrokerDeviceReadiness::Unavailable { reason },
    }
}

fn main() {
    // This process is the broker. Nothing in it may ever spawn another one:
    // the host's gateway attaches to the descriptor this process publishes.
    qc_remote_lib::forbid_broker_spawn();
    qc_device_broker::cli::main(qc_device_broker::cli::Binary {
        version: env!("CARGO_PKG_VERSION"),
        default_directory: Some(qc_remote_lib::broker_paths::app_local_data_dir),
        host: Some(Arc::new(McpHost)),
        build_marker: Some(RESIDENT_HOST_BUILD_MARKER),
    });
}

#[cfg(test)]
mod tests {
    use super::host_readiness;
    use qc_device_broker::resident::DeviceReadiness;
    use qc_remote_lib::BrokerDeviceReadiness;

    /// Every broker answer keeps its meaning and its detail across the
    /// crate boundary: only `Ready` may become ready.
    #[test]
    fn every_broker_readiness_maps_to_the_same_host_readiness() {
        assert_eq!(
            host_readiness(DeviceReadiness::Unknown),
            BrokerDeviceReadiness::Unknown
        );
        assert_eq!(
            host_readiness(DeviceReadiness::Ready),
            BrokerDeviceReadiness::Ready
        );
        for phase in ["disconnected", "searching", "handshaking", "syncing"] {
            assert_eq!(
                host_readiness(DeviceReadiness::NotReady {
                    phase: phase.into()
                }),
                BrokerDeviceReadiness::NotReady {
                    phase: phase.into()
                }
            );
        }
        assert_eq!(
            host_readiness(DeviceReadiness::Unavailable(
                "the device broker is shutting down".into()
            )),
            BrokerDeviceReadiness::Unavailable {
                reason: "the device broker is shutting down".into()
            }
        );
    }
}
