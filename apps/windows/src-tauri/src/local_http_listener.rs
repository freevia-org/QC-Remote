//! Second credential audience in the EXISTING host/runtime. No host spawning.
use super::local_http_binding::StablePort;
use super::local_http_helper::{DirectDescriptor, DESCRIPTOR_FILE, PORT_FILE};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use qc_remote_mcp::QcBackend;
use rand::RngExt as _;
use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub(crate) struct DirectHost {
    directory: PathBuf,
    backend: Arc<dyn QcBackend>,
    instance: String,
    prepared: Option<Published>,
    selection: Option<Vec<u8>>,
}

struct Published {
    listener: Option<TcpListener>,
    descriptor: DirectDescriptor,
    path: PathBuf,
    _lease: fs::File,
}

impl Published {
    fn prepare(directory: &Path, instance: &str) -> Result<Option<Self>, String> {
        let record = directory.join(PORT_FILE);
        match fs::symlink_metadata(&record) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("Could not inspect the protected HTTP pairing".into()),
            Ok(_) => {}
        }
        let port = StablePort::read(&record)?;
        let path = directory.join(DESCRIPTOR_FILE);
        let lease =
            crate::local_access::acquire_document_lease(&path, "publisher", Duration::ZERO)?;
        let listener = port.bind()?;
        let mut bytes = [0_u8; 32];
        rand::rng().fill(&mut bytes);
        let process_id = std::process::id();
        let process_created_at = qc_native_platform::process::os_process_created_at(process_id)
            .ok_or("Could not prove this HTTP host's OS identity")?;
        Ok(Some(Self {
            listener: Some(listener),
            path,
            _lease: lease,
            descriptor: DirectDescriptor {
                schema_version: 1,
                port: port.port(),
                bearer_token: URL_SAFE_NO_PAD.encode(bytes),
                instance_id: instance.to_owned(),
                process_id,
                process_created_at,
                published_at_unix_ms: 0,
            },
        }))
    }

    fn publish(&mut self) -> Result<(), String> {
        self.publish_with(qc_owner_only_file::publish_atomically)
    }

    fn publish_with(
        &mut self,
        publisher: impl FnOnce(&Path, &[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut next = self.descriptor.clone();
        next.published_at_unix_ms = crate::broker_paths::unix_time_ms();
        let bytes =
            serde_json::to_vec(&next).map_err(|_| "Could not encode the HTTP descriptor")?;
        publisher(&self.path, &bytes)
            .map_err(|_| "Could not publish the owner-private HTTP descriptor")?;
        self.descriptor = next;
        Ok(())
    }

    fn private_record_matches(&self) -> bool {
        qc_owner_only_file::read_owner_only_bounded(&self.path, 4096)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DirectDescriptor>(&bytes).ok())
            .is_some_and(|record| {
                record.schema_version == 1
                    && record.port == self.descriptor.port
                    && record.instance_id == self.descriptor.instance_id
                    && record.bearer_token == self.descriptor.bearer_token
                    && record.process_id == self.descriptor.process_id
                    && record.process_created_at == self.descriptor.process_created_at
            })
    }
}

#[cfg(test)]
#[path = "local_http_tests.rs"]
mod tests;

impl Drop for Published {
    fn drop(&mut self) {
        // Only our epoch; never delete a successor/foreign descriptor.
        if qc_owner_only_file::read_owner_only_bounded(&self.path, 4096)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DirectDescriptor>(&bytes).ok())
            .is_some_and(|record| {
                record.instance_id == self.descriptor.instance_id
                    && record.bearer_token == self.descriptor.bearer_token
            })
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl DirectHost {
    pub(crate) fn prepare(directory: &Path, backend: Arc<dyn QcBackend>, instance: &str) -> Self {
        // Startup failure refuses ONLY the HTTP audience, not compatibility.
        // No port fallback/reassignment or timer retry after that refusal.
        let prepared = match Published::prepare(directory, instance) {
            Ok(prepared) => prepared,
            Err(error) => {
                eprintln!("QC Remote shared HTTP startup refused: {error}");
                None
            }
        };
        Self {
            directory: directory.to_owned(),
            backend,
            instance: instance.to_owned(),
            prepared,
            selection: StablePort::activation(&directory.join(PORT_FILE)).ok(),
        }
    }

    pub(crate) async fn run(mut self) -> Result<(), String> {
        // Pair may first select a port while this host is already running.
        // Observe only that protected record, not device state. No new thread,
        // no second process, no reassignment and no host startup from a helper.
        let mut last_selection = self.selection.take();
        loop {
            if let Some(prepared) = self.prepared.take() {
                if let Err(error) = self.serve(prepared).await {
                    eprintln!("QC Remote shared HTTP pairing refused: {error}");
                }
            }
            let selected = StablePort::activation(&self.directory.join(PORT_FILE)).ok();
            if selected.is_some() && selected != last_selection {
                last_selection = selected;
                match Published::prepare(&self.directory, &self.instance) {
                    Ok(prepared) => self.prepared = prepared,
                    Err(error) => eprintln!("QC Remote shared HTTP pairing refused: {error}"),
                }
                // Exactly one attempt per explicit Pair revision. Never spin
                // retrying an occupied port or a failed publication on a tick.
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn serve(&self, mut published: Published) -> Result<(), String> {
        let listener = tokio::net::TcpListener::from_std(
            published.listener.take().ok_or("HTTP socket missing")?,
        )
        .map_err(|_| "Could not adopt the exclusive HTTP socket")?;
        let router = crate::local_access::direct_http_router(
            self.backend.clone(),
            Arc::from(published.descriptor.bearer_token.clone()),
            Arc::from(self.instance.clone()),
            published.descriptor.port,
        );
        published.publish()?;
        let serve = crate::local_access::serve_loopback(
            listener,
            router,
            Duration::from_secs(15),
            std::future::pending(),
        );
        tokio::pin!(serve);
        let mut refresh = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                result = &mut serve => return result.map_err(|_| "Shared HTTP listener stopped".into()),
                _ = refresh.tick() => {
                    if !published.private_record_matches() {
                        return Err("Shared HTTP descriptor ownership or identity became unavailable".into());
                    }
                    if published.publish().is_err() {
                        if !published.private_record_matches() || !within_publication_grace(published.descriptor.published_at_unix_ms,crate::broker_paths::unix_time_ms()) {
                            return Err("Shared HTTP publication could not be renewed safely".into());
                        }
                        // Existing30s cadence bounds retries/logging. Failure
                        // NEVER advances the last successful publication stamp.
                        eprintln!("Shared HTTP refresh failed; retrying within the last successful publication budget");
                    }
                },
            }
        }
        // Outer native-owner shutdown drops this future/socket and Published;
        // runtime retirement precedes native acknowledgement/publication release.
    }
}

fn within_publication_grace(last_success: u64, now: u64) -> bool {
    now >= last_success && now.saturating_sub(last_success) < 120_000
}
