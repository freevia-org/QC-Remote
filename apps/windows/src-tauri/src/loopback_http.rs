//! HTTP clients for endpoints on this machine's loopback interface.
//!
//! reqwest honours the system proxy (`HTTP_PROXY`/`HTTPS_PROXY`, and the
//! per-user Windows proxy) for every URL, `127.0.0.1` included. A loopback
//! request routed through a proxy would hand that proxy the bearer token the
//! request carries, and any process the user can be talked into configuring as
//! a proxy could then call the local MCP endpoint as the user. Nothing these
//! clients talk to is ever off this machine, so they never use a proxy.

/// A client builder for loopback endpoints: proxies disabled.
pub(crate) fn loopback_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_proxy()
}

/// A loopback client that also accepts the self-signed certificate a local
/// language server generates per session (Google Antigravity).
///
/// Only for `https://127.0.0.1:<port>` URLs whose owning process the caller
/// has already tied to an installed executable; never for any other host.
pub(crate) fn loopback_self_signed_client_builder() -> reqwest::ClientBuilder {
    loopback_client_builder().danger_accept_invalid_certs(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    const PROBE_MARKER: &str = "QC_REMOTE_LOOPBACK_PROXY_PROBE";

    /// Answer every connection with a fixed status and count them.
    struct Server {
        port: u16,
        hits: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn serve(status: &'static str) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&hits);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => return,
                };
                if stream.set_nonblocking(false).is_err()
                    || stream
                        .set_read_timeout(Some(Duration::from_millis(200)))
                        .is_err()
                    || stream
                        .set_write_timeout(Some(Duration::from_millis(200)))
                        .is_err()
                {
                    return;
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                );
            }
        });
        Server {
            port,
            hits,
            stop,
            worker: Some(worker),
        }
    }

    /// Runs in a child process with `HTTP_PROXY` pointed at a fake proxy, so
    /// the environment of every other test in this binary stays untouched.
    #[tokio::test]
    #[ignore = "run by loopback_clients_never_use_a_configured_proxy in a child process"]
    async fn loopback_proxy_probe() {
        let Ok(proxy_port) = std::env::var(PROBE_MARKER) else {
            return;
        };
        let target = serve("204 No Content");
        let url = format!("http://127.0.0.1:{}/health", target.port);

        // Control: a default client in this environment does use the proxy,
        // so the environment really is hostile and the check below means
        // something.
        let default = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(&url)
            .send()
            .await
            .unwrap();
        assert_eq!(
            default.status(),
            502,
            "the default client bypassed the proxy"
        );
        assert_eq!(target.hits.load(Ordering::SeqCst), 0);

        for builder in [
            loopback_client_builder(),
            loopback_self_signed_client_builder(),
        ] {
            let response = builder
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap()
                .get(&url)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 204, "the request went through the proxy");
        }
        assert_eq!(target.hits.load(Ordering::SeqCst), 2);
        println!("probe-proxy-port={proxy_port}");
    }

    #[test]
    fn loopback_clients_never_use_a_configured_proxy() {
        let server = serve("502 Bad Gateway");
        let proxy_port = server.port;
        let proxy = format!("http://127.0.0.1:{proxy_port}");
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "loopback_http::tests::loopback_proxy_probe",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PROBE_MARKER, proxy_port.to_string())
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env("ALL_PROXY", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy");
        #[cfg(windows)]
        let output =
            qc_native_platform::process::owned_tool::run(&mut command, Duration::from_secs(30))
                .unwrap();
        #[cfg(not(windows))]
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains(&format!("probe-proxy-port={proxy_port}")),
            "the probe did not run: {stdout}"
        );
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            1,
            "only the control request may reach the proxy"
        );
    }

    #[test]
    fn fixture_unwind_joins_listener_even_with_an_unfinished_request() {
        let server = serve("204 No Content");
        let port = server.port;
        let client = std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_secs(2),
        )
        .unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned = server;
            panic!("injected fixture failure");
        }));
        assert!(result.is_err());
        assert!(std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_millis(100)
        )
        .is_err());
        drop(client);
    }
}
