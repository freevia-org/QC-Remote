//! Stable direct-HTTP port admission. No credential, host startup or device I/O.
//!
//! Exclusivity protects a live listener against co-binding; it does NOT reserve
//! a port after shutdown or authenticate a later listener to a cached client.

use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::num::NonZeroU16;
use std::os::windows::io::AsRawSocket;
use std::path::Path;

const MAX_PORT_RECORD_BYTES: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortRecord {
    schema_version: u8,
    port: u16,
    #[serde(default)]
    pairing_revision: u64,
}

/// A previously selected port, never an implicit request to select a new one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct StablePort(NonZeroU16);

impl StablePort {
    /// Explicit Pair only. The caller holds the owner-private pairing lease.
    /// Existing metadata is never silently reassigned, even when occupied.
    pub(crate) fn select_for_pairing(path: &Path) -> Result<Self, String> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                let bytes = Self::activation(path)?;
                let selected = Self::parse(&bytes)?;
                let mut record: PortRecord = serde_json::from_slice(&bytes)
                    .map_err(|_| "The shared HTTP port record is invalid")?;
                record.pairing_revision = record
                    .pairing_revision
                    .checked_add(1)
                    .ok_or("The pairing revision is exhausted")?;
                qc_owner_only_file::publish_atomically(
                    path,
                    &serde_json::to_vec(&record)
                        .map_err(|_| "Could not encode the pairing revision")?,
                )
                .map_err(|_| "Could not publish the explicit pairing revision")?;
                Ok(selected)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let reservation = bind_exclusive(0)
                    .map_err(|_| "Could not select an exclusive shared HTTP port")?;
                let port = reservation
                    .local_addr()
                    .map_err(|_| "Could not inspect the selected port")?
                    .port();
                let selected = Self(NonZeroU16::new(port).ok_or("No port was selected")?);
                let bytes = serde_json::to_vec(&PortRecord {
                    schema_version: 1,
                    port,
                    pairing_revision: 1,
                })
                .map_err(|_| "Could not encode the selected port")?;
                qc_owner_only_file::publish_atomically(path, &bytes)
                    .map_err(|_| "Could not publish the owner-private selected port")?;
                // This is NOT a persistent reservation: startup must bind the
                // same port exclusively or refuse. No foreign listener fallback.
                drop(reservation);
                Ok(selected)
            }
            Err(_) => Err("Could not inspect the selected HTTP port record".into()),
        }
    }
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_PORT_RECORD_BYTES {
            return Err("The shared HTTP port record exceeds its byte budget".into());
        }
        let record: PortRecord =
            serde_json::from_slice(bytes).map_err(|_| "The shared HTTP port record is invalid")?;
        if record.schema_version != 1 {
            return Err("The shared HTTP port record version is unsupported".into());
        }
        NonZeroU16::new(record.port)
            .map(Self)
            .ok_or_else(|| "The shared HTTP port must be a selected nonzero port".into())
    }

    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let bytes = Self::activation(path)?;
        Self::parse(&bytes)
    }

    pub(crate) fn activation(path: &Path) -> Result<Vec<u8>, String> {
        qc_owner_only_file::read_owner_only_bounded(path, MAX_PORT_RECORD_BYTES)
            .map_err(|_| "Could not read the owner-private shared HTTP port record".into())
    }

    pub(crate) fn port(self) -> u16 {
        self.0.get()
    }

    pub(crate) fn url(self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port())
    }

    /// Called only inside the publisher lease; no retry or replacement port.
    pub(crate) fn bind(self) -> Result<TcpListener, String> {
        bind_exclusive(self.port()).map_err(|_| {
            format!(
                "QC Remote could not exclusively bind its selected shared HTTP port {}. \
                 Close the conflicting listener, then choose Pair again; \
                 the port was not changed and no other process was stopped.",
                self.port()
            )
        })
    }
}

fn bind_exclusive(port: u16) -> io::Result<TcpListener> {
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, WSAGetLastError, SOCKET_ERROR, SOL_SOCKET, SO_EXCLUSIVEADDRUSE,
    };
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    let enabled: i32 = 1;
    // SAFETY: the fresh owned socket remains live; enabled is an aligned i32
    // readable for the exact option length. Exclusivity is set BEFORE bind.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as _,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&enabled as *const i32).cast(),
            std::mem::size_of::<i32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        // SAFETY: WSAGetLastError takes no pointers and reads this thread's error.
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
    }
    socket.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_record_accepts_only_a_literal_nonzero_selected_port() {
        let port = StablePort::parse(br#"{"schemaVersion":1,"port":41012}"#).unwrap();
        assert_eq!(port.url(), "http://127.0.0.1:41012/mcp");
        for bytes in [
            br#"{"schemaVersion":1,"port":0}"#.as_slice(),
            br#"{"schemaVersion":2,"port":41012}"#,
            br#"{"schemaVersion":1,"port":65536}"#,
            br#"{"schemaVersion":1,"port":41012,"host":"0.0.0.0"}"#,
        ] {
            assert!(StablePort::parse(bytes).is_err());
        }
        assert!(StablePort::parse(&vec![b' '; MAX_PORT_RECORD_BYTES + 1]).is_err());
    }

    #[test]
    fn exclusive_listener_refuses_loopback_cobind_and_wildcard_cannot_receive_its_connections() {
        let owner = bind_exclusive(0).unwrap();
        let address = owner.local_addr().unwrap();
        assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
        let selected = StablePort(NonZeroU16::new(address.port()).unwrap());
        assert!(selected.bind().is_err());
        // Windows Server2003+ permits a wildcard bind excluding the specific
        // exclusive address. Microsoft documents this as enhanced socket
        // security, NOT a hijack. Prove which socket gets loopback traffic.
        // https://learn.microsoft.com/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse
        if let Ok(wildcard) = TcpListener::bind((Ipv4Addr::UNSPECIFIED, address.port())) {
            wildcard.set_nonblocking(true).unwrap();
            let connection =
                std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(2))
                    .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let accepted = loop {
                match owner.accept() {
                    Ok((accepted, _)) => break accepted,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(error) => {
                        panic!("exclusive owner did not receive loopback connection: {error}")
                    }
                }
            };
            assert!(
                matches!(wildcard.accept(),Err(error) if error.kind()==io::ErrorKind::WouldBlock)
            );
            drop(connection);
            drop(accepted);
            drop(wildcard);
        }
        assert_eq!(owner.local_addr().unwrap(), address);
        drop(owner);
        // TCP retirement can retain the tuple beyond close; admission must
        // refuse rather than use REUSEADDR or silently choose another port.
    }

    #[test]
    fn a_presquatted_selected_port_refuses_without_fallback_or_displacement() {
        let stranger = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = stranger.local_addr().unwrap();
        let selected = StablePort(NonZeroU16::new(address.port()).unwrap());
        let error = selected.bind().unwrap_err();
        assert!(error.contains("port was not changed"));
        assert_eq!(stranger.local_addr().unwrap(), address);
    }
}
