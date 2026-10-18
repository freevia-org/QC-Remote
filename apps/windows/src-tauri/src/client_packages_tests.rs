#![cfg(test)]

use super::*;
// Only the tests name the three liveness values directly: production code
// asks the evidence what it means rather than matching on it.
use crate::local_access::LocalClientLiveness;
use std::io::Read;

/// The fixture's own bound for whole-script checks: a cold Windows PowerShell
/// or node start plus its script. Loaded CI runners exceeded the former 10s,
/// so the run, not the product, timed out. A healthy run returns in seconds;
/// this never changes a production status-tool deadline.
#[cfg(windows)]
const SCRIPT_FIXTURE_TIMEOUT: Duration = Duration::from_secs(120);

fn create_file_symlink_for_test(source: &Path, target: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(source, target).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source, target);
        false
    }
}

#[test]
fn package_export_preserves_existing_file_after_partial_write_and_unwind() {
    let directory = crate::tests::unique_fixture_directory("package-export-atomic");
    let _guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let path = directory.join("clients.zip");
    let previous = b"previous complete client package";
    for unwind in [false, true] {
        fs::write(&path, previous).unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_package_bytes_with(
                &path,
                b"replacement complete client package",
                |file, bytes| {
                    file.write_all(&bytes[..7])?;
                    if unwind {
                        panic!("injected package export writer unwind");
                    }
                    Err(std::io::Error::other(
                        "injected package export partial write",
                    ))
                },
            )
        }));
        if unwind {
            assert!(outcome.is_err(), "the injected unwind remains an unwind");
        } else {
            assert!(
                outcome.unwrap().is_err(),
                "a partial write must not report success"
            );
        }
        assert_eq!(
            fs::read(&path).unwrap(),
            previous,
            "the selected previous export must survive an unsuccessful replacement"
        );
        let entries = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![path.clone()],
            "failed writer must retire only its owned stage"
        );
    }
    let healthy = vec![0x5a; 128 * 1024];
    write_package_bytes_with(&path, &healthy, |file, bytes| file.write_all(bytes)).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        healthy,
        "healthy replacement remains byte-exact"
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
}

#[tokio::test]
async fn antigravity_state_response_envelope_bounds_actual_http_bytes_and_preserves_route_truth() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const LIMIT: usize = 256;
    for mode in [
        "known-exact",
        "chunked-exact",
        "close-exact",
        "chunked-growth",
        "declared-over",
        "deadline",
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/fixture-state", listener.local_addr().unwrap());
        let client = crate::loopback_http::loopback_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let executable = Path::new("C:/fixture/qc-remote.exe");
        let expected = json!({"states":[{
            "status":"MCP_SERVER_STATUS_READY",
            "spec":{"serverName":"qc-remote","command":executable.to_string_lossy(),
                "args":["--local-mcp-bridge"]}
        }]});
        let mut payload = serde_json::to_vec(&expected).unwrap();
        assert!(payload.len() < LIMIT);
        payload.resize(LIMIT + usize::from(mode == "chunked-growth"), b' ');
        let (headers_seen, headers_received) = tokio::sync::oneshot::channel();
        let (done, completed) = tokio::sync::oneshot::channel();
        let server = async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 8192];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            let framing = match mode {
                "known-exact" => format!("Content-Length: {LIMIT}\r\n"),
                "declared-over" => format!("Content-Length: {}\r\n", LIMIT + 1),
                "deadline" => "Content-Length: 2\r\n".to_owned(),
                "close-exact" => String::new(),
                _ => "Transfer-Encoding: chunked\r\n".to_owned(),
            };
            socket
                .write_all(
                    format!("HTTP/1.1 200 OK\r\n{framing}Connection: close\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
            // No body existed when reqwest returned the actual response. This
            // gates later chunk growth, rather than trusting Content-Length.
            let _ = headers_received.await;
            match mode {
                "declared-over" | "deadline" => {
                    let _ = completed.await;
                }
                "chunked-exact" | "chunked-growth" => {
                    let split = LIMIT / 2;
                    for part in [&payload[..split], &payload[split..]] {
                        let chunk = format!("{:x}\r\n", part.len());
                        if socket.write_all(chunk.as_bytes()).await.is_err() {
                            return;
                        }
                        if socket.write_all(part).await.is_err() {
                            return;
                        }
                        if socket.write_all(b"\r\n").await.is_err() {
                            return;
                        }
                    }
                    let _ = socket.write_all(b"0\r\n\r\n").await;
                }
                _ => {
                    let _ = socket.write_all(&payload).await;
                }
            }
        };
        let collect = async {
            let response = client.get(url).send().await.unwrap();
            let _ = headers_seen.send(());
            if matches!(mode, "chunked-exact" | "chunked-growth" | "close-exact") {
                assert_eq!(response.content_length(), None);
            }
            let deadline = Instant::now()
                + if mode == "deadline" {
                    Duration::from_millis(60)
                } else {
                    Duration::from_secs(2)
                };
            let result = read_antigravity_state_response(response, deadline, LIMIT).await;
            let _ = done.send(());
            result
        };
        // Exact joined futures own both sockets; failure/timeout drops them
        // together. No task, native thread, process or scratch root is detached.
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(collect, server)
        })
        .await
        .expect("bounded actual HTTP fixture");
        if matches!(mode, "chunked-growth" | "declared-over") {
            assert!(
                result.unwrap_err().contains("software document envelope"),
                "{mode}"
            );
        } else if mode == "deadline" {
            assert!(result.unwrap_err().contains("total operation deadline"));
        } else {
            let document = result.unwrap_or_else(|error| panic!("{mode}: {error}"));
            assert_eq!(document, expected);
            assert!(antigravity_state_has_current_qc_remote(
                &document, executable
            ));
            assert!(
                !antigravity_state_has_current_qc_remote(
                    &document,
                    Path::new("C:/other/qc-remote.exe")
                ),
                "a healthy body must not manufacture matching route authority"
            );
        }
    }
}

#[test]
fn held_client_document_reader_accepts_exact_limit_and_refuses_growth_after_open() {
    let directory = crate::tests::unique_fixture_directory("client-document-held-growth");
    let _guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let path = directory.join("document.json");
    fs::write(&path, b"{}").unwrap();
    let exact = fs::OpenOptions::new().write(true).open(&path).unwrap();
    exact.set_len(CLIENT_DOCUMENT_MAX_BYTES as u64).unwrap();
    drop(exact);
    assert_eq!(
        read_client_document_bytes(&path).unwrap().len(),
        CLIENT_DOCUMENT_MAX_BYTES
    );
    let held = fs::File::open(&path).unwrap();
    let grow = fs::OpenOptions::new().write(true).open(&path).unwrap();
    grow.set_len((CLIENT_DOCUMENT_MAX_BYTES + 1) as u64)
        .unwrap();
    drop(grow);
    let error = read_client_document_from(held).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        (CLIENT_DOCUMENT_MAX_BYTES + 1) as u64
    );
    assert!(staged_leftovers(&directory).is_empty());
}

#[test]
fn oversized_valid_client_configuration_is_refused_without_overwrite_or_backup() {
    let directory = crate::tests::unique_fixture_directory("client-document-envelope");
    let _guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("mcp.json");
    let mut bytes = br#"{"foreign":{"preserved":true},"mcpServers":{}}"#.to_vec();
    bytes.resize(16 * 1024 * 1024 + 1, b' ');
    fs::write(&config, &bytes).unwrap();
    let result = write_json_mcp_route(
        &config,
        Path::new("C:/fixture/qc-remote.exe"),
        JAN_CLIENT_ID,
        "fixture",
        true,
    );
    assert!(
        result.is_err(),
        "Oversized valid JSON must be refused before route replacement"
    );
    assert_eq!(
        fs::read(&config).unwrap(),
        bytes,
        "Prior complete bytes remain unchanged"
    );
    assert!(
        config_backups(&config).is_empty(),
        "No write transaction was admitted"
    );
    assert!(staged_leftovers(&directory).is_empty());
}

#[test]
fn ordinary_client_configuration_keeps_unknown_fields_through_the_shared_reader() {
    let directory = crate::tests::unique_fixture_directory("client-document-healthy");
    let _guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("mcp.json");
    fs::write(
        &config,
        br#"{"foreign":{"preserved":[1,2,3]},"mcpServers":{"other":{"command":"elsewhere"}}}"#,
    )
    .unwrap();
    write_json_mcp_route(
        &config,
        Path::new("C:/fixture/qc-remote.exe"),
        JAN_CLIENT_ID,
        "fixture",
        true,
    )
    .unwrap();
    let result: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(result["foreign"], json!({"preserved":[1,2,3]}));
    assert_eq!(result["mcpServers"]["other"]["command"], "elsewhere");
    assert_eq!(result["mcpServers"][CLAUDE_EXTENSION_NAME]["active"], true);
    assert_eq!(config_backups(&config).len(), 1);
    assert!(staged_leftovers(&directory).is_empty());
}

#[test]
fn client_fixture_root_is_removed_on_partial_setup_and_assertion_unwind() {
    for populated in [false, true] {
        let mut root = None;
        let primary = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let directory = crate::tests::unique_fixture_directory("client-unwind");
            root = Some(directory.clone());
            let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
            if populated {
                fs::create_dir_all(directory.join("nested")).unwrap();
                fs::write(directory.join("nested/config.json"), b"partial").unwrap();
            }
            panic!("injected client fixture failure");
        }));
        assert!(primary.is_err(), "primary failure is not swallowed");
        assert!(
            !root.expect("exclusive fixture allocated").exists(),
            "owned fixture removed even before happy-path cleanup"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn non_windows_node_syntax_fixture_preserves_real_success_and_parser_failure() {
    let directory = crate::tests::unique_fixture_directory("node-parser-contract");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let file = directory.join("fixture.cjs");
    fs::write(&file, b"const valid = 1;\n").unwrap();
    let valid = crate::tests::run_node_syntax_fixture(&file).expect("actual Node parser");
    assert!(valid.status.success());
    fs::write(&file, b"const = ;\n").unwrap();
    let invalid = crate::tests::run_node_syntax_fixture(&file).expect("actual Node parser failure");
    assert!(
        !invalid.status.success(),
        "invalid JavaScript is not treated as success"
    );
    assert!(
        !invalid.stderr.is_empty(),
        "parser diagnostics remain available"
    );
}

#[test]
fn owned_config_stage_cleans_partial_failure_unwind_and_preserves_collisions() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(crate::tests::unique_fixture_directory("owned-stage"));
    let path = fixture.0.join("stage");
    let result = OwnedConfigStage::create_with(path.clone(), |file| {
        assert_eq!(file.metadata()?.len(), 0);
        assert!(
            qc_owner_only_file::is_owner_only(&path).unwrap(),
            "stage must be private before any secret byte"
        );
        file.write_all(b"partial")?;
        Err(std::io::Error::other("injected write fault"))
    });
    assert!(result.is_err());
    assert!(!path.exists());
    let panic = std::panic::catch_unwind(|| {
        let _ = OwnedConfigStage::create_with(path.clone(), |file| {
            file.write_all(b"partial")?;
            panic!("injected unwind");
        });
    });
    assert!(panic.is_err());
    assert!(!path.exists());
    fs::write(&path, b"contender").unwrap();
    assert!(OwnedConfigStage::write(path.clone(), b"replacement").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"contender");
    fs::remove_file(&path).unwrap();
    let destination = fixture.0.join("blocked");
    fs::create_dir(&destination).unwrap();
    assert!(OwnedConfigStage::write(path.clone(), b"complete")
        .unwrap()
        .install(&destination)
        .is_err());
    assert!(!path.exists());
    assert!(destination.is_dir());
    let installed = fixture.0.join("installed");
    OwnedConfigStage::write(path.clone(), b"complete")
        .unwrap()
        .install_with(&installed, || fs::write(&path, b"new occupant").unwrap())
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new occupant");
    assert_eq!(fs::read(&installed).unwrap(), b"complete");
}

#[test]
fn config_backup_publishes_complete_exclusive_bytes_and_cleans_failed_copy() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(crate::tests::unique_fixture_directory("owned-backup"));
    let config = fixture.0.join("config.json");
    assert!(back_up_config_file(&config, "fixture").is_err());
    assert!(config_backups(&config).is_empty());
    assert!(staged_leftovers(&fixture.0).is_empty());
    fs::write(&config, b"original").unwrap();
    let first = back_up_config_file(&config, "fixture").unwrap();
    assert!(qc_owner_only_file::is_owner_only(&first).unwrap());
    fs::write(&config, b"updated").unwrap();
    let second = back_up_config_file(&config, "fixture").unwrap();
    assert!(qc_owner_only_file::is_owner_only(&second).unwrap());
    assert_ne!(first, second);
    assert_eq!(fs::read(first).unwrap(), b"original");
    assert_eq!(fs::read(second).unwrap(), b"updated");
    assert_eq!(fs::read(config).unwrap(), b"updated");
    assert!(staged_leftovers(&fixture.0).is_empty());
}

#[test]
fn native_json_and_toml_config_publication_and_backups_remain_owner_private() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(crate::tests::unique_fixture_directory(
        "private-config-publication",
    ));
    let json_path = fixture.0.join("config.json");
    fs::write(&json_path, br#"{"foreign":{"token":"retained-secret"}}"#).unwrap();
    write_json_document(
        &json_path,
        &json!({"foreign":{"token":"retained-secret"},"qc":true}),
        "fixture",
    )
    .unwrap();
    assert!(qc_owner_only_file::is_owner_only(&json_path).unwrap());
    assert!(fs::read_to_string(&json_path)
        .unwrap()
        .contains("retained-secret"));
    let toml_path = fixture.0.join("config.toml");
    fs::write(&toml_path, "[mcp_servers.foreign]\ncommand = 'foreign'\n[mcp_servers.foreign.env]\nTOKEN = 'retained-secret'\n").unwrap();
    write_codex_config(
        &toml_path,
        &fixture.0.join("QC Remote.exe"),
        CODEX_CLIENT_ID,
    )
    .unwrap();
    assert!(qc_owner_only_file::is_owner_only(&toml_path).unwrap());
    assert!(fs::read_to_string(&toml_path)
        .unwrap()
        .contains("retained-secret"));
    let backups = config_backups(&toml_path)
        .into_iter()
        .chain(config_backups(&json_path))
        .collect::<Vec<_>>();
    assert!(
        !backups.is_empty(),
        "the existing foreign configuration must be backed up"
    );
    for (_, backup) in backups {
        assert!(
            qc_owner_only_file::is_owner_only(&backup).unwrap(),
            "backup {}",
            backup.display()
        );
    }
}

#[test]
fn all_config_document_writers_preserve_live_bytes_on_staging_collision() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(crate::tests::unique_fixture_directory("writer-collision"));
    let config = fixture.0.join("config");
    let executable = fixture.0.join("QC Remote.exe");
    for operation in 0..3 {
        let original = if operation == 0 {
            "{\"user\":true}"
        } else {
            "[mcp_servers.qc-remote]\ncommand = 'original'\n"
        };
        fs::write(&config, original).unwrap();
        let _fixed = PinnedStageSequence::new();
        let stage = staged_config_path(
            &fixture.0,
            &config,
            if operation == 2 {
                "qc-remote-remove"
            } else {
                "qc-remote"
            },
        );
        fs::write(&stage, b"contender").unwrap();
        let result = match operation {
            0 => write_json_document(&config, &json!({"user":false}), "fixture"),
            1 => write_codex_config(&config, &executable, CODEX_CLIENT_ID),
            _ => remove_codex_route(&config).map(|_| ()),
        };
        assert!(result.is_err(), "operation {operation}");
        assert_eq!(fs::read(&config).unwrap(), original.as_bytes());
        assert_eq!(fs::read(&stage).unwrap(), b"contender");
        assert!(config_backups(&config).is_empty());
        fs::remove_file(stage).unwrap();
    }
}

#[test]
fn jan_pair_refresh_and_unpair_preserve_user_configuration_and_private_files() {
    let directory = refresh_fixture("linux-jan-config-lifecycle");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = jan_target(&directory);
    fs::write(
        &target.config_path,
        br#"{"userPreferences":{"keep":true},"mcpServers":{"their-own-server":{"command":"their-own-server"}}}"#,
    )
    .expect("seed Jan configuration");

    write_json_mcp_route(
        &target.config_path,
        &target.executable_path,
        JAN_CLIENT_ID,
        "Jan",
        true,
    )
    .expect("pair Jan");
    let paired: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("paired configuration"))
            .expect("paired JSON");
    assert_eq!(paired["userPreferences"], json!({"keep": true}));
    assert_eq!(
        paired["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server")
    );
    assert!(qc_owner_only_file::is_owner_only(&target.config_path).unwrap());
    assert!(config_backups(&target.config_path)
        .iter()
        .all(|(_, backup)| qc_owner_only_file::is_owner_only(backup).unwrap()));

    // Mark only the managed identity stale; the route refresh must use the
    // same private writer while retaining every unrelated user setting.
    let stale_id = "jan:0.1.100";
    let mut stale = paired;
    stale["mcpServers"][CLAUDE_EXTENSION_NAME]["args"][2] = json!(stale_id);
    stale["mcpServers"][CLAUDE_EXTENSION_NAME]["env"]["QC_REMOTE_LOCAL_MCP_CLIENT"] =
        json!(stale_id);
    fs::write(
        &target.config_path,
        serde_json::to_vec_pretty(&stale).expect("stale Jan configuration"),
    )
    .expect("write stale fixture");
    let refreshed = refresh_managed_route(&target);
    assert_eq!(
        refreshed.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        }
    );
    let refreshed_document: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("refreshed configuration"))
            .expect("refreshed JSON");
    assert_eq!(refreshed_document["userPreferences"], json!({"keep": true}));
    assert_eq!(
        refreshed_document["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server")
    );
    assert_eq!(
        refreshed_document["mcpServers"][CLAUDE_EXTENSION_NAME]["args"][2],
        json!(JAN_CLIENT_ID)
    );
    assert!(qc_owner_only_file::is_owner_only(&target.config_path).unwrap());

    assert!(remove_json_mcp_route(&target.config_path, "Jan").expect("unpair Jan"));
    let unpaired: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("unpaired configuration"))
            .expect("unpaired JSON");
    assert_eq!(unpaired["userPreferences"], json!({"keep": true}));
    assert_eq!(
        unpaired["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server")
    );
    assert!(unpaired["mcpServers"][CLAUDE_EXTENSION_NAME].is_null());
    assert!(qc_owner_only_file::is_owner_only(&target.config_path).unwrap());
    assert!(config_backups(&target.config_path)
        .iter()
        .all(|(_, backup)| qc_owner_only_file::is_owner_only(backup).unwrap()));
}

#[cfg(unix)]
#[test]
fn json_config_writer_preserves_a_preexisting_symlink_at_its_stage_path() {
    let directory = refresh_fixture("config-stage-symlink");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("mcp.json");
    let original = br#"{"userPreferences":{"keep":true},"mcpServers":{}}"#;
    fs::write(&config, original).expect("seed live configuration");
    let target = directory.join("symlink-target");
    let target_bytes = b"do not follow or overwrite this target";
    fs::write(&target, target_bytes).expect("seed symlink target");
    let _pinned = PinnedStageSequence::new();
    let stage = staged_config_path(&directory, &config, "qc-remote");
    std::os::unix::fs::symlink(&target, &stage).expect("create occupied staged symlink");

    let result = write_json_mcp_route(
        &config,
        Path::new("/fixture/qc-remote"),
        JAN_CLIENT_ID,
        "Jan",
        true,
    );
    assert!(result.is_err(), "an existing stage path is never adopted");
    assert!(
        fs::symlink_metadata(&stage)
            .expect("stage remains present")
            .file_type()
            .is_symlink(),
        "failed cleanup must not remove a symlink it did not create"
    );
    assert_eq!(
        fs::read(&stage).expect("symlink still resolves"),
        target_bytes
    );
    assert_eq!(
        fs::read(&target).expect("target remains untouched"),
        target_bytes
    );
    assert_eq!(
        fs::read(&config).expect("live config remains untouched"),
        original
    );
    assert!(config_backups(&config).is_empty());
}

#[test]
fn antigravity_total_budget_is_not_restarted_by_another_port_or_body() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let deadline = Instant::now() + Duration::from_millis(40);
        within_antigravity_budget(deadline, async {
            tokio::time::sleep(Duration::from_millis(15)).await;
            Ok::<_, String>(())
        })
        .await
        .unwrap();
        let late =
            within_antigravity_budget(deadline, std::future::pending::<Result<(), String>>()).await;
        assert!(late.unwrap_err().contains("total operation deadline"));
        let mut dispatched = false;
        let expired = within_antigravity_budget(deadline, async {
            dispatched = true;
            Ok::<_, String>(())
        })
        .await;
        assert!(expired.is_err());
        assert!(!dispatched, "expired port/body must not be polled");
    });
    for ports in [
        vec![],
        vec![0],
        vec![123, 123],
        vec![1; ANTIGRAVITY_MAX_PORTS + 1],
    ] {
        assert!(validate_antigravity_ports(&ports).is_err());
    }
    assert!(validate_antigravity_ports(&[123, 456]).is_ok());
}

#[test]
fn antigravity_discovery_tool_budget_restores_on_success_and_unwind() {
    let original = TOOL_DEADLINE.with(Cell::get);
    let deadline = Instant::now() + Duration::from_millis(20);
    with_tool_deadline(deadline, || {
        assert_eq!(TOOL_DEADLINE.with(Cell::get), Some(deadline))
    });
    assert_eq!(TOOL_DEADLINE.with(Cell::get), original);
    assert!(
        std::panic::catch_unwind(|| with_tool_deadline(deadline, || panic!(
            "injected discovery fault"
        )))
        .is_err()
    );
    assert_eq!(TOOL_DEADLINE.with(Cell::get), original);
    // An expired scope refuses before trying to start even an invalid executable.
    let error = with_tool_deadline(Instant::now() - Duration::from_millis(1), || {
        run_with_timeout(
            &mut Command::new("qc-no-executable-ever"),
            Duration::from_secs(10),
        )
    })
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn expired_client_inspection_cannot_start_discovery() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dispatched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = dispatched.clone();
    let result = runtime.block_on(inspect_client_until(
        Instant::now() - Duration::from_millis(1),
        move || {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, String>(())
        },
    ));
    assert!(result.is_err());
    assert!(!dispatched.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn started_read_only_discovery_can_finish_but_cannot_admit_rpc_after_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (ended, finished) = std::sync::mpsc::channel();
    let rpc = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dispatched = rpc.clone();
    let deadline = Instant::now() + Duration::from_millis(100);
    let result = runtime.block_on(inspect_client_until(deadline, move || {
        // Controlled already-started read-only OS stand-in. The owned original
        // may outlive awaiting, but cannot launch a later RPC past deadline.
        std::thread::sleep(Duration::from_millis(200));
        let _ = ended.send(());
        remaining(deadline)?;
        dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, String>(())
    }));
    assert!(result.is_err());
    finished.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!rpc.load(std::sync::atomic::Ordering::SeqCst));
    qc_native_platform::thread_owner::reap();
}

#[test]
fn chatgpt_reload_is_not_blocked_by_an_older_codex_process() {
    let configured_at = 1_000_u128;
    assert!(chatgpt_restart_required_for_processes(configured_at, [900]));
    assert!(chatgpt_restart_required_for_processes(
        configured_at,
        [800, 900]
    ));
    assert!(
        !chatgpt_restart_required_for_processes(configured_at, [800, 1_100]),
        "a freshly reloaded ChatGPT or Codex host has the current shared configuration"
    );
    assert!(
        !chatgpt_restart_required_for_processes(configured_at, std::iter::empty()),
        "there is nothing to restart when neither host is running"
    );
}

/// The three states a host's bridge can be in, named so the restart rule
/// reads as the sentence it is meant to be.
fn evidence(liveness: LocalClientLiveness, ever_attached: bool) -> LocalClientEvidence {
    LocalClientEvidence {
        liveness,
        ever_attached,
    }
}

/// A bridge record in the two terms the status documents used to receive:
/// whether a heartbeat is live, and whether one was ever seen. The tests
/// that only exercise a state machine say it this way; the ones about the
/// start-up window name the third state with [`evidence`].
fn observed(connected: bool, attached: bool) -> LocalClientEvidence {
    evidence(
        if connected {
            LocalClientLiveness::Connected
        } else {
            LocalClientLiveness::Absent
        },
        attached,
    )
}

#[test]
fn chatgpt_mcp_reload_is_confirmed_by_the_bridge_handshake() {
    assert!(
        host_restart_required(true, evidence(LocalClientLiveness::Absent, false)),
        "nothing has announced itself, so process age is all there is to go on"
    );
    assert!(
        !host_restart_required(true, evidence(LocalClientLiveness::Absent, true)),
        "a host whose bridge has announced this build has read this configuration"
    );
    assert!(
        !host_restart_required(false, evidence(LocalClientLiveness::Absent, false)),
        "a host newer than the configuration was never asked to restart"
    );
}

#[test]
fn a_live_bridge_clears_the_restart_prompt_whatever_the_process_timestamps_say() {
    // The awkward case: the host process started before its
    // configuration was written, so the timestamp comparison says
    // "restart", and a bridge carrying this build's client id is
    // announcing itself the whole time. The bridge is the proof.
    let live = evidence(LocalClientLiveness::Connected, true);
    assert!(!host_restart_required(true, live));
    // And on the first poll after an app restart, before the record has
    // been rebuilt, a live announcement is still enough on its own.
    assert!(!host_restart_required(
        true,
        evidence(LocalClientLiveness::Connected, false)
    ));
}

#[test]
fn a_host_is_not_asked_to_restart_while_its_bridge_has_not_ticked_yet() {
    let settling = evidence(LocalClientLiveness::Settling, false);
    assert!(
        !host_restart_required(true, settling),
        "the app has not yet had time to hear from a healthy bridge"
    );
    assert!(
        !settling.connected(),
        "and it must not claim a connection either"
    );
}

#[test]
fn a_status_document_reports_the_start_up_window_without_claiming_a_connection() {
    let checking = chatgpt_status_document(
        true,
        true,
        true,
        true,
        Some(env!("CARGO_PKG_VERSION").to_string()),
        None,
        false,
        evidence(LocalClientLiveness::Settling, false),
    );
    assert_eq!(checking["state"], "current");
    assert_eq!(checking["connectionActive"], false);
    assert_eq!(checking["bridgeAttached"], false);
    assert_eq!(checking["connectionPending"], true);

    let connected = chatgpt_status_document(
        true,
        true,
        true,
        true,
        Some(env!("CARGO_PKG_VERSION").to_string()),
        None,
        false,
        evidence(LocalClientLiveness::Connected, true),
    );
    assert_eq!(connected["connectionActive"], true);
    assert_eq!(
        connected["connectionPending"], false,
        "an observed connection is not still being checked"
    );
}

#[test]
fn every_managed_provider_reports_liveness_on_the_same_three_terms() {
    let claude = claude_status_document(
        true,
        Some(env!("CARGO_PKG_VERSION").to_string()),
        evidence(LocalClientLiveness::Settling, false),
    );
    let antigravity = antigravity_status_document(
        true,
        &AntigravityIntegration {
            route: ConfiguredAntigravityRoute {
                configured: true,
                launcher_current: true,
                managed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                enabled: true,
            },
            permission_current: true,
            legacy_route_present: false,
        },
        &json!({ "installed": true, "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION, "state": "current" }),
        None,
        false,
        evidence(LocalClientLiveness::Settling, false),
    );
    for (provider, status) in [("Claude", &claude), ("Antigravity", &antigravity)] {
        assert_eq!(status["connectionActive"], false, "{provider}");
        assert_eq!(status["connectionPending"], true, "{provider}");
    }
}

#[test]
fn claude_bundle_contains_current_manifest_and_bridge() {
    let mut archive = zip::ZipArchive::new(Cursor::new(
        claude_desktop_extension().expect("Claude extension archive"),
    ))
    .expect("MCPB zip");
    assert_eq!(archive.len(), 2);
    let manifest: Value = {
        let mut entry = archive.by_name("manifest.json").expect("manifest entry");
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("manifest text");
        serde_json::from_str(&text).expect("manifest JSON")
    };
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest["server"]["entry_point"], "server/index.cjs");
    let mut bridge = String::new();
    archive
        .by_name("server/index.cjs")
        .expect("bridge entry")
        .read_to_string(&mut bridge)
        .expect("bridge text");
    assert!(bridge.contains("local-mcp.json"));
    assert!(bridge.contains(env!("CARGO_PKG_VERSION")));
    assert!(!bridge.contains(VERSION_SLOT));
}

fn archive_text(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, path: &str) -> String {
    let mut text = String::new();
    archive
        .by_name(path)
        .unwrap_or_else(|error| panic!("{path} entry: {error}"))
        .read_to_string(&mut text)
        .unwrap_or_else(|error| panic!("{path} text: {error}"));
    text
}

/// The targets of the relative `[text](target)` links in a Markdown document
/// at `document` (a path relative to the skill root), resolved against that
/// document's directory. URLs, `mailto:` and in-page `#` anchors are skipped,
/// and a target's own `#fragment` is dropped.
fn relative_markdown_link_targets(document: &str, text: &str) -> Vec<String> {
    let base: Vec<&str> = document.split('/').collect();
    let base = &base[..base.len() - 1];
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find(')') else { break };
        let target = rest[..end].trim();
        rest = &rest[end..];
        let target = target.split('#').next().unwrap_or_default();
        if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
            continue;
        }
        let mut parts: Vec<&str> = base.to_vec();
        let mut parts_visited = 0;
        for part in target.split('/') {
            parts_visited += 1;
            match part {
                "" | "." => {}
                ".." => {
                    assert!(
                        parts.pop().is_some(),
                        "{document} links {target}, outside the skill"
                    );
                }
                part => parts.push(part),
            }
        }
        assert!(
            parts_visited > 0,
            "{document} link target has path segments"
        );
        targets.push(parts.join("/"));
    }
    targets
}

#[test]
fn relative_markdown_link_targets_resolve_against_the_document() {
    assert_eq!(
        relative_markdown_link_targets(
            "LICENSE",
            "see [`LICENSE-MIT`](LICENSE-MIT), [terms](LICENSES/T.txt#top), \
             [site](https://example.org/x), [mail](mailto:a@b), [here](#anchor)"
        ),
        ["LICENSE-MIT", "LICENSES/T.txt"]
    );
    assert_eq!(
        relative_markdown_link_targets("LICENSES/T.txt", "[up](../LICENSE) [same](./U.txt)"),
        ["LICENSE", "LICENSES/U.txt"]
    );
}

#[test]
fn bundled_bridges_are_stamped_with_the_running_tool_contract() {
    let expected = crate::local_access::tool_contract_sha256();
    let mut claude = zip::ZipArchive::new(Cursor::new(
        claude_desktop_extension().expect("Claude extension archive"),
    ))
    .expect("MCPB zip");
    let mut generic = zip::ZipArchive::new(Cursor::new(
        local_client_archive().expect("local client archive"),
    ))
    .expect("client zip");
    let mut bridges = vec![(
        "server/index.cjs",
        archive_text(&mut claude, "server/index.cjs"),
    )];
    {
        let path = "claude/server/index.cjs";
        bridges.push((path, archive_text(&mut generic, path)));
    }
    for (path, bridge) in bridges {
        assert!(
            bridge.contains(&expected),
            "{path} must carry the tool contract hash it was built against"
        );
        assert!(
            !bridge.contains(CONTRACT_SLOT),
            "{path} must not ship the unreplaced contract slot"
        );
    }
}

#[test]
fn claude_manifest_satisfies_the_mcpb_required_fields() {
    let manifest: Value =
        serde_json::from_slice(&claude_manifest().expect("manifest")).expect("manifest JSON");
    for field in [
        "manifest_version",
        "name",
        "version",
        "description",
        "author",
        "server",
    ] {
        assert!(manifest.get(field).is_some(), "MCPB requires {field}");
    }
    assert_eq!(manifest["manifest_version"], "0.3");
    // Literal compatibility pins: a branding/version update must replace the
    // existing extension, not install another MCP under a new publisher id.
    assert_eq!(manifest["name"], "qc-remote");
    assert_eq!(manifest["author"]["name"], "Freevia EOOD");
    #[cfg(target_os = "macos")]
    assert_eq!(manifest["compatibility"]["platforms"], json!(["darwin"]));
    #[cfg(target_os = "linux")]
    assert_eq!(manifest["compatibility"]["platforms"], json!(["linux"]));
    #[cfg(target_os = "windows")]
    assert_eq!(manifest["compatibility"]["platforms"], json!(["win32"]));
    assert!(manifest["author"]["name"]
        .as_str()
        .is_some_and(|name| !name.is_empty()));
    let server = &manifest["server"];
    assert!(
        matches!(
            server["type"].as_str(),
            Some("node" | "python" | "binary" | "uv")
        ),
        "unknown MCPB server type {:?}",
        server["type"]
    );
    let entry = server["entry_point"].as_str().expect("entry_point");
    // Claude Desktop ships its own Node runtime and resolves a bare `node`
    // against it, which is what every MCPB example does.
    assert_eq!(server["mcp_config"]["command"], "node");
    // The identity travels on the command line as well as in `env`. A
    // host that retains the configured environment without handing it to
    // the child it spawns leaves this bridge announcing no client at all,
    // and the row can then never report a working extension as connected.
    assert_eq!(
        server["mcp_config"]["args"],
        json!([
            format!("${{__dirname}}/{entry}"),
            CLIENT_ID_ARGUMENT,
            CLAUDE_CLIENT_ID
        ])
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(
        claude_desktop_extension().expect("Claude extension archive"),
    ))
    .expect("MCPB zip");
    assert!(
        archive.by_name(entry).is_ok(),
        "entry_point {entry} is not in the bundle"
    );
}

#[test]
fn the_bundled_bridge_passes_a_node_syntax_check() {
    let mut archive = zip::ZipArchive::new(Cursor::new(
        claude_desktop_extension().expect("Claude extension archive"),
    ))
    .expect("MCPB zip");
    let bridge = archive_text(&mut archive, "server/index.cjs");
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("syntax check directory");
    let file = directory.join("index.cjs");
    fs::write(&file, bridge).expect("bridge copy");
    #[cfg(windows)]
    let checked = run_with_timeout(
        Command::new("node")
            .arg("--check")
            .arg(&file)
            .stderr(std::process::Stdio::piped()),
        SCRIPT_FIXTURE_TIMEOUT,
    )
    .expect("node is required to verify the bundled bridge");
    #[cfg(not(windows))]
    let checked = crate::tests::run_node_syntax_fixture(&file)
        .expect("node is required to verify the bundled bridge");
    assert!(
        checked.status.success(),
        "node --check rejected the bundled bridge ({}): {}",
        checked.status,
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn every_manual_configuration_names_one_installed_bridge() {
    let mut archive = zip::ZipArchive::new(Cursor::new(
        local_client_archive().expect("local client archive"),
    ))
    .expect("client zip");
    let installer = codex_installer();
    assert!(installer.contains(BRIDGE_FILE_NAME));
    #[cfg(target_os = "windows")]
    assert!(
        installer.contains("codex mcp remove qc-remote"),
        "an existing server must come with the command that upgrades it"
    );
    #[cfg(target_os = "macos")]
    {
        assert!(installer.contains("codex mcp add qc-remote"));
        assert!(installer.contains("managed-native"));
        assert!(installer.contains("An MCP server named 'qc-remote' already exists"));
        assert!(installer.contains("codex mcp remove qc-remote"));
    }
    for path in ["codex/mcp-config.example.toml", "vscode/mcp.example.json"] {
        let text = archive_text(&mut archive, path);
        assert!(
            text.contains(BRIDGE_FILE_NAME),
            "{path} must point at the installed bridge"
        );
        assert!(
            !text.contains("index.cjs"),
            "{path} must not name a second bridge copy"
        );
    }
    let antigravity = archive_text(&mut archive, "antigravity/mcp_config.example.json");
    assert!(antigravity.contains("--local-mcp-bridge"));
    #[cfg(target_os = "windows")]
    assert!(antigravity.contains("qc-remote.exe"));
    #[cfg(target_os = "macos")]
    assert!(antigravity.contains("Contents/MacOS/QC Remote"));
    let readme = archive_text(&mut archive, "README.md");
    assert!(
        readme.contains(BRIDGE_FILE_NAME),
        "the README must document the one installed bridge location"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_macos_codex_installer_is_valid_shell() {
    let installer = codex_installer();
    let path =
        std::env::temp_dir().join(format!("qc-remote-codex-install-{}.sh", std::process::id()));
    fs::write(&path, installer).expect("write installer fixture");
    let checked = std::process::Command::new("/bin/sh")
        .arg("-n")
        .arg(&path)
        .output()
        .expect("macOS provides /bin/sh");
    let _ = fs::remove_file(&path);
    assert!(
        checked.status.success(),
        "sh -n rejected the generated Codex installer: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn generic_package_is_self_contained() {
    let mut archive = zip::ZipArchive::new(Cursor::new(
        local_client_archive().expect("local client archive"),
    ))
    .expect("client zip");
    #[cfg(target_os = "windows")]
    let installer_path = "codex/install.ps1";
    #[cfg(target_os = "macos")]
    let installer_path = "codex/install.sh";
    #[cfg(target_os = "linux")]
    let installer_path = "codex/install.ps1";
    for path in [
        "README.md",
        installer_path,
        "codex/mcp-config.example.toml",
        "claude/server/index.cjs",
        "antigravity/mcp_config.example.json",
        "vscode/mcp.example.json",
        "skills/qc-device-knowledge/SKILL.md",
        "skills/qc-device-knowledge/LICENSE",
        "skills/qc-device-knowledge/LICENSES/LicenseRef-QC-Remote-Reference.txt",
        "skills/qc-device-knowledge/.qc-remote-managed.json",
    ] {
        assert!(archive.by_name(path).is_ok(), "missing {path}");
    }
    let terms = archive_text(
        &mut archive,
        "skills/qc-device-knowledge/LICENSES/LicenseRef-QC-Remote-Reference.txt",
    );
    assert!(terms.contains("SPDX identifier: LicenseRef-QC-Remote-Reference"));
    for file in DEVICE_KNOWLEDGE_SKILL_FILES {
        let path = format!("skills/{DEVICE_KNOWLEDGE_SKILL_NAME}/{}", file.path);
        assert!(archive.by_name(&path).is_ok(), "missing {path}");
    }
    // Every relative link in every bundled skill file names a file the archive
    // carries at that path inside the skill, so the set is closed under
    // linking: whatever LICENSE reaches, through any number of links, ships.
    let mut targets = Vec::new();
    let mut links_checked = 0;
    for file in DEVICE_KNOWLEDGE_SKILL_FILES {
        let text = archive_text(
            &mut archive,
            &format!("skills/{DEVICE_KNOWLEDGE_SKILL_NAME}/{}", file.path),
        );
        for target in relative_markdown_link_targets(file.path, &text) {
            links_checked += 1;
            let path = format!("skills/{DEVICE_KNOWLEDGE_SKILL_NAME}/{target}");
            assert!(
                archive.by_name(&path).is_ok(),
                "{} links {target}, which the skill archive does not carry",
                file.path
            );
            targets.push(target);
        }
    }
    assert!(
        links_checked > 0,
        "the bundled skill documents contain relative links"
    );
    for expected in [
        "LICENSES/LicenseRef-QC-Remote-Reference.txt",
        "SECURITY.md",
        "legal/THIRD_PARTY-LICENSE-TEXTS.txt",
    ] {
        assert!(
            targets.iter().any(|target| target == expected),
            "a bundled document links {expected}"
        );
    }
    assert!(archive
        .by_name(&format!(
            "QC-Remote-Claude-{}.mcpb",
            env!("CARGO_PKG_VERSION")
        ))
        .is_ok());
}

#[test]
fn device_knowledge_skill_install_is_versioned_and_backs_up_modifications() {
    assert!(DEVICE_KNOWLEDGE_SKILL_FILES
        .iter()
        .all(|file| safe_skill_relative_path(file.path)));
    let skill_document = std::str::from_utf8(DEVICE_KNOWLEDGE_SKILL_FILES[0].contents)
        .expect("bundled SKILL.md is UTF-8");
    assert!(skill_document.contains(&format!("version: \"{DEVICE_KNOWLEDGE_SKILL_VERSION}\"")));

    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let skill = directory
        .join("codex")
        .join("skills")
        .join(DEVICE_KNOWLEDGE_SKILL_NAME);
    let backups = directory.join("backups");
    assert_eq!(
        device_knowledge_skill_status_at(&skill)["state"],
        "not-installed"
    );

    let first = install_device_knowledge_skill_at(&skill, &backups);
    assert!(first.installed, "install failed: {:?}", first.error);
    assert!(first.backup.is_none(), "nothing was there to back up");
    let installed = device_knowledge_skill_status_at(&skill);
    assert_eq!(installed["state"], "current");
    assert_eq!(
        installed["installedVersion"],
        DEVICE_KNOWLEDGE_SKILL_VERSION
    );
    assert_eq!(
        fs::read(skill.join("SKILL.md")).expect("installed skill"),
        DEVICE_KNOWLEDGE_SKILL_FILES[0].contents
    );

    fs::write(skill.join("SKILL.md"), b"user-modified skill").expect("modify installed skill");
    assert_eq!(
        device_knowledge_skill_status_at(&skill)["state"],
        "modified"
    );
    let repaired = install_device_knowledge_skill_at(&skill, &backups);
    assert!(repaired.installed, "repair failed: {:?}", repaired.error);
    let backup = repaired.backup.expect("existing skill backup");
    assert_eq!(
        fs::read(backup.join("SKILL.md")).expect("backed-up skill"),
        b"user-modified skill"
    );
    assert_eq!(device_knowledge_skill_status_at(&skill)["state"], "current");

    fs::remove_dir_all(directory).expect("remove tone skill fixture");
}

#[test]
fn claude_status_detects_install_and_update_versions() {
    assert_eq!(
        claude_status_document(true, None, observed(false, false))["state"],
        "not-installed"
    );
    assert_eq!(
        claude_status_document(
            true,
            Some(env!("CARGO_PKG_VERSION").into()),
            observed(false, false)
        )["state"],
        "current"
    );
    assert_eq!(
        claude_status_document(true, Some("0.1.1".into()), observed(false, false))["state"],
        "update-available"
    );
    assert_eq!(
        claude_status_document(true, Some("99.0.0".into()), observed(false, false))["state"],
        "newer-installed"
    );
    assert_eq!(
        claude_status_document(false, Some("0.1.1".into()), observed(false, false))["state"],
        "claude-not-installed"
    );
}

#[test]
#[cfg(windows)]
fn claude_path_falls_back_to_the_newest_squirrel_version_directory() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let old = directory.join("AnthropicClaude").join("app-1.9.0");
    let current = directory.join("AnthropicClaude").join("app-1.10.0");
    fs::create_dir_all(&old).expect("old Claude directory");
    fs::create_dir_all(&current).expect("current Claude directory");
    fs::write(old.join("claude.exe"), b"old").expect("old Claude executable");
    fs::write(current.join("claude.exe"), b"current").expect("current Claude executable");

    assert_eq!(
        desktop_app_paths_in(std::slice::from_ref(&directory), &CLAUDE_DESKTOP_SEARCH),
        Some(current.join("claude.exe"))
    );

    fs::remove_dir_all(directory).expect("remove Claude path fixture");
}

#[test]
#[cfg(windows)]
fn desktop_versions_cover_squirrel_and_store_package_layouts() {
    assert_eq!(
        version_from_executable_path(Path::new(
            r"C:\Users\test\AppData\Local\AnthropicClaude\app-1.2.34\claude.exe"
        )),
        Some("1.2.34".into())
    );
}

#[test]
fn package_version_extracts_only_known_package_families() {
    assert_eq!(
        package_version(
            "Claude_1.49585.0.0_x64__pzs8sxrjxfjjc",
            &["Claude_", "Anthropic.Claude_"]
        ),
        Some("1.49585.0.0".into())
    );
    assert_eq!(
        package_version(
            "OpenAI.Codex_26.901.6511.0_x64__2p2nqsd0c76g0",
            &["OpenAI.ChatGPT_", "OpenAI.Codex_"]
        ),
        Some("26.901.6511.0".into())
    );
    assert_eq!(
        package_version("Unrelated_1.0.0_x64__publisher", &["Claude_"]),
        None
    );
}

#[test]
#[cfg(windows)]
fn claude_msix_path_uses_the_package_install_location() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let app = directory.join("app");
    fs::create_dir_all(&app).expect("Claude MSIX app directory");
    fs::write(app.join("claude.exe"), b"Claude").expect("Claude MSIX executable");

    let output = format!("\r\n{}\r\n", directory.display());
    assert_eq!(
        msix_executable_from_install_location(
            output.as_bytes(),
            CLAUDE_DESKTOP_SEARCH.package_executables
        ),
        Some(app.join("claude.exe"))
    );

    fs::remove_dir_all(directory).expect("remove Claude MSIX fixture");
}

#[test]
#[cfg(windows)]
fn chatgpt_msix_path_uses_the_package_install_location() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let app = directory.join("app");
    fs::create_dir_all(&app).expect("ChatGPT MSIX app directory");
    fs::write(app.join("ChatGPT.exe"), b"ChatGPT").expect("ChatGPT MSIX executable");

    let output = format!("\r\n{}\r\n", directory.display());
    assert_eq!(
        msix_executable_from_install_location(
            output.as_bytes(),
            CHATGPT_DESKTOP_SEARCH.package_executables
        ),
        Some(app.join("ChatGPT.exe"))
    );
    fs::remove_file(app.join("ChatGPT.exe")).expect("remove nested executable");
    fs::write(directory.join("ChatGPT.exe"), b"ChatGPT").expect("root ChatGPT MSIX executable");
    assert_eq!(
        msix_executable_from_install_location(
            output.as_bytes(),
            CHATGPT_DESKTOP_SEARCH.package_executables
        ),
        Some(directory.join("ChatGPT.exe"))
    );

    fs::remove_dir_all(directory).expect("remove ChatGPT MSIX fixture");
}

#[test]
fn chatgpt_desktop_search_does_not_launch_codex_cli() {
    assert_eq!(CHATGPT_DESKTOP_SEARCH.executable_names, &["ChatGPT.exe"]);
    assert!(CHATGPT_DESKTOP_SEARCH
        .package_executables
        .contains(&r"app\ChatGPT.exe"));
    assert!(CHATGPT_DESKTOP_SEARCH
        .relative_candidates
        .contains(&r"Codex\Codex.exe"));
}

#[test]
fn chatgpt_package_detection_accepts_supported_package_families() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(directory.join("OpenAI.Codex_test-family")).expect("Codex package fixture");
    assert!(package_directory_contains(
        &directory,
        &["OpenAI.Codex_", "OpenAI.ChatGPT_"]
    ));
    assert!(!package_directory_contains(&directory, &["Claude_"]));
    fs::remove_dir_all(directory).expect("remove package fixture");
}

#[test]
fn chatgpt_skill_read_failure_preserves_the_independent_route_and_actual_evidence() {
    let skill_error = json!({
        "installed": false,
        "availableVersion": "1.0.0",
        "state": "status-error",
        "statusError": "The skill marker could not be read",
    });
    let cases = [
        (env!("CARGO_PKG_VERSION"), "current"),
        ("99.0.0", "newer-installed"),
    ];
    assert_eq!(cases.len(), 2);
    for (version, expected) in cases {
        let mut row = chatgpt_status_document(
            true,
            true,
            true,
            true,
            Some(version.into()),
            None,
            false,
            observed(true, true),
        );
        assert_eq!(
            row["state"], expected,
            "the actual native constructor establishes the route state"
        );
        apply_chatgpt_device_knowledge_skill_status(&mut row, &skill_error);
        assert_eq!(
            row["state"], expected,
            "an unreadable skill is not a mismatched MCP route"
        );
        assert_eq!(row["connectionActive"], true);
        assert_eq!(row["integrationInstalled"], true);
        assert_eq!(row["skillInstalled"], false);
        assert_eq!(row["skillState"], "status-error");
        assert_eq!(row["skillStatusError"], skill_error["statusError"]);

        let known = json!({"installed": true, "installedVersion": "1.0.0", "availableVersion": "1.0.0", "state": "current"});
        apply_chatgpt_device_knowledge_skill_status(&mut row, &known);
        assert_eq!(row["state"], expected);
        assert_eq!(row["skillInstalled"], true);
        assert_eq!(row["installedSkillVersion"], "1.0.0");
        assert!(row["skillStatusError"].is_null());
    }

    let states = [
        (false, true, None, "not-installed"),
        (true, false, None, "different-configuration"),
        (
            true,
            true,
            Some("The MCP config could not be read".to_owned()),
            "status-error",
        ),
    ];
    assert_eq!(states.len(), 3);
    for (configured, launcher, error, expected) in states {
        let mut row = chatgpt_status_document(
            true,
            configured,
            launcher,
            true,
            None,
            error,
            false,
            observed(false, false),
        );
        assert_eq!(row["state"], expected);
        apply_chatgpt_device_knowledge_skill_status(&mut row, &skill_error);
        assert_eq!(
            row["state"], expected,
            "skill health cannot improve an unpaired, foreign or unreadable route"
        );
        assert_eq!(row["skillStatusError"], skill_error["statusError"]);
    }
}

#[test]
fn chatgpt_status_distinguishes_setup_update_and_mismatched_routes() {
    assert_eq!(
        chatgpt_status_document(
            false,
            false,
            false,
            false,
            None,
            None,
            false,
            observed(false, false)
        )["state"],
        "chatgpt-not-installed"
    );
    assert_eq!(
        chatgpt_status_document(
            true,
            false,
            false,
            false,
            None,
            None,
            false,
            observed(false, false)
        )["state"],
        "not-installed"
    );
    assert_eq!(
        chatgpt_status_document(
            true,
            true,
            false,
            false,
            None,
            None,
            false,
            observed(false, false)
        )["state"],
        "different-configuration"
    );
    assert_eq!(
        chatgpt_status_document(
            true,
            true,
            true,
            false,
            Some(env!("CARGO_PKG_VERSION").into()),
            None,
            false,
            observed(false, false)
        )["state"],
        "update-available"
    );
    assert_eq!(
        chatgpt_status_document(
            true,
            true,
            true,
            true,
            Some(env!("CARGO_PKG_VERSION").into()),
            None,
            false,
            observed(true, true)
        )["state"],
        "current"
    );
    assert_eq!(
        chatgpt_status_document(
            true,
            true,
            true,
            true,
            Some("0.1.1".into()),
            None,
            false,
            observed(false, false)
        )["state"],
        "update-available"
    );
    assert_eq!(
        chatgpt_status_document(
            true,
            true,
            true,
            true,
            Some("99.0.0".into()),
            None,
            false,
            observed(false, false)
        )["state"],
        "newer-installed"
    );
}

#[test]
fn chatgpt_install_preserves_unrelated_codex_configuration() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("ChatGPT config directory");
    let config = directory.join("config.toml");
    let executable = directory.join("QC Remote.exe");
    let original = "model = \"gpt-test\"\n\n[mcp_servers.other]\ncommand = \"other-server\"\n";
    fs::write(&config, original).expect("seed ChatGPT configuration");

    write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("install ChatGPT route");
    let updated = fs::read_to_string(&config).expect("updated ChatGPT configuration");
    let document = updated.parse::<DocumentMut>().expect("valid TOML");
    assert_eq!(document["model"].as_str(), Some("gpt-test"));
    assert_eq!(
        document["mcp_servers"]["other"]["command"].as_str(),
        Some("other-server")
    );
    assert_eq!(
        document["mcp_servers"][CLAUDE_EXTENSION_NAME]["command"].as_str(),
        Some(executable.to_string_lossy().as_ref())
    );
    assert!(document["mcp_servers"][CLAUDE_EXTENSION_NAME]["args"]
        .as_array()
        .is_some_and(|args| args
            .iter()
            .any(|argument| argument.as_str() == Some("--local-mcp-bridge"))));
    assert_eq!(
        document["mcp_servers"][CLAUDE_EXTENSION_NAME]["default_tools_approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        configured_chatgpt_route(&config, &executable).expect("read managed route"),
        ConfiguredChatgptRoute {
            configured: true,
            launcher_current: true,
            approval_mode_current: true,
            managed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }
    );

    let first_backup = config_backups(&config);
    assert_eq!(first_backup.len(), 1, "one backup after the first pair");
    assert_eq!(
        fs::read_to_string(&first_backup[0].1).expect("configuration backup"),
        original
    );

    let previous_generation = fs::read_to_string(&config).expect("first generation");
    write_codex_config(&config, &executable, CODEX_CLIENT_ID)
        .expect("replace ChatGPT route safely");
    let second_backup = config_backups(&config);
    assert_eq!(second_backup.len(), 2, "the second pair adds a backup");
    assert_eq!(
        fs::read_to_string(&second_backup[0].1).expect("original backup"),
        original,
        "the second pair must not overwrite the user's own configuration"
    );
    assert_eq!(
        fs::read_to_string(&second_backup[1].1).expect("rotated backup"),
        previous_generation
    );
    assert!(
        staged_leftovers(&directory).is_empty(),
        "no staged file outlives a write that landed"
    );

    fs::remove_dir_all(directory).expect("remove ChatGPT config fixture");
}

/// The single rotating `config.toml.qc-remote-backup` was rewritten by every
/// pair and by the unpair, so after pair, pair, unpair nothing QC Remote had
/// written still held the configuration the user started with.
#[test]
fn repeated_chatgpt_pairing_never_loses_the_original_configuration() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("ChatGPT config directory");
    let config = directory.join("config.toml");
    let executable = directory.join("QC Remote.exe");
    let original = "model = \"the-user-wrote-this\"\n";
    fs::write(&config, original).expect("seed ChatGPT configuration");

    write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("first pair");
    write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("second pair");
    assert!(remove_codex_route(&config).expect("unpair"));

    let backups = config_backups(&config);
    assert_eq!(backups.len(), 3, "pair, pair and unpair each keep a copy");
    assert_eq!(
        fs::read_to_string(&backups[0].1).expect("original backup"),
        original,
        "the oldest backup is the configuration QC Remote first found"
    );
    assert!(
        backups.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "backups carry distinct, increasing timestamps: {backups:?}"
    );

    // Well past the cap: the history stays bounded, and the pristine copy
    // is the one thing pruning never takes.
    for _ in 0..CONFIG_BACKUP_HISTORY + 4 {
        write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("repeated pair");
    }
    let pruned = config_backups(&config);
    assert_eq!(pruned.len(), CONFIG_BACKUP_HISTORY);
    assert_eq!(
        fs::read_to_string(&pruned[0].1).expect("surviving original backup"),
        original
    );

    fs::remove_dir_all(directory).expect("remove ChatGPT backup fixture");
}

/// Whatever goes wrong, the vendor's live configuration has to still be
/// there and still be theirs.  The test names the staged file in advance,
/// so a directory in its place fails the write deterministically.
#[test]
fn chatgpt_install_keeps_the_live_config_when_staging_fails() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("ChatGPT config directory");
    let config = directory.join("config.toml");
    let executable = directory.join("QC Remote.exe");
    let original = "model = \"keep-me\"\n";
    fs::write(&config, original).expect("seed ChatGPT configuration");
    let pinned = PinnedStageSequence::new();
    let staged = staged_config_path(&directory, &config, "qc-remote");
    fs::create_dir(&staged).expect("blocking staged directory");

    assert!(write_codex_config(&config, &executable, CODEX_CLIENT_ID).is_err());
    drop(pinned);
    assert_eq!(
        fs::read_to_string(&config).expect("preserved live configuration"),
        original
    );
    assert!(
        config_backups(&config).is_empty(),
        "a write that never started must not leave a backup behind"
    );

    fs::remove_dir_all(directory).expect("remove ChatGPT config failure fixture");
}

#[test]
fn chatgpt_remove_preserves_unrelated_codex_configuration() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("ChatGPT config directory");
    let config = directory.join("config.toml");
    let executable = directory.join("QC Remote.exe");
    fs::write(
        &config,
        "model = \"gpt-test\"\n\n[mcp_servers.other]\ncommand = \"other-server\"\n",
    )
    .expect("seed ChatGPT configuration");
    write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("install ChatGPT route");
    let paired = fs::read_to_string(&config).expect("paired ChatGPT configuration");

    assert!(remove_codex_route(&config).expect("remove managed route"));
    let updated = fs::read_to_string(&config).expect("updated ChatGPT configuration");
    let document = updated.parse::<DocumentMut>().expect("valid TOML");
    assert_eq!(document["model"].as_str(), Some("gpt-test"));
    assert_eq!(
        document["mcp_servers"]["other"]["command"].as_str(),
        Some("other-server")
    );
    assert!(document["mcp_servers"].get(CLAUDE_EXTENSION_NAME).is_none());
    let backups = config_backups(&config);
    assert_eq!(
        fs::read_to_string(&backups.last().expect("unpair backup").1).expect("backup contents"),
        paired,
        "unpair backs up exactly what it is about to replace"
    );
    assert!(!remove_codex_route(&config).expect("idempotent removal"));

    fs::remove_dir_all(directory).expect("remove ChatGPT config fixture");
}

#[test]
fn claude_registry_version_comes_from_the_qc_remote_manifest() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("test directory");
    let registry = directory.join("extensions-installations.json");
    fs::write(
            &registry,
            br#"{"extensions":{"unrelated":{"version":"8.0.0","manifest":{"name":"other"}},"generated-id":{"version":"1.2.3","manifest":{"name":"qc-remote"}}}}"#,
        )
        .expect("test registry");
    assert_eq!(
        installed_claude_extension_version(&registry)
            .expect("read Claude extension registry")
            .as_deref(),
        Some("1.2.3")
    );
    fs::remove_file(registry).expect("remove test registry");
    fs::remove_dir(directory).expect("remove test directory");
}

#[test]
fn claude_registry_paths_cover_classic_and_store_installations() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let roaming = directory.join("Roaming");
    let local = directory.join("Local");
    let store = local.join("Packages").join("Claude_pzs8sxrjxfjjc");
    let unrelated = local.join("Packages").join("NotClaude_example");
    fs::create_dir_all(&store).expect("Store package directory");
    fs::create_dir_all(&unrelated).expect("unrelated package directory");

    let roots = claude_user_data_roots_in(Some(&roaming), Some(&local));
    let paths = claude_extension_registry_paths_in(&roots);
    assert!(paths.contains(&roaming.join("Claude").join("extensions-installations.json")));
    assert!(paths.contains(
        &store
            .join("LocalCache")
            .join("Roaming")
            .join("Claude")
            .join("extensions-installations.json")
    ));
    assert!(paths.iter().all(|path| !path.starts_with(&unrelated)));
    assert!(roots.contains(&store.join("LocalCache").join("Roaming").join("Claude")));

    fs::remove_dir_all(directory).expect("remove registry paths fixture");
}

#[test]
fn claude_registry_uses_newest_install_across_classic_and_store_paths() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("test directory");
    let classic = directory.join("classic.json");
    let store = directory.join("store.json");
    fs::write(
        &classic,
        br#"{"extensions":{"classic":{"version":"0.1.100","manifest":{"name":"qc-remote"}}}}"#,
    )
    .expect("classic registry");
    fs::write(
        &store,
        br#"{"extensions":{"store":{"version":"0.1.112","manifest":{"name":"qc-remote"}}}}"#,
    )
    .expect("Store registry");

    assert_eq!(
        installed_claude_extension_version_from_paths(&[classic, store], &[])
            .expect("read all Claude registries")
            .as_deref(),
        Some("0.1.112")
    );

    fs::remove_dir_all(directory).expect("remove registry versions fixture");
}

#[test]
fn claude_install_is_detected_from_its_manifest_before_registry_refresh() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let extension = directory
        .join("Claude Extensions")
        .join("local.mcpb.freevia.qc-remote");
    fs::create_dir_all(&extension).expect("extension directory");
    fs::write(
        extension.join("manifest.json"),
        br#"{"name":"qc-remote","version":"0.1.112"}"#,
    )
    .expect("extension manifest");

    let manifests = claude_extension_manifest_paths_in(std::slice::from_ref(&directory));
    assert_eq!(
        installed_claude_extension_version_from_paths(&[], &manifests)
            .expect("read extension manifest")
            .as_deref(),
        Some("0.1.112")
    );

    fs::remove_dir_all(directory).expect("remove extension manifest fixture");
}

#[test]
#[cfg(windows)]
fn antigravity_paths_cover_per_user_and_machine_installations() {
    let local = Path::new(r"C:\Users\test\AppData\Local");
    let program_w6432 = Path::new(r"C:\Program Files");
    let program_files_x86 = Path::new(r"C:\Program Files (x86)");
    let roots = installation_roots_from(
        Some(local),
        Some(program_w6432),
        // The 64-bit process case: `%ProgramFiles%` repeats
        // `%ProgramW6432%`, and a repeated root must not become a repeated
        // search.
        Some(program_w6432),
        Some(program_files_x86),
    );
    assert_eq!(
        roots,
        vec![
            local.join("Programs"),
            local.to_path_buf(),
            program_w6432.to_path_buf(),
            program_files_x86.to_path_buf(),
        ]
    );
    let candidates = roots
        .iter()
        .flat_map(|root| {
            ANTIGRAVITY_SEARCH
                .relative_candidates
                .iter()
                .map(|relative| root.join(relative))
        })
        .collect::<Vec<_>>();
    for expected in [
        local
            .join("Programs")
            .join("Antigravity")
            .join("Antigravity.exe"),
        local
            .join("Programs")
            .join("Google")
            .join("Antigravity")
            .join("Antigravity.exe"),
        local.join("Antigravity").join("Antigravity.exe"),
        program_w6432.join("Antigravity").join("Antigravity.exe"),
        program_files_x86
            .join("Google")
            .join("Antigravity")
            .join("Antigravity.exe"),
    ] {
        assert!(
            candidates.contains(&expected),
            "{} is not searched",
            expected.display()
        );
    }
}

#[test]
fn antigravity_versioned_installation_chooses_the_newest_executable() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    for version in ["2.9.0", "2.12.2"] {
        let app = directory.join(format!("app-{version}"));
        fs::create_dir_all(&app).expect("Antigravity version directory");
        fs::write(app.join("Antigravity.exe"), []).expect("Antigravity fixture executable");
    }
    assert_eq!(
        newest_versioned_sibling(&directory.join("Antigravity.exe")),
        Some(directory.join("app-2.12.2").join("Antigravity.exe"))
    );
    fs::remove_dir_all(directory).expect("remove Antigravity version fixture");
}

/// A scratch directory named after the test that owns it, so the suite can
/// run its cases in parallel without them colliding.
fn installation_fixture(name: &str) -> PathBuf {
    crate::tests::unique_fixture_directory(&format!("client-installation-{name}"))
}

fn executable_at(path: &Path) -> PathBuf {
    fs::create_dir_all(path.parent().expect("fixture parent"))
        .expect("fixture executable directory");
    fs::write(path, []).expect("fixture executable");
    path.to_path_buf()
}

#[test]
#[cfg(windows)]
fn an_uninstall_entry_resolves_a_quoted_install_location_and_display_icon() {
    // Both values arrive with the quotation marks inside them, which is
    // how Jan registers itself.
    let directory = installation_fixture("quoted-entry");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let installed = directory.join("Programs").join("Jan");
    let executable = executable_at(&installed.join("jan.exe"));

    let from_icon = UninstallEntry {
        display_icon: Some(format!("\"{}\"", executable.display())),
        ..Default::default()
    };
    assert_eq!(
        uninstall_entry_executable(&from_icon, JAN_SEARCH.executable_names),
        Some(executable.clone()),
        "a quoted DisplayIcon names the executable itself"
    );

    let from_location = UninstallEntry {
        install_location: Some(format!("\"{}\"", installed.display())),
        ..Default::default()
    };
    assert_eq!(
        uninstall_entry_executable(&from_location, JAN_SEARCH.executable_names),
        Some(installed.join("Jan.exe")),
        "a quoted InstallLocation is joined with the declared executable name"
    );

    fs::remove_dir_all(directory).expect("remove quoted entry fixture");
}

#[test]
fn an_uninstall_entry_resolves_a_display_icon_carrying_an_icon_index() {
    let directory = installation_fixture("icon-index");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let executable = executable_at(&directory.join("Bionic").join("Bionic.exe"));
    let icon = directory.join("Bionic").join("Bionic.ico");
    fs::write(&icon, []).expect("fixture icon");

    for spelling in [
        format!("{},0", executable.display()),
        format!("\"{}\",0", executable.display()),
        format!("\"{},0\"", executable.display()),
        format!("{}, 12", executable.display()),
    ] {
        let entry = UninstallEntry {
            display_icon: Some(spelling.clone()),
            ..Default::default()
        };
        assert_eq!(
            uninstall_entry_executable(&entry, LM_STUDIO_SEARCH.executable_names),
            Some(executable.clone()),
            "{spelling} should resolve to the executable"
        );
    }

    // An icon index on an `.ico` leaves the directory as the only usable
    // part of the value, and the executable is still found beside it.
    let icon_entry = UninstallEntry {
        display_icon: Some(format!("\"{}\",0", icon.display())),
        ..Default::default()
    };
    assert_eq!(
        uninstall_entry_executable(&icon_entry, LM_STUDIO_SEARCH.executable_names),
        Some(executable),
    );

    fs::remove_dir_all(directory).expect("remove icon index fixture");
}

#[test]
fn an_uninstall_entry_whose_paths_are_gone_is_not_an_installation() {
    let directory = installation_fixture("stale-entry");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let missing = directory.join("Programs").join("Jan");
    let entry = UninstallEntry {
        display_version: Some("0.8.4".into()),
        install_location: Some(format!("\"{}\"", missing.display())),
        display_icon: Some(format!("\"{}\"", missing.join("jan.exe").display())),
    };
    assert_eq!(
        uninstall_entry_executable(&entry, JAN_SEARCH.executable_names),
        None,
        "an uninstaller that left its key behind must not be reported as installed"
    );

    // The very same entry answers once the file it names is there, so the
    // case above fails for the absence and not for the shape of the value.
    let executable = executable_at(&missing.join("jan.exe"));
    assert_eq!(
        uninstall_entry_executable(&entry, JAN_SEARCH.executable_names),
        Some(executable)
    );

    fs::remove_dir_all(directory).expect("remove stale entry fixture");
}

#[test]
fn a_relative_registry_path_is_never_searched() {
    // A relative value would resolve against this process's working
    // directory, so `DisplayIcon` of a bare file name must not be able to
    // match something sitting next to QC Remote.
    let entry = UninstallEntry {
        display_icon: Some("jan.exe".into()),
        install_location: Some(".".into()),
        ..Default::default()
    };
    assert_eq!(
        uninstall_entry_executable(&entry, JAN_SEARCH.executable_names),
        None
    );
}

#[test]
fn an_install_record_is_used_only_when_it_names_a_file_that_is_there() {
    let directory = installation_fixture("self-record");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let executable = executable_at(&directory.join("Programs").join("Bionic").join("Bionic.exe"));
    let record = directory.join(LM_STUDIO_SEARCH.self_records[0].relative_path);
    fs::create_dir_all(record.parent().expect("record parent")).expect("record directory");

    let write = |contents: &str| fs::write(&record, contents).expect("install record");

    write(&format!(
        "{{\"path\":{},\"argv\":[],\"cwd\":{}}}",
        serde_json::to_string(&executable).expect("serialize path"),
        serde_json::to_string(executable.parent().expect("record cwd")).expect("serialize cwd")
    ));
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        Some(executable.clone()),
        "the app's own record of where it installed itself is authoritative"
    );

    write("{\"path\": not json");
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        None,
        "a malformed record is untrusted input, not an installation"
    );

    write("{\"cwd\":\"C:\\\\somewhere\"}");
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        None,
        "a record without the member that holds the path says nothing"
    );

    write("{\"path\":[\"Bionic.exe\"]}");
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        None,
        "a path that is not a string says nothing"
    );

    write("{\"path\":\"Programs\\\\Bionic\\\\Bionic.exe\"}");
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        None,
        "a relative path would resolve against this process, not the app"
    );

    write(&format!(
        "{{\"path\":{}}}",
        serde_json::to_string(&executable.with_file_name("Gone.exe")).expect("serialize path")
    ));
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        None,
        "a record naming a file that is not there is not an installation"
    );

    fs::remove_file(&record).expect("remove install record");
    assert_eq!(
        self_recorded_path(&directory, LM_STUDIO_SEARCH.self_records),
        None,
        "an absent record is not an error"
    );

    fs::remove_dir_all(directory).expect("remove self record fixture");
}

#[test]
#[cfg(windows)]
fn lm_studio_resolves_from_bionic_and_from_its_legacy_layout() {
    let directory = installation_fixture("lm-studio-layouts");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let bionic = executable_at(&directory.join("Bionic").join("Bionic.exe"));
    assert_eq!(
        desktop_app_paths_in(std::slice::from_ref(&directory), &LM_STUDIO_SEARCH),
        Some(bionic.clone()),
        "the renamed application is found where its installer now puts it"
    );

    fs::remove_dir_all(directory.join("Bionic")).expect("remove Bionic layout");
    let legacy = executable_at(&directory.join("LM Studio").join("LM Studio.exe"));
    assert_eq!(
        desktop_app_paths_in(std::slice::from_ref(&directory), &LM_STUDIO_SEARCH),
        Some(legacy),
        "an installation made before the rename still resolves"
    );

    assert_eq!(
        desktop_app_paths_in(std::slice::from_ref(&directory), &JAN_SEARCH),
        None,
        "and neither layout answers for a different provider"
    );

    fs::remove_dir_all(directory).expect("remove LM Studio layout fixture");
}

#[test]
fn a_store_execution_alias_is_accepted_although_it_is_zero_length() {
    // A Store alias is a zero-length AppExecLink reparse point:
    // `is_file()` is true, `metadata()` succeeds with length 0. So the
    // probe must accept an empty file, and this case fails the moment
    // anybody adds a "and is not empty" clause to it.
    let directory = installation_fixture("store-alias");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let alias = executable_at(
        &directory
            .join("Microsoft")
            .join("WindowsApps")
            .join("Bionic.exe"),
    );
    assert_eq!(
        fs::metadata(&alias).expect("alias metadata").len(),
        0,
        "the fixture has to be empty for this to be evidence"
    );
    assert_eq!(
        store_alias_path(&directory, LM_STUDIO_SEARCH.executable_names),
        Some(alias)
    );
    assert_eq!(
        store_alias_path(&directory, JAN_SEARCH.executable_names),
        None
    );
    fs::remove_dir_all(directory).expect("remove store alias fixture");
}

#[test]
#[cfg(windows)]
fn a_default_path_answers_before_the_slower_mechanisms() {
    // Both mechanisms hold a real file at once, which is the state Bionic
    // is actually in, so the returned path alone cannot say which was
    // used - the mechanism reports it.
    static SEARCH: DesktopAppSearch = DesktopAppSearch {
        // A name nothing on any machine can have, so `App Paths`, `PATH`,
        // the Uninstall hives and the Store alias directory cannot answer
        // and the case stays about the first two steps.
        executable_names: &["qc-remote-detection-probe.exe"],
        relative_candidates: &[r"Probe\qc-remote-detection-probe.exe"],
        display_names: &[],
        self_records: &[SelfInstallRecord {
            relative_path: "probe-install-location.json",
            member: "path",
        }],
        package_prefixes: &[],
        package_executables: &[],
    };
    let directory = installation_fixture("mechanism-order");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let default_path = executable_at(
        &directory
            .join("Probe")
            .join("qc-remote-detection-probe.exe"),
    );
    let recorded = executable_at(
        &directory
            .join("Recorded")
            .join("qc-remote-detection-probe.exe"),
    );
    fs::write(
        directory.join("probe-install-location.json"),
        format!(
            "{{\"path\":{}}}",
            serde_json::to_string(&recorded).expect("serialize recorded path")
        ),
    )
    .expect("probe install record");

    assert_eq!(
        resolve_desktop_app_in(
            std::slice::from_ref(&directory),
            Some(&directory),
            Some(&directory),
            &SEARCH
        ),
        Some((
            InstallationMechanism::DefaultPath,
            DesktopInstallation {
                executable: Some(default_path.clone()),
                version: None,
            }
        )),
        "a direct path hit must not pay for the mechanisms behind it"
    );

    fs::remove_dir_all(directory.join("Probe")).expect("remove default path");
    assert_eq!(
        resolve_desktop_app_in(
            std::slice::from_ref(&directory),
            Some(&directory),
            Some(&directory),
            &SEARCH
        ),
        Some((
            InstallationMechanism::SelfRecord,
            DesktopInstallation {
                executable: Some(recorded),
                version: None,
            }
        )),
        "and the record behind it answers once the direct path is gone"
    );

    fs::remove_dir_all(directory).expect("remove mechanism order fixture");
}

#[test]
fn uninstall_entries_are_read_out_of_the_registry_dump_they_come_in() {
    // The shape `reg.exe` prints: a key line, then its values indented and
    // separated from their type by a run of spaces a DisplayName may
    // itself contain.
    let index = uninstall_display_names(concat!(
        "\r\n",
        "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Jan\r\n",
        "    DisplayName    REG_SZ    Jan\r\n",
        "\r\n",
        "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{a-b-c}\r\n",
        "    DisplayNameLocalized    REG_SZ    Not a display name\r\n",
        "    DisplayName    REG_SZ    LM  Studio  2\r\n",
        "\r\n",
    ));
    assert_eq!(
        index,
        vec![
            (
                r"HKEY_CURRENT_USER\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Jan"
                    .to_string(),
                "Jan".to_string()
            ),
            (
                r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{a-b-c}"
                    .to_string(),
                "LM  Studio  2".to_string()
            ),
        ]
    );

    let entry = uninstall_entry_from_key_dump(concat!(
        "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Jan\r\n",
        "    MainBinaryName    REG_SZ    jan.exe\r\n",
        "    DisplayName    REG_SZ    Jan\r\n",
        "    DisplayIcon    REG_SZ    \"C:\\Programs\\Jan\\jan.exe\"\r\n",
        "    DisplayVersion    REG_SZ    0.8.4\r\n",
        "    InstallLocation    REG_SZ    \"C:\\Programs\\Jan\"\r\n",
        "    NoModify    REG_DWORD    0x1\r\n",
    ));
    assert_eq!(
        entry,
        UninstallEntry {
            display_version: Some("0.8.4".into()),
            install_location: Some(r#""C:\Programs\Jan""#.into()),
            display_icon: Some(r#""C:\Programs\Jan\jan.exe""#.into()),
        }
    );
    assert_eq!(
        unwrap_registry_path(&entry.install_location.expect("install location")),
        Some(r"C:\Programs\Jan".to_string())
    );
}

#[test]
fn a_path_field_that_only_looks_like_an_icon_index_is_left_alone() {
    assert_eq!(
        unwrap_registry_path(r"C:\Program Files\Foo, Inc\app.exe"),
        Some(r"C:\Program Files\Foo, Inc\app.exe".to_string()),
        "a comma that is not followed by digits is part of the path"
    );
    assert_eq!(unwrap_registry_path("  \"\"  "), None);
    assert_eq!(unwrap_registry_path("   "), None);
}

#[test]
fn antigravity_pair_and_unpair_preserve_unrelated_mcp_servers() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Antigravity config directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("QC Remote.exe");
    let original = json!({
        "mcpServers": {
            "other": { "command": "other-server" }
        },
        "custom": { "preserved": true }
    });
    fs::write(
        &config,
        serde_json::to_vec_pretty(&original).expect("serialize fixture"),
    )
    .expect("seed Antigravity configuration");

    write_antigravity_config(&config, &executable, ANTIGRAVITY_CLIENT_ID)
        .expect("pair Antigravity");
    fs::write(&executable, b"executable fixture").expect("write Antigravity executable");
    let paired: Value =
        serde_json::from_slice(&fs::read(&config).expect("paired Antigravity configuration"))
            .expect("valid paired configuration");
    assert_eq!(paired["custom"]["preserved"], true);
    assert_eq!(paired["mcpServers"]["other"]["command"], "other-server");
    assert_eq!(
        paired["mcpServers"][CLAUDE_EXTENSION_NAME]["command"],
        executable.to_string_lossy().as_ref()
    );
    assert_eq!(
        paired["mcpServers"][CLAUDE_EXTENSION_NAME]["args"],
        json!([
            "--local-mcp-bridge",
            "--local-mcp-client",
            ANTIGRAVITY_CLIENT_ID
        ])
    );
    assert_eq!(
        paired["mcpServers"][CLAUDE_EXTENSION_NAME]["env"][LOCAL_MCP_CLIENT_ENV],
        ANTIGRAVITY_CLIENT_ID
    );
    assert_eq!(
        configured_antigravity_route(&config, &executable).expect("inspect Antigravity pairing"),
        ConfiguredAntigravityRoute {
            configured: true,
            launcher_current: true,
            managed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            enabled: true,
        }
    );
    let backups = config_backups(&config);
    assert_eq!(backups.len(), 1, "pairing keeps one copy of what it found");
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(&backups[0].1).expect("Antigravity configuration backup")
        )
        .expect("valid backup JSON"),
        original
    );

    assert!(remove_antigravity_route(&config).expect("unpair Antigravity"));
    let unpaired: Value =
        serde_json::from_slice(&fs::read(&config).expect("unpaired Antigravity configuration"))
            .expect("valid unpaired configuration");
    assert_eq!(unpaired["custom"]["preserved"], true);
    assert_eq!(unpaired["mcpServers"]["other"]["command"], "other-server");
    assert!(unpaired["mcpServers"].get(CLAUDE_EXTENSION_NAME).is_none());
    assert!(!remove_antigravity_route(&config).expect("idempotent unpair"));

    fs::remove_dir_all(directory).expect("remove Antigravity config fixture");
}

#[test]
fn antigravity_ready_state_requires_the_current_qc_remote_launcher() {
    let executable = Path::new(r"C:\Users\test\AppData\Local\QC Remote\qc-remote.exe");
    let ready = json!({
        "states": [{
            "spec": {
                "serverName": "qc-remote",
                "command": executable,
                "args": ["--local-mcp-bridge", "--local-mcp-client", ANTIGRAVITY_CLIENT_ID]
            },
            "status": "MCP_SERVER_STATUS_READY"
        }]
    });
    assert!(antigravity_state_has_current_qc_remote(&ready, executable));

    let mut stale = ready.clone();
    stale["states"][0]["spec"]["command"] = Value::String(r"C:\stale\qc-remote.exe".into());
    assert!(!antigravity_state_has_current_qc_remote(&stale, executable));

    let mut failed = ready;
    failed["states"][0]["status"] = Value::String("MCP_SERVER_STATUS_ERROR".into());
    assert!(!antigravity_state_has_current_qc_remote(
        &failed, executable
    ));
}

#[test]
fn antigravity_empty_config_is_available_and_pairable() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Antigravity config directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("QC Remote.exe");
    fs::write(&config, "\r\n  ").expect("seed empty Antigravity configuration");

    assert_eq!(
        configured_antigravity_route(&config, &executable)
            .expect("inspect empty Antigravity configuration"),
        ConfiguredAntigravityRoute {
            configured: false,
            launcher_current: false,
            managed_version: None,
            enabled: true,
        }
    );

    write_antigravity_config(&config, &executable, ANTIGRAVITY_CLIENT_ID)
        .expect("pair from empty Antigravity configuration");
    fs::write(&executable, b"executable fixture").expect("write Antigravity executable");
    assert_eq!(
        configured_antigravity_route(&config, &executable)
            .expect("inspect paired Antigravity configuration"),
        ConfiguredAntigravityRoute {
            configured: true,
            launcher_current: true,
            managed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            enabled: true,
        }
    );

    fs::remove_dir_all(directory).expect("remove Antigravity empty config fixture");
}

#[test]
fn antigravity_older_managed_bridge_is_an_update() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Antigravity config directory");
    let config = directory.join("mcp_config.json");
    let current = directory.join(format!(
        "{ANTIGRAVITY_BRIDGE_PREFIX}{}.exe",
        env!("CARGO_PKG_VERSION")
    ));
    let older = directory.join(format!("{ANTIGRAVITY_BRIDGE_PREFIX}0.1.136.exe"));
    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                "qc-remote": {
                    "command": older,
                    "args": ["--local-mcp-bridge"],
                    "env": { "QC_REMOTE_LOCAL_MCP_CLIENT": "antigravity:0.1.136" }
                }
            }
        }))
        .expect("serialize Antigravity fixture"),
    )
    .expect("write Antigravity fixture");
    let integration = AntigravityIntegration {
        route: configured_antigravity_route(&config, &current).expect("inspect older pairing"),
        permission_current: true,
        legacy_route_present: false,
    };
    let current_skill = json!({
        "installed": true,
        "installedVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "state": "current",
    });
    let status = antigravity_status_document(
        true,
        &integration,
        &current_skill,
        None,
        false,
        observed(false, false),
    );
    assert_eq!(
        integration.route.managed_version.as_deref(),
        Some("0.1.136")
    );
    assert_eq!(status["state"], "update-available");
    assert_eq!(status["installedVersion"], "0.1.136");

    fs::remove_dir_all(directory).expect("remove Antigravity update fixture");
}

#[test]
fn antigravity_stable_launcher_with_older_client_is_an_update() {
    // Every release writes the same command now, so the route's version
    // has to come from its own client identity or an older pairing reads
    // as current and is never offered an update.
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Antigravity stable launcher directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("qc-remote.exe");
    fs::write(&executable, b"executable fixture").expect("write stable launcher fixture");
    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                "qc-remote": {
                    "command": executable,
                    "args": ["--local-mcp-bridge"],
                    "env": { "QC_REMOTE_LOCAL_MCP_CLIENT": "antigravity:0.1.140" }
                }
            }
        }))
        .expect("serialize stable launcher fixture"),
    )
    .expect("write stable launcher fixture");

    let route =
        configured_antigravity_route(&config, &executable).expect("inspect stable launcher");
    assert!(route.configured);
    assert!(!route.launcher_current);
    assert_eq!(route.managed_version.as_deref(), Some("0.1.140"));

    fs::remove_dir_all(directory).expect("remove stable launcher fixture");
}

#[test]
fn antigravity_missing_current_bridge_requires_repair() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Antigravity config directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join(format!(
        "{ANTIGRAVITY_BRIDGE_PREFIX}{}.exe",
        env!("CARGO_PKG_VERSION")
    ));
    write_antigravity_config(&config, &executable, ANTIGRAVITY_CLIENT_ID)
        .expect("write Antigravity route");

    assert_eq!(
        configured_antigravity_route(&config, &executable)
            .expect("inspect missing Antigravity bridge"),
        ConfiguredAntigravityRoute {
            configured: true,
            launcher_current: false,
            managed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            enabled: true,
        }
    );

    fs::remove_dir_all(directory).expect("remove missing bridge fixture");
}

#[test]
fn antigravity_status_distinguishes_unavailable_available_and_connected() {
    let current_skill = json!({
        "installed": true,
        "installedVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "state": "current",
    });
    let paired = || ConfiguredAntigravityRoute {
        configured: true,
        launcher_current: true,
        managed_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        enabled: true,
    };
    let unconfigured = AntigravityIntegration {
        route: ConfiguredAntigravityRoute {
            configured: false,
            launcher_current: false,
            managed_version: None,
            enabled: true,
        },
        permission_current: false,
        legacy_route_present: false,
    };
    let connected = AntigravityIntegration {
        route: paired(),
        permission_current: true,
        legacy_route_present: false,
    };
    let permission_missing = AntigravityIntegration {
        route: paired(),
        permission_current: false,
        legacy_route_present: false,
    };
    let legacy_route_left_behind = AntigravityIntegration {
        route: paired(),
        permission_current: true,
        legacy_route_present: true,
    };
    assert_eq!(
        antigravity_status_document(
            false,
            &unconfigured,
            &current_skill,
            None,
            false,
            observed(false, false)
        )["state"],
        "antigravity-not-installed"
    );
    assert_eq!(
        antigravity_status_document(
            true,
            &unconfigured,
            &current_skill,
            None,
            false,
            observed(false, false)
        )["state"],
        "not-installed"
    );
    assert_eq!(
        antigravity_status_document(
            true,
            &connected,
            &current_skill,
            None,
            true,
            observed(true, true)
        )["state"],
        "current"
    );
    assert_eq!(
        antigravity_status_document(
            true,
            &permission_missing,
            &current_skill,
            None,
            false,
            observed(false, false)
        )["state"],
        "different-configuration"
    );
    assert_eq!(
        antigravity_status_document(
            true,
            &legacy_route_left_behind,
            &current_skill,
            None,
            false,
            observed(false, false)
        )["state"],
        "different-configuration"
    );
    assert_eq!(
        antigravity_status_document(
            true,
            &connected,
            &current_skill,
            Some("bad settings".into()),
            false,
            observed(false, false)
        )["state"],
        "status-error"
    );
    // The connected light stays scoped to a route that is actually current.
    let live = antigravity_status_document(
        true,
        &connected,
        &current_skill,
        None,
        true,
        observed(true, true),
    );
    assert_eq!(live["connectionActive"], true);
    assert_eq!(live["bridgeAttached"], true);
    let stale = antigravity_status_document(
        true,
        &permission_missing,
        &current_skill,
        None,
        true,
        observed(true, true),
    );
    assert_eq!(stale["connectionActive"], false);
    assert_eq!(stale["bridgeAttached"], false);
}

#[test]
fn antigravity_permission_pair_and_unpair_preserve_other_settings() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Antigravity settings directory");
    let settings = directory.join("config.json");
    let original = json!({
        "plugins": { "unrelatedPlugin": { "enabled": true } },
        "userSettings": {
            "themeMode": "THEME_MODE_LIGHT",
            "globalPermissionGrants": {
                "allow": ["command(git status)", "mcp(qc-remote/get_current_preset)"],
                "deny": ["command(rm -rf)"]
            }
        }
    });
    fs::write(
        &settings,
        serde_json::to_vec_pretty(&original).expect("serialize settings fixture"),
    )
    .expect("seed Antigravity settings");

    assert!(!antigravity_permission_configured(&settings).expect("inspect permission"));
    write_antigravity_permission(&settings).expect("allow QC Remote tools");
    write_antigravity_permission(&settings).expect("idempotent permission install");
    assert!(antigravity_permission_configured(&settings).expect("inspect paired permission"));
    let paired: Value =
        serde_json::from_slice(&fs::read(&settings).expect("paired Antigravity settings"))
            .expect("valid paired settings");
    assert_eq!(paired["plugins"], original["plugins"]);
    assert_eq!(paired["userSettings"]["themeMode"], "THEME_MODE_LIGHT");
    assert_eq!(
        paired["userSettings"]["globalPermissionGrants"]["deny"],
        original["userSettings"]["globalPermissionGrants"]["deny"]
    );
    assert_eq!(
        paired["userSettings"]["globalPermissionGrants"]["allow"]
            .as_array()
            .expect("allowlist")
            .iter()
            .filter(|value| value.as_str() == Some(ANTIGRAVITY_MCP_PERMISSION))
            .count(),
        1
    );

    assert!(remove_antigravity_permission(&settings).expect("remove managed permission"));
    assert!(!remove_antigravity_permission(&settings).expect("idempotent permission removal"));
    let unpaired: Value =
        serde_json::from_slice(&fs::read(&settings).expect("unpaired Antigravity settings"))
            .expect("valid unpaired settings");
    assert_eq!(unpaired["plugins"], original["plugins"]);
    assert_eq!(
        unpaired["userSettings"]["globalPermissionGrants"]["allow"],
        original["userSettings"]["globalPermissionGrants"]["allow"]
    );

    fs::remove_dir_all(directory).expect("remove Antigravity permission fixture");
}

#[test]
fn antigravity_pair_removes_only_the_stale_legacy_gemini_route() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("legacy Gemini settings directory");
    let settings = directory.join("settings.json");
    let original = json!({
        "mcpServers": {
            "qc-remote": { "command": "old-qc-remote.exe" },
            "other": { "command": "other-server" }
        },
        "security": { "auth": { "selectedType": "oauth-personal" } }
    });
    fs::write(
        &settings,
        serde_json::to_vec_pretty(&original).expect("serialize legacy settings fixture"),
    )
    .expect("seed legacy Gemini settings");

    assert!(legacy_gemini_route_present(&settings).expect("inspect legacy route"));
    assert!(remove_legacy_gemini_route(&settings).expect("remove legacy route"));
    assert!(!remove_legacy_gemini_route(&settings).expect("idempotent legacy removal"));
    assert!(!legacy_gemini_route_present(&settings).expect("inspect removed legacy route"));
    let migrated: Value =
        serde_json::from_slice(&fs::read(&settings).expect("migrated legacy settings"))
            .expect("valid migrated legacy settings");
    assert_eq!(
        migrated["mcpServers"]["other"],
        original["mcpServers"]["other"]
    );
    assert_eq!(migrated["security"], original["security"]);

    fs::remove_dir_all(directory).expect("remove legacy Gemini settings fixture");
}

#[test]
fn managed_json_mcp_pair_update_and_unpair_preserve_other_servers() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("managed MCP fixture directory");
    let config = directory.join("mcp.json");
    let executable = directory.join("qc-remote.exe");
    let original = json!({
        "mcpServers": { "other": { "command": "other-server" } },
        "preferences": { "theme": "dark" }
    });
    fs::write(
        &config,
        serde_json::to_vec_pretty(&original).expect("fixture JSON"),
    )
    .expect("seed managed MCP fixture");

    write_json_mcp_route(
        &config,
        &executable,
        LM_STUDIO_CLIENT_ID,
        "LM Studio",
        false,
    )
    .expect("pair LM Studio");
    let paired: Value = serde_json::from_slice(&fs::read(&config).expect("paired config"))
        .expect("valid paired config");
    assert_eq!(
        paired["mcpServers"]["other"],
        original["mcpServers"]["other"]
    );
    assert_eq!(paired["preferences"], original["preferences"]);
    assert_eq!(paired["mcpServers"]["qc-remote"]["disabled"], false);
    let route = configured_json_mcp_route(
        &config,
        &executable,
        "lm-studio:",
        LM_STUDIO_CLIENT_ID,
        "LM Studio",
        false,
    )
    .expect("inspect LM Studio route");
    assert!(route.configured && route.launcher_current && route.enabled);
    assert_eq!(
        route.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );

    assert!(remove_json_mcp_route(&config, "LM Studio").expect("unpair LM Studio"));
    assert!(!remove_json_mcp_route(&config, "LM Studio").expect("idempotent unpair"));
    let unpaired: Value = serde_json::from_slice(&fs::read(&config).expect("unpaired config"))
        .expect("valid unpaired config");
    assert_eq!(unpaired, original);
    fs::remove_dir_all(directory).expect("remove managed MCP fixture");
}

/// Unpair used to remove routes and permissions and leave the installed
/// skill - and the copy it had taken of the user's own - on disk forever.
#[test]
fn unpair_removes_only_a_skill_that_is_still_ours_and_restores_the_backup() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let skill = directory.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME);
    let backups = directory.join("backups");

    // Nothing installed: removal is a no-op, not an error.
    assert_eq!(
        remove_device_knowledge_skill_at(&skill, &backups).expect("remove nothing"),
        RemovedDeviceKnowledgeSkill::default()
    );

    // A skill the user wrote themselves is never QC Remote's to delete.
    fs::create_dir_all(&skill).expect("user skill directory");
    fs::write(skill.join("SKILL.md"), b"the user's own skill").expect("user skill");
    assert_eq!(
        remove_device_knowledge_skill_at(&skill, &backups).expect("leave the user's skill alone"),
        RemovedDeviceKnowledgeSkill::default()
    );
    assert_eq!(
        fs::read(skill.join("SKILL.md")).expect("untouched user skill"),
        b"the user's own skill"
    );

    // Pair over it, then unpair: the managed copy goes, theirs comes back.
    let installed = install_device_knowledge_skill_at(&skill, &backups);
    assert!(installed.installed, "install failed: {:?}", installed.error);
    let backup = installed.backup.expect("the user's skill was backed up");
    let removed =
        remove_device_knowledge_skill_at(&skill, &backups).expect("unpair the managed skill");
    assert!(removed.removed);
    assert_eq!(removed.restored_from.as_deref(), Some(backup.as_path()));
    assert_eq!(
        fs::read(skill.join("SKILL.md")).expect("restored user skill"),
        b"the user's own skill"
    );

    // A managed skill the user has since edited stays exactly as they left it.
    let reinstalled = install_device_knowledge_skill_at(&skill, &backups);
    assert!(reinstalled.installed);
    fs::write(skill.join("SKILL.md"), b"hand-edited").expect("edit the managed skill");
    assert_eq!(
        device_knowledge_skill_status_at(&skill)["state"],
        "modified"
    );
    assert_eq!(
        remove_device_knowledge_skill_at(&skill, &backups).expect("leave an edited skill alone"),
        RemovedDeviceKnowledgeSkill::default()
    );
    assert_eq!(
        fs::read(skill.join("SKILL.md")).expect("untouched edit"),
        b"hand-edited"
    );

    fs::remove_dir_all(directory).expect("remove tone skill removal fixture");
}

/// Both hosts install the same skill name into different profiles.  One
/// shared backup root would let unpairing one of them restore the copy the
/// other had taken aside.
#[test]
fn each_host_restores_its_own_skill_backup() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let hosts = [
        (CODEX_SKILL_HOST, b"the codex skill".as_slice()),
        (ANTIGRAVITY_SKILL_HOST, b"the gemini skill".as_slice()),
    ];
    assert_ne!(
        device_knowledge_skill_backup_root(CODEX_SKILL_HOST).ok(),
        device_knowledge_skill_backup_root(ANTIGRAVITY_SKILL_HOST).ok(),
        "the two hosts must not share a backup root"
    );
    for (host, contents) in hosts {
        let skill = directory.join(host).join(DEVICE_KNOWLEDGE_SKILL_NAME);
        fs::create_dir_all(&skill).expect("host skill directory");
        fs::write(skill.join("SKILL.md"), contents).expect("host skill");
        let installed =
            install_device_knowledge_skill_at(&skill, &directory.join("backups").join(host));
        assert!(installed.installed, "install failed: {:?}", installed.error);
    }
    for (host, contents) in hosts {
        let skill = directory.join(host).join(DEVICE_KNOWLEDGE_SKILL_NAME);
        let removed =
            remove_device_knowledge_skill_at(&skill, &directory.join("backups").join(host))
                .expect("unpair the host");
        assert!(removed.removed);
        assert_eq!(
            fs::read(skill.join("SKILL.md")).expect("restored skill"),
            contents,
            "{host} was handed another host's backup"
        );
    }

    fs::remove_dir_all(directory).expect("remove skill backup host fixture");
}

/// Pairing installed the skill first and wrote the MCP entry second, so a
/// failed config write left the skill installed, the user's own skill moved
/// aside, and the caller holding nothing but an `Err`.
#[test]
fn pairing_writes_the_route_before_it_touches_the_users_skill() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("config.toml");
    let executable = directory.join("QC Remote.exe");
    let skill = directory.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME);
    let backups = directory.join("backups");
    fs::write(&config, "model = \"gpt-test\"\n").expect("seed configuration");
    fs::create_dir_all(&skill).expect("user skill directory");
    fs::write(skill.join("SKILL.md"), b"the user's own skill").expect("user skill");
    // A file where the backup root belongs: the skill install cannot run.
    fs::write(&backups, b"not a directory").expect("blocking backup root");

    let partial = pair_chatgpt_route(&config, &executable, &skill, &backups)
        .expect("a skill failure must not fail the pair");
    assert_eq!(partial["installed"], true);
    assert_eq!(partial["skillInstalled"], false);
    assert_eq!(partial["skillBackupPath"], Value::Null);
    assert!(
        partial["skillError"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "the partial result names what went wrong: {partial}"
    );
    assert!(
        configured_chatgpt_route(&config, &executable)
            .expect("inspect the route")
            .launcher_current,
        "the MCP entry is written before the skill, so it survives"
    );
    assert_eq!(
        fs::read(skill.join("SKILL.md")).expect("preserved user skill"),
        b"the user's own skill",
        "a pair that could not back the skill up must not replace it"
    );

    fs::remove_file(&backups).expect("clear the blocking backup root");
    let complete = pair_chatgpt_route(&config, &executable, &skill, &backups)
        .expect("pair with a working backup root");
    assert_eq!(complete["skillInstalled"], true);
    assert_eq!(complete["skillError"], Value::Null);
    assert!(complete["skillBackupPath"].as_str().is_some_and(|path| {
        fs::read(Path::new(path).join("SKILL.md"))
            .is_ok_and(|contents| contents == b"the user's own skill")
    }));
    assert_eq!(device_knowledge_skill_status_at(&skill)["state"], "current");

    fs::remove_dir_all(directory).expect("remove pair order fixture");
}

#[test]
fn a_failed_skill_install_is_reported_without_losing_the_paired_route() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let skill = directory.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME);
    let backups = directory.join("backups");
    fs::create_dir_all(&skill).expect("user skill directory");
    fs::write(skill.join("SKILL.md"), b"the user's own skill").expect("user skill");
    // A file where the backup root belongs: the backup cannot be taken, so
    // the user's skill must be left alone and the failure described.
    fs::write(&backups, b"not a directory").expect("blocking backup root");

    let outcome = install_device_knowledge_skill_at(&skill, &backups);
    assert!(!outcome.installed);
    assert!(outcome.backup.is_none());
    let report = outcome.report();
    assert_eq!(report["skillInstalled"], false);
    assert_eq!(report["skillBackupPath"], Value::Null);
    assert!(
        report["skillError"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "the partial result names what went wrong: {report}"
    );
    assert_eq!(
        fs::read(skill.join("SKILL.md")).expect("preserved user skill"),
        b"the user's own skill"
    );

    // The successful case reports the same three fields truthfully.
    fs::remove_file(&backups).expect("clear the blocking backup root");
    let installed = install_device_knowledge_skill_at(&skill, &backups).report();
    assert_eq!(installed["skillInstalled"], true);
    assert_eq!(installed["skillVersion"], DEVICE_KNOWLEDGE_SKILL_VERSION);
    assert_eq!(installed["skillError"], Value::Null);
    assert!(installed["skillBackupPath"]
        .as_str()
        .is_some_and(|path| Path::new(path).join("SKILL.md").is_file()));

    fs::remove_dir_all(directory).expect("remove partial install fixture");
}

/// `serde_json` without `preserve_order` makes `Value::Object` a `BTreeMap`,
/// so adding one entry rewrote another vendor's whole file with every key at
/// every depth sorted and reindented.
#[test]
fn pairing_leaves_another_vendors_key_order_alone() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("key order fixture directory");
    let config = directory.join("mcp.json");
    let executable = directory.join("qc-remote.exe");
    // Deliberately not alphabetical, at two depths.
    let original = "{\n  \"zeta\": 1,\n  \"mcpServers\": {\n    \"zulu\": { \"command\": \"z\" },\n    \"alpha\": { \"command\": \"a\" }\n  },\n  \"alphaRoot\": {\n    \"second\": 2,\n    \"first\": 1\n  }\n}\n";
    fs::write(&config, original).expect("seed unsorted configuration");

    write_json_mcp_route(&config, &executable, JAN_CLIENT_ID, "Jan", true).expect("pair");
    let paired = fs::read_to_string(&config).expect("paired configuration");
    let key_order = |text: &str, keys: [&str; 3]| {
        keys.map(|key| text.find(&format!("\"{key}\"")).unwrap_or(usize::MAX))
    };
    let [zeta, servers, alpha_root] = key_order(&paired, ["zeta", "mcpServers", "alphaRoot"]);
    assert!(
        zeta < servers && servers < alpha_root,
        "root keys were reordered: {paired}"
    );
    let [zulu, alpha, _] = key_order(&paired, ["zulu", "alpha", "zeta"]);
    assert!(zulu < alpha, "nested keys were reordered: {paired}");
    let [second, first, _] = key_order(&paired, ["second", "first", "zeta"]);
    assert!(second < first, "unrelated keys were reordered: {paired}");
    assert!(
        paired.find("\"qc-remote\"").expect("managed entry") > alpha,
        "the managed entry is appended, not interleaved: {paired}"
    );

    assert!(remove_json_mcp_route(&config, "Jan").expect("unpair"));
    let unpaired: Value =
        serde_json::from_slice(&fs::read(&config).expect("unpaired")).expect("valid JSON");
    let seeded: Value = serde_json::from_str(original).expect("valid fixture");
    assert_eq!(unpaired, seeded, "unpairing restores the original document");

    fs::remove_dir_all(directory).expect("remove key order fixture");
}

/// `remove_json_mcp_route` deleted `mcpServers` whenever it emptied it, even
/// when the user had written an empty `mcpServers` themselves.
#[test]
fn unpairing_leaves_an_empty_server_map_the_user_already_had() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("mcp.json");
    let executable = directory.join("qc-remote.exe");
    let original = json!({ "mcpServers": {}, "preferences": { "theme": "dark" } });
    fs::write(
        &config,
        serde_json::to_vec_pretty(&original).expect("fixture JSON"),
    )
    .expect("seed configuration");

    write_json_mcp_route(
        &config,
        &executable,
        LM_STUDIO_CLIENT_ID,
        "LM Studio",
        false,
    )
    .expect("pair LM Studio");
    assert!(remove_json_mcp_route(&config, "LM Studio").expect("unpair LM Studio"));
    let unpaired: Value =
        serde_json::from_slice(&fs::read(&config).expect("unpaired")).expect("valid JSON");
    assert_eq!(unpaired, original, "the user's own empty map survives");

    fs::remove_dir_all(directory).expect("remove empty server map fixture");
}

/// Every other provider is scoped by client id.  ChatGPT's managed route
/// carried no identity, so its bridge could only be observed through the
/// global `/rpc` slot that every other host writes to as well.
#[test]
fn the_chatgpt_route_names_itself_and_reports_its_own_version() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("config.toml");
    let executable = directory.join("QC Remote.exe");
    write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("pair ChatGPT");

    let document = fs::read_to_string(&config)
        .expect("paired configuration")
        .parse::<DocumentMut>()
        .expect("valid TOML");
    assert_eq!(
        document["mcp_servers"][CLAUDE_EXTENSION_NAME]["env"][LOCAL_MCP_CLIENT_ENV].as_str(),
        Some(CODEX_CLIENT_ID)
    );
    assert!(
        CODEX_CLIENT_ID.len() <= 64,
        "the bridge drops a client id longer than 64 bytes"
    );

    // An older QC Remote's route: still ours, still named, just behind.
    fs::write(
            &config,
            format!(
                "[mcp_servers.\"qc-remote\"]\ncommand = {}\nargs = [\"--local-mcp-bridge\"]\ndefault_tools_approval_mode = \"approve\"\n\n[mcp_servers.\"qc-remote\".env]\n{LOCAL_MCP_CLIENT_ENV} = \"codex:0.1.136\"\n",
                toml_edit::Value::from(executable.to_string_lossy().as_ref())
            ),
        )
        .expect("seed an older managed route");
    let older = configured_chatgpt_route(&config, &executable).expect("inspect older route");
    assert!(!older.launcher_current, "an older identity is not current");
    assert_eq!(older.managed_version.as_deref(), Some("0.1.136"));
    assert_eq!(
        chatgpt_status_document(
            true,
            older.configured,
            older.launcher_current,
            older.approval_mode_current,
            older.managed_version.clone(),
            None,
            false,
            observed(false, false)
        )["installedVersion"],
        "0.1.136",
        "the managed route's version comes from the route, never from the \
             separately installed node bridge it does not run"
    );

    fs::remove_dir_all(directory).expect("remove ChatGPT client id fixture");
}

/// The Claude payload carried no liveness at all, so the panel rendered
/// "Connected" whenever a file of the right version sat on disk.
#[test]
fn claude_status_reports_liveness_only_for_a_current_extension() {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let live = claude_status_document(true, Some(current.clone()), observed(true, true));
    assert_eq!(live["connectionActive"], true);
    assert_eq!(live["bridgeAttached"], true);
    let idle = claude_status_document(true, Some(current), observed(false, false));
    assert_eq!(idle["connectionActive"], false);
    assert_eq!(idle["bridgeAttached"], false);
    // A stale or missing extension cannot be the thing that is connected,
    // however much unrelated bridge traffic the app has seen.
    let stale = claude_status_document(true, Some("0.1.1".into()), observed(true, true));
    assert_eq!(stale["state"], "update-available");
    assert_eq!(stale["connectionActive"], false);
    assert_eq!(stale["bridgeAttached"], false);
    let absent = claude_status_document(true, None, observed(true, true));
    assert_eq!(absent["connectionActive"], false);
    assert_eq!(absent["bridgeAttached"], false);

    // The extension names itself so the per-client map has a key for it.
    let manifest: Value =
        serde_json::from_slice(&claude_manifest().expect("manifest")).expect("valid manifest");
    assert_eq!(
        manifest["server"]["mcp_config"]["env"][LOCAL_MCP_CLIENT_ENV].as_str(),
        Some(CLAUDE_CLIENT_ID)
    );
}

/// The documented manual Antigravity route has to satisfy the same check
/// the in-app pairing writes, or the app labels a working server
/// "different-configuration" forever.
#[test]
fn the_manual_antigravity_example_passes_the_apps_own_route_check() {
    let mut archive = zip::ZipArchive::new(Cursor::new(
        local_client_archive().expect("local client archive"),
    ))
    .expect("client zip");
    let example = archive_text(&mut archive, "antigravity/mcp_config.example.json");
    let document: Value = serde_json::from_str(&example).expect("valid example JSON");
    let command = document["mcpServers"][CLAUDE_EXTENSION_NAME]["command"]
        .as_str()
        .expect("example command");

    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("qc-remote.exe");
    // The example names a placeholder profile; point it at a real file and
    // run it through the very function the status panel calls.
    let rewritten = example.replace(
        &serde_json::to_string(command).expect("encode command"),
        &serde_json::to_string(executable.to_string_lossy().as_ref()).expect("encode path"),
    );
    fs::write(&config, &rewritten).expect("write example configuration");
    fs::write(&executable, b"executable fixture").expect("write executable fixture");

    let route = configured_antigravity_route(&config, &executable).expect("inspect the example");
    assert!(
        route.launcher_current,
        "the documented manual configuration must read as current: {rewritten}"
    );

    fs::remove_dir_all(directory).expect("remove Antigravity example fixture");
}

/// The installer threw whenever an MCP server named `qc-remote` existed,
/// which is the state after in-app pairing, and its suggested remedy
/// downgraded the managed native route to this node one.
#[test]
#[cfg(windows)]
fn the_codex_installer_classifies_an_existing_qc_remote_entry() {
    let installer = codex_installer();
    let classifier = installer
        .split_once("# --- qc-remote entry classification ---")
        .expect("classification block")
        .1
        .split_once("# --- end qc-remote entry classification ---")
        .expect("classification block end")
        .0;
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("classifier fixture directory");
    let script = directory.join("classify.ps1");
    fs::write(
            &script,
            format!(
                "{classifier}\n\
                 $bridge = '{BRIDGE_FILE_NAME}'\n\
                 $cases = @(\n\
                 [pscustomobject]@{{ name = 'qc-remote'; command = 'C:\\QC Remote\\qc-remote.exe'; args = @('--local-mcp-bridge') }},\n\
                 [pscustomobject]@{{ name = 'qc-remote'; command = 'node'; args = @(\"C:\\integrations\\$bridge\") }},\n\
                 [pscustomobject]@{{ name = 'qc-remote'; command = 'python'; args = @('other.py') }}\n\
                 )\n\
                 foreach ($case in $cases) {{ Get-QcRemoteEntryAction -Entry $case -BridgeFileName $bridge }}\n\
                 Get-QcRemoteEntryAction -Entry $null -BridgeFileName $bridge\n"
            ),
        )
        .expect("write classifier script");
    let output = run_with_timeout(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script),
        SCRIPT_FIXTURE_TIMEOUT,
    )
    .expect("PowerShell is required to verify the bundled installer");
    assert!(
        output.status.success(),
        "the installer's classifier failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decisions = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        decisions,
        ["managed-native", "replace", "foreign", "add"],
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        installer.contains("codex mcp remove qc-remote"),
        "a foreign entry must still come with the command that clears it"
    );

    fs::remove_dir_all(directory).expect("remove classifier fixture");
}

#[test]
#[cfg(windows)]
fn exported_installer_transactions_preserve_actual_user_bytes_on_failures() {
    let installer = codex_installer();
    let helpers = installer
        .split_once("# --- qc-remote owned installer transactions ---")
        .unwrap()
        .1
        .split_once("# --- end qc-remote owned installer transactions ---")
        .unwrap()
        .0;
    let directory = crate::tests::unique_fixture_directory("installer-transactions");
    let _guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let script = directory.join("transactions.ps1");
    fs::write(&script, format!("param([string]$Root)\n$ErrorActionPreference = 'Stop'\n{helpers}\n{}", r###"
$config = Join-Path $Root 'config.toml'
$original = [Text.Encoding]::UTF8.GetBytes("# custom approvals remain byte-identical`n[mcp_servers.qc-remote]`ncommand='node'`n[unrelated]`nvalue=123`n")
[IO.File]::WriteAllBytes($config, $original)
$script:deniedStage = $null
try {
  Write-QcFileAtomically (Join-Path $Root 'denied.toml') $original $false {
    param($held, $sid)
    $script:deniedStage = $held.Name
    if ($held.Length -ne 0) { throw 'credential bytes preceded security verification' }
    return $false
  }
  throw 'false held security accepted'
} catch { if (-not $_.ToString().Contains('held stage is not owner-private')) { throw } }
if ($null -eq $script:deniedStage -or (Test-Path -LiteralPath $script:deniedStage) -or (Test-Path -LiteralPath (Join-Path $Root 'denied.toml'))) { throw 'denied stage leaked or destination published' }
try {
  Invoke-QcRemoteRouteChange $config { throw 'injected add failure' }
  throw 'failure not surfaced'
} catch {
  if (-not $_.ToString().Contains('injected add failure')) { throw }
}
if (-not (Test-QcBytesEqual (Read-QcConfigBytes $config) $original)) { throw 'failed add retired the original working route' }
$firstRecovery = @(Get-ChildItem -LiteralPath $Root -Filter 'qc-remote-recovery-*')
if ($firstRecovery.Count -ne 1 -or -not (Test-QcBytesEqual (Read-QcConfigBytes $firstRecovery[0].FullName) $original)) { throw 'exact original recovery document missing' }
$acl = (New-Object IO.FileInfo($firstRecovery[0].FullName)).GetAccessControl()
$userSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
if (-not $acl.AreAccessRulesProtected -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -cne $userSid) { throw 'recovery document leaked credentials through inherited permissions' }
[IO.File]::Delete($firstRecovery[0].FullName)
Write-Output 'failed add preserves working route'
$partial = [Text.Encoding]::UTF8.GetBytes("# another writer or partially failing CLI`ncustom=456`n")
[IO.File]::WriteAllBytes($config, $original)
try {
  Invoke-QcRemoteRouteChange $config { [IO.File]::WriteAllBytes($config, $partial); throw 'injected partial add' }
  throw 'failure not surfaced'
} catch {
  if (-not $_.ToString().Contains('injected partial add') -or -not $_.ToString().Contains('recovery document retained')) { throw }
}
if (-not (Test-QcBytesEqual (Read-QcConfigBytes $config) $partial)) { throw 'rollback overwrote changed configuration' }
$recovery = @(Get-ChildItem -LiteralPath $Root -Filter 'qc-remote-recovery-*')
if ($recovery.Count -ne 1 -or -not (Test-QcBytesEqual (Read-QcConfigBytes $recovery[0].FullName) $original)) { throw 'exact recovery document missing' }
Write-Output 'partial change preserved'
$source = Join-Path $Root 'source'
$target = Join-Path $Root 'skill'
$backups = Join-Path $Root 'backups'
New-Item -ItemType Directory -Path $source,$target | Out-Null
New-Item -ItemType Directory -Path (Join-Path $source 'empty') | Out-Null
[IO.File]::WriteAllText((Join-Path $source 'SKILL.md'), 'new complete skill')
[IO.File]::WriteAllText((Join-Path $target 'SKILL.md'), 'old user skill')
try {
  Install-QcSkillTransaction $source $target $backups { param($from,$stage); [IO.File]::WriteAllText((Join-Path $stage 'partial'), 'partial'); throw 'injected copy failure' } { param($from,$to); [IO.Directory]::Move($from,$to) }
  throw 'failure not surfaced'
} catch { if (-not $_.ToString().Contains('injected copy failure')) { throw } }
if ([IO.File]::ReadAllText((Join-Path $target 'SKILL.md')) -cne 'old user skill') { throw 'copy failure retired original skill' }
if (Test-Path -LiteralPath $backups) { throw 'copy failure moved original into backup' }
if (@(Get-ChildItem -LiteralPath $Root -Filter '.qc-skill-install-*').Count -ne 0) { throw 'partial skill stage leaked' }
Write-Output 'copy failure preserves original'
try {
  Install-QcSkillTransaction $source $target $backups {
    param($from,$stage); Get-ChildItem -LiteralPath $from -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse -Force }
  } {
    param($from,$to); if ([IO.Path]::GetFileName($from).StartsWith('.qc-skill-install-')) { throw 'injected activation failure' }; [IO.Directory]::Move($from,$to)
  }
  throw 'failure not surfaced'
} catch { if (-not $_.ToString().Contains('injected activation failure')) { throw } }
if ([IO.File]::ReadAllText((Join-Path $target 'SKILL.md')) -cne 'old user skill') { throw 'activation failure did not restore exact original' }
if (@(Get-ChildItem -LiteralPath $Root -Filter '.qc-skill-install-*').Count -ne 0) { throw 'activation failure stage leaked' }
if (@(Get-ChildItem -LiteralPath $Root -Filter '.qc-install-*').Count -ne 0) { throw 'atomic config stage leaked' }
Write-Output 'activation failure rolls back'
$oversized = Join-Path $source 'oversized'
$large = [IO.File]::Create($oversized)
try { $large.SetLength(64MB + 1) } finally { $large.Dispose() }
try {
  Install-QcSkillTransaction $source $target $backups { throw 'copy admitted despite oversized source' } { throw 'move admitted despite oversized source' }
  throw 'oversized source accepted'
} catch { if (-not $_.ToString().Contains('Skill file exceeds')) { throw } }
if ([IO.File]::ReadAllText((Join-Path $target 'SKILL.md')) -cne 'old user skill') { throw 'inventory refusal changed active user skill' }
if (@(Get-ChildItem -LiteralPath $Root -Filter '.qc-skill-install-*').Count -ne 0) { throw 'inventory refusal stage leaked' }
Write-Output 'bounded inventory refuses before copy'
"###)).unwrap();
    let output = run_with_timeout(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg(&directory)
            .stderr(std::process::Stdio::piped()),
        SCRIPT_FIXTURE_TIMEOUT,
    )
    .expect("bounded installer transaction fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            "failed add preserves working route",
            "partial change preserved",
            "copy failure preserves original",
            "activation failure rolls back",
            "bounded inventory refuses before copy"
        ]
    );
}

/// A backup must not fail the pair over a link in the user's own skill
/// directory.  Creating a symlink needs Developer Mode or elevation on
/// Windows; where that is unavailable the copy itself is still exercised.
#[test]
fn a_skill_backup_copies_the_tree_and_skips_links() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let source = directory.join("source");
    let nested = source.join("references").join("data");
    fs::create_dir_all(&nested).expect("nested source directory");
    fs::write(source.join("SKILL.md"), b"top level").expect("top level file");
    fs::write(nested.join("tones.json"), b"{}").expect("nested file");
    let linked =
        create_file_symlink_for_test(&source.join("SKILL.md"), &source.join("SKILL.link.md"));

    let destination = directory.join("backup");
    copy_directory(&source, &destination).expect("copy the skill directory");
    assert_eq!(
        fs::read(destination.join("SKILL.md")).expect("copied top level file"),
        b"top level"
    );
    assert_eq!(
        fs::read(
            destination
                .join("references")
                .join("data")
                .join("tones.json")
        )
        .expect("copied nested file"),
        b"{}"
    );
    if linked {
        assert!(
            !destination.join("SKILL.link.md").exists(),
            "a link is skipped, never followed into the backup"
        );
        // The same link the copy skips is what a caller that deletes its
        // source has to refuse over, and it has to learn that *before*
        // writing a backup: an abandoned backup is the newest in the root,
        // and unpair restores the newest.
        assert_eq!(
            unbackupable_entries(&source).expect("scan the skill directory"),
            vec![source.join("SKILL.link.md")],
            "the scan names the link the copy would drop",
        );
    } else {
        eprintln!("symlink creation was refused by Windows, so the skip itself was not exercised");
        assert!(
            unbackupable_entries(&source)
                .expect("scan the skill directory")
                .is_empty(),
            "a tree of plain files and directories has nothing a backup would drop",
        );
    }

    fs::remove_dir_all(directory).expect("remove copy fixture");
}

/// The refusal, end to end, and what it promises about the disk.
///
/// `unbackupable_entries` is tested above in isolation - that it names a
/// link - which says nothing about the three things the refusal actually
/// guarantees. A refactor that moved the scan below `fs::remove_dir_all`,
/// or dropped the post-copy check, would pass every other test here while
/// destroying exactly what it exists to protect, so the guarantees are
/// asserted against the filesystem rather than against the return value
/// alone.
///
/// Creating a symlink needs Developer Mode or elevation on Windows. Where
/// that is unavailable there is no link to refuse over, so the same call
/// must *succeed* - which is worth asserting too, and keeps this from
/// becoming a test that quietly checks nothing on most machines.
#[test]
fn a_pair_refuses_rather_than_deleting_a_skill_it_cannot_back_up() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let skill = directory.join("skills").join(DEVICE_KNOWLEDGE_SKILL_NAME);
    let backups = directory.join("backups");
    fs::create_dir_all(&skill).expect("an existing skill directory");
    fs::write(skill.join("NOTES.md"), b"the owner's own notes").expect("an owner file");
    let linked =
        create_file_symlink_for_test(&skill.join("NOTES.md"), &skill.join("NOTES.link.md"));

    let result = install_device_knowledge_skill_at(&skill, &backups);

    if linked {
        assert!(
            !result.installed,
            "a skill folder holding a link must not be replaced",
        );
        let error = result.error.expect("a refusal names what it would delete");
        assert!(
            error.contains("NOTES.link.md") && error.contains("Move it aside first"),
            "the refusal names the entry and what to do about it: {error}",
        );
        // The point of refusing: the owner's directory is untouched.
        assert!(
            skill.join("NOTES.md").is_file(),
            "the owner's own file survives a refused pair",
        );
        assert!(
            skill.join("NOTES.link.md").exists(),
            "the link the pair refused over is still there",
        );
        // And nothing is left for a later unpair to restore, because
        // `latest_device_knowledge_skill_backup` takes the newest.
        assert_eq!(
            latest_device_knowledge_skill_backup(&backups),
            None,
            "a refused pair leaves no backup behind",
        );
    } else {
        eprintln!(
            "symlink creation was refused by Windows, so the refusal itself was not exercised"
        );
        assert!(
            result.installed,
            "with nothing to refuse over the same call installs the skill",
        );
        assert!(
            skill.join("SKILL.md").is_file(),
            "the managed skill is in place",
        );
    }
}

#[test]
fn jan_pair_marks_the_server_active() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("Jan MCP fixture directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("qc-remote.exe");
    write_json_mcp_route(&config, &executable, JAN_CLIENT_ID, "Jan", true).expect("pair Jan");
    let paired: Value = serde_json::from_slice(&fs::read(&config).expect("paired Jan config"))
        .expect("valid Jan config");
    assert_eq!(paired["mcpServers"]["qc-remote"]["active"], true);
    assert!(
        configured_json_mcp_route(&config, &executable, "jan:", JAN_CLIENT_ID, "Jan", true,)
            .expect("inspect Jan route")
            .enabled
    );
    fs::remove_dir_all(directory).expect("remove Jan MCP fixture");
}

/// Read a managed route's `args` out of a JSON MCP configuration on disk.
fn json_route_arguments(config: &Path) -> Vec<String> {
    let document: Value = serde_json::from_slice(&fs::read(config).expect("configuration"))
        .expect("valid configuration JSON");
    document["mcpServers"][CLAUDE_EXTENSION_NAME]["args"]
        .as_array()
        .expect("the managed route declares arguments")
        .iter()
        .filter_map(|argument| argument.as_str().map(str::to_owned))
        .collect()
}

fn borrowed(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}

fn current_device_knowledge_skill() -> Value {
    json!({
        "installed": true,
        "installedVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
        "state": "current",
    })
}

/// The app credits a heartbeat to whichever host named itself, and the
/// environment is not a channel it can rely on: the ChatGPT/Codex host runs
/// the native STDIO child without the `env` it holds in its own server
/// state, so a route identified only that way drove the hardware while its
/// row stayed amber. Two properties settle it, and the second is what makes
/// the first reachable for somebody who paired before the argument existed.
#[test]
fn the_chatgpt_route_names_its_host_where_the_host_cannot_drop_it() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("config.toml");
    let executable = directory.join("qc-remote.exe");

    write_codex_config(&config, &executable, CODEX_CLIENT_ID).expect("pair ChatGPT");
    let document = fs::read_to_string(&config)
        .expect("paired configuration")
        .parse::<DocumentMut>()
        .expect("valid TOML");
    let arguments = document["mcp_servers"][CLAUDE_EXTENSION_NAME]["args"]
        .as_array()
        .expect("the managed route declares arguments")
        .iter()
        .filter_map(|argument| argument.as_str())
        .collect::<Vec<_>>();
    assert!(
        arguments_name_client(&arguments, CODEX_CLIENT_ID),
        "the written route must name its host on the command line: {arguments:?}"
    );
    assert!(
        configured_chatgpt_route(&config, &executable)
            .expect("inspect the new route")
            .launcher_current,
        "the route this app just wrote must read as current"
    );

    // The same route as written before the argument existed: this app's
    // own entry, at this app's own version, named in `env` alone.
    fs::write(
            &config,
            format!(
                "[mcp_servers.\"qc-remote\"]\ncommand = {}\nargs = [\"--local-mcp-bridge\"]\ndefault_tools_approval_mode = \"approve\"\n\n[mcp_servers.\"qc-remote\".env]\n{LOCAL_MCP_CLIENT_ENV} = \"{CODEX_CLIENT_ID}\"\n",
                toml_edit::Value::from(executable.to_string_lossy().as_ref())
            ),
        )
        .expect("seed an environment-only pairing");
    let legacy = configured_chatgpt_route(&config, &executable).expect("inspect the legacy route");
    assert!(legacy.configured, "the pairing still exists");
    assert!(
        !legacy.launcher_current,
        "a route the app would not write today is not current"
    );
    assert_eq!(
        legacy.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION")),
        "the route is still recognised as this app's own"
    );
    let status = chatgpt_status_document(
        true,
        legacy.configured,
        legacy.launcher_current,
        legacy.approval_mode_current,
        legacy.managed_version.clone(),
        None,
        false,
        observed(false, false),
    );
    assert_eq!(
        status["state"], "update-available",
        "an environment-only pairing is this app's own route needing a \
             rewrite, not a configuration it does not recognise"
    );
    assert_eq!(
        status["integrationInstalled"], true,
        "the row has to keep the control that removes the pairing"
    );

    fs::remove_dir_all(directory).expect("remove ChatGPT identity fixture");
}

#[test]
fn the_lm_studio_route_names_its_host_where_the_host_cannot_drop_it() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("mcp.json");
    let executable = directory.join("qc-remote.exe");

    write_json_mcp_route(
        &config,
        &executable,
        LM_STUDIO_CLIENT_ID,
        "LM Studio",
        false,
    )
    .expect("pair LM Studio");
    let arguments = json_route_arguments(&config);
    assert!(
        arguments_name_client(&borrowed(&arguments), LM_STUDIO_CLIENT_ID),
        "the written route must name its host on the command line: {arguments:?}"
    );
    assert!(
        configured_json_mcp_route(
            &config,
            &executable,
            "lm-studio:",
            LM_STUDIO_CLIENT_ID,
            "LM Studio",
            false,
        )
        .expect("inspect the new route")
        .launcher_current,
        "the route this app just wrote must read as current"
    );

    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                CLAUDE_EXTENSION_NAME: {
                    "command": executable.to_string_lossy(),
                    "args": ["--local-mcp-bridge"],
                    "env": { LOCAL_MCP_CLIENT_ENV: LM_STUDIO_CLIENT_ID },
                    "disabled": false
                }
            }
        }))
        .expect("serialize an environment-only pairing"),
    )
    .expect("seed an environment-only pairing");
    let legacy = configured_json_mcp_route(
        &config,
        &executable,
        "lm-studio:",
        LM_STUDIO_CLIENT_ID,
        "LM Studio",
        false,
    )
    .expect("inspect the legacy route");
    assert!(legacy.configured && legacy.enabled);
    assert!(!legacy.launcher_current);
    assert_eq!(
        legacy.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let status = local_json_integration_status(
        true,
        None,
        Ok(legacy),
        "lmStudioInstalled",
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_SUPPORTED_VERSIONS,
    );
    assert_eq!(status["state"], "update-available");
    assert_eq!(status["integrationInstalled"], true);

    fs::remove_dir_all(directory).expect("remove LM Studio identity fixture");
}

/// A scratch directory for one of the Bionic document tests.
fn bionic_fixture(name: &str) -> PathBuf {
    crate::tests::unique_fixture_directory(&format!("client-bionic-{name}"))
}

/// A user-authored Bionic MCP server entry, unrelated to this app. Nothing
/// in it is this app's, and every check below requires it back exactly as
/// it went in: losing somebody's MCP servers while pairing would be worse
/// than not pairing at all.
fn their_own_bionic_server() -> Value {
    json!({
        "id": "d170175c-8040-4750-85f2-575e4a114adb",
        "name": "QC R",
        "enabled": true,
        "connection": {
            "type": "stdio",
            "command": "http://127.0.0.1:12345",
            "args": [],
            "env": {}
        }
    })
}

/// A second entry carrying keys this app has no model for, so "unrelated
/// entries survive" is asserted about more than the fields this writer
/// happens to know how to spell. A merge that rebuilt each entry from the
/// keys it understands would drop every one of these and still keep the
/// entry's name.
fn a_bionic_server_with_keys_we_do_not_understand() -> Value {
    json!({
        "id": "0f3a1c88-7b21-4a05-9d6e-2b8c4f1e5a70",
        "name": "their-research-tools",
        "enabled": false,
        "notes": "keep me",
        "toolApprovals": { "search": "always" },
        "connection": {
            "type": "stdio",
            "command": "tools.exe",
            "args": ["--verbose"],
            "env": { "THEIR_TOKEN": "theirs" },
            "startupTimeoutMs": 9000
        }
    })
}

fn seed_bionic_document(config: &Path, servers: Vec<Value>) {
    fs::write(
        config,
        serde_json::to_vec_pretty(&json!({ "servers": servers }))
            .expect("serialize a Bionic document"),
    )
    .expect("seed a Bionic document");
}

fn bionic_document(config: &Path) -> Value {
    serde_json::from_slice(&fs::read(config).expect("read the Bionic document"))
        .expect("the Bionic document is valid JSON")
}

fn bionic_servers_of(config: &Path) -> Vec<Value> {
    bionic_document(config)["servers"]
        .as_array()
        .expect("the Bionic document carries a servers array")
        .clone()
}

fn managed_bionic_servers(config: &Path) -> Vec<Value> {
    bionic_servers_of(config)
        .into_iter()
        .filter(bionic_entry_is_managed)
        .collect()
}

fn pair_bionic(config: &Path, executable: &Path) {
    write_bionic_mcp_route(
        config,
        executable,
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_CONFIG_OWNER,
    )
    .expect("pair Bionic");
}

fn read_bionic_route(config: &Path, executable: &Path) -> ConfiguredJsonMcpRoute {
    configured_bionic_route(
        config,
        executable,
        LM_STUDIO_CLIENT_PREFIX,
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_CONFIG_OWNER,
    )
    .expect("read the Bionic route")
}

/// The document Bionic reads, in the shape Bionic writes.
///
/// The entry is that server's own JSON with this app's command, arguments
/// and identity in it. A route written to the pre-rename
/// `~\.lmstudio\mcp.json` is not read by Bionic at all, so the old writer
/// reported success while writing a file nothing opens.
///
/// The literals here are deliberately literals. A test that spelled the
/// document with the same constants the writer uses would follow any rename
/// of them and stay green through exactly the substitution it exists to
/// catch - this is the host's format, not ours to redefine.
/// Bionic's skill folder is the one its own documentation names, and the
/// legacy location is honoured only when the current one is absent.
///
/// `~/.lmstudio/skills/<name>/SKILL.md` is stated by the `skill-management`
/// skill Bionic ships. The legacy `~/.cache/lm-studio/skills` is described
/// there as a rare, very old installation, so a machine carrying both must
/// be written where Bionic actually reads - getting that backwards installs
/// a skill that is never loaded and reports success for it.
#[test]
fn the_bionic_skill_path_prefers_the_location_bionic_documents() {
    let home = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(home.clone());
    std::fs::create_dir_all(&home).expect("temp home");
    let current = home
        .join(".lmstudio")
        .join("skills")
        .join("qc-device-knowledge");
    let legacy = home
        .join(".cache")
        .join("lm-studio")
        .join("skills")
        .join("qc-device-knowledge");

    // Neither directory present: still the current location, never the
    // legacy one, because a fresh install is not an old one.
    assert_eq!(
        bionic_device_knowledge_skill_path_in(&home),
        current,
        "a machine with neither directory must be offered the current layout",
    );

    // Only the legacy tree present: honour it.
    std::fs::create_dir_all(home.join(".cache").join("lm-studio")).expect("legacy");
    assert_eq!(
        bionic_device_knowledge_skill_path_in(&home),
        legacy,
        "a very old installation must be written where that installation reads",
    );

    // Both present: the current one wins. Getting this backwards installs a
    // skill Bionic never loads and reports success for it.
    std::fs::create_dir_all(home.join(".lmstudio")).expect("current");
    assert_eq!(
        bionic_device_knowledge_skill_path_in(&home),
        current,
        "with both on disk the skill must go where Bionic reads it now",
    );

    // An installed copy outranks both roots, which is what stops the answer
    // moving between calls. Status and install are separate commands behind
    // separate clicks, so Bionic creating `~/.lmstudio` in between used to
    // send the install to a directory the status call had never inspected -
    // and left update and unpair unable to reach the copy pair had written.
    let _ = std::fs::remove_dir_all(home.join(".lmstudio"));
    std::fs::create_dir_all(&legacy).expect("an installed legacy skill");
    std::fs::create_dir_all(home.join(".lmstudio")).expect("current root appears");
    assert_eq!(
        bionic_device_knowledge_skill_path_in(&home),
        legacy,
        "a skill already installed under the legacy root stays the answer once the current root appears",
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// A paired row whose skill is missing or stale must offer Update, and a
/// host with no skill at all must be left alone.
///
/// These two travel together because separating them is the defect: the
/// five `skill*` fields are what the panel prints, and the fold is what
/// turns the row's button from Open into Update. A row that published
/// `skillState: "not-installed"` while its own state still read `current`
/// would show Open and never install the skill.
#[test]
fn a_missing_skill_turns_a_paired_row_into_an_update_and_spares_a_host_without_one() {
    let paired = || json!({ "state": "current", "integrationInstalled": true });

    let mut needs_skill = paired();
    apply_device_knowledge_skill_status(
        &mut needs_skill,
        &json!({ "installed": false, "availableVersion": "1.0.0", "state": "not-installed" }),
    );
    assert_eq!(
        needs_skill["state"], "update-available",
        "a paired host missing the skill must offer Update, not Open",
    );
    assert_eq!(needs_skill["skillState"], "not-installed");
    assert_eq!(needs_skill["availableSkillVersion"], "1.0.0");

    let mut current = paired();
    apply_device_knowledge_skill_status(
        &mut current,
        &json!({ "installed": true, "installedVersion": "1.0.0", "availableVersion": "1.0.0", "state": "current" }),
    );
    assert_eq!(
        current["state"], "current",
        "a host whose skill is current keeps the calm row",
    );

    // Unreadable is not the same as different. This used to fall through to
    // "different-configuration", which accuses the owner's skill of
    // conflicting with the managed one and offers an Update that would try
    // to back up the very directory that cannot be read.
    let mut unreadable = paired();
    apply_device_knowledge_skill_status(
        &mut unreadable,
        &json!({
            "installed": false,
            "availableVersion": "1.0.0",
            "state": "status-error",
            "statusError": "Access is denied.",
        }),
    );
    assert_eq!(
        unreadable["state"], "current",
        "a skill folder that cannot be read must not be reported as a conflicting configuration",
    );
    assert_eq!(
        unreadable["skillStatusError"], "Access is denied.",
        "the row carries the error it actually has",
    );

    // A skill the owner has edited *is* different, and still says so.
    let mut modified = paired();
    apply_device_knowledge_skill_status(
        &mut modified,
        &json!({ "installed": true, "availableVersion": "1.0.0", "state": "modified" }),
    );
    assert_eq!(
        modified["state"], "different-configuration",
        "an edited skill is a real conflict and keeps its row",
    );

    // A row already describing something worse than a stale skill is not
    // improved by one, so the fold leaves it alone.
    let mut absent = json!({ "state": "not-installed", "integrationInstalled": false });
    apply_device_knowledge_skill_status(
        &mut absent,
        &json!({ "installed": false, "availableVersion": "1.0.0", "state": "not-installed" }),
    );
    assert_eq!(
        absent["state"], "not-installed",
        "a row that is not paired at all must not be recast as an update",
    );

    // Jan: the native side never calls this, so its row carries no skill
    // field at all and the panel prints no skill sentence.
    let jan = paired();
    assert!(
        jan.get("skillState").is_none(),
        "a host with no skills mechanism must publish no skill state",
    );
}

#[test]
fn the_bionic_route_is_written_where_bionic_reads_it_in_the_shape_bionic_writes() {
    assert!(
        bionic_config_path()
            .expect("a Windows user profile")
            .ends_with(Path::new(".lmstudio/apps/bionic/.internal/ng-mcp.json")),
        "the correct path is the only one Bionic reads"
    );

    let directory = bionic_fixture("shape");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("ng-mcp.json");
    let executable = directory.join("QC Remote.exe");
    pair_bionic(&config, &executable);

    assert_eq!(
        bionic_document(&config),
        json!({
            "servers": [{
                "id": BIONIC_MANAGED_SERVER_ID,
                "name": "qc-remote",
                "enabled": true,
                "connection": {
                    "type": "stdio",
                    "command": executable.to_string_lossy(),
                    "args": [
                        "--local-mcp-bridge",
                        "--local-mcp-client",
                        LM_STUDIO_CLIENT_ID
                    ],
                    "env": { "QC_REMOTE_LOCAL_MCP_CLIENT": LM_STUDIO_CLIENT_ID }
                }
            }]
        }),
        "the written document is not the one Bionic's own writer produces"
    );
    assert_eq!(
        bionic_document(&config)["servers"][0]["enabled"],
        json!(true),
        "this app enables its own server, so a Bionic pairing leaves the \
             owner nothing to switch on"
    );

    let route = read_bionic_route(&config, &executable);
    assert!(
        route.configured && route.launcher_current && route.enabled,
        "the route this app just wrote must read back as current: {route:?}"
    );
    assert_eq!(
        route.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION")),
        "the route must be recognisable as this build's own"
    );

    fs::remove_dir_all(directory).expect("remove the Bionic shape fixture");
}

/// The array has no key to overwrite, so pairing has to merge.
///
/// Every stage of the provider's life is run over one document holding two
/// servers that are not this app's, and each stage has to hand both of them
/// back unchanged, in the order they were in, with keys this app does not
/// model still on them.
#[test]
fn a_server_the_owner_added_survives_a_pair_a_refresh_and_an_unpair() {
    let directory = bionic_fixture("neighbours");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("ng-mcp.json");
    let executable = directory.join("QC Remote.exe");
    let theirs = their_own_bionic_server();
    let stranger = a_bionic_server_with_keys_we_do_not_understand();
    seed_bionic_document(&config, vec![theirs.clone(), stranger.clone()]);

    let their_servers_are_intact = |stage: &str| {
        let servers = bionic_servers_of(&config);
        assert!(
            servers.len() >= 2,
            "{stage}: the owner's two servers are no longer both in the document: {servers:?}"
        );
        assert_eq!(servers[0], theirs, "{stage}: the owner's own server");
        assert_eq!(
            servers[1], stranger,
            "{stage}: a server whose keys this app does not understand"
        );
    };

    pair_bionic(&config, &executable);
    their_servers_are_intact("after pairing");
    assert_eq!(
        bionic_servers_of(&config).len(),
        3,
        "pairing adds exactly one server"
    );

    // A route this app wrote one release ago, in the position pairing left
    // it, so the unattended pass has something to freshen.
    seed_bionic_document(
        &config,
        vec![
            theirs.clone(),
            stranger.clone(),
            bionic_route_server(&executable, "lm-studio:0.1.100", None),
        ],
    );
    let target = ManagedRouteTarget {
        provider: LM_STUDIO_ROUTE,
        label: LM_STUDIO_CONFIG_OWNER,
        config_path: config.clone(),
        executable_path: executable.clone(),
        client_id: LM_STUDIO_CLIENT_ID,
        client_prefix: LM_STUDIO_CLIENT_PREFIX,
        format: ManagedRouteFormat::Bionic,
    };
    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        },
        "an entry among other servers is still recognisable as this app's own"
    );
    assert!(outcome.failure.is_none(), "{outcome:?}");
    their_servers_are_intact("after the start-up refresh");
    assert!(
        read_bionic_route(&config, &executable).launcher_current,
        "the freshened route must read as this build's own"
    );

    assert!(
        remove_bionic_mcp_route(&config, LM_STUDIO_CONFIG_OWNER).expect("unpair Bionic"),
        "unpairing a paired document removes something"
    );
    their_servers_are_intact("after unpairing");
    assert_eq!(
        bionic_servers_of(&config),
        vec![theirs, stranger],
        "unpairing removed a server that was not this app's"
    );
    assert!(
        !remove_bionic_mcp_route(&config, LM_STUDIO_CONFIG_OWNER).expect("unpair again"),
        "a second unpair has nothing of this app's left to remove"
    );

    fs::remove_dir_all(directory).expect("remove the Bionic neighbours fixture");
}

/// Pairing is an update, not an append.
///
/// Without an identity for this app's own entry the array grows one more
/// QC Remote server every time Pair is pressed and every time the start-up
/// refresh runs, which the owner would read as this panel corrupting his
/// configuration.
#[test]
fn pairing_twice_leaves_one_qc_remote_server_rather_than_two() {
    let directory = bionic_fixture("duplicates");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("ng-mcp.json");
    let executable = directory.join("QC Remote.exe");
    seed_bionic_document(&config, vec![their_own_bionic_server()]);

    pair_bionic(&config, &executable);
    let after_first = bionic_document(&config);
    pair_bionic(&config, &executable);
    pair_bionic(&config, &executable);

    assert_eq!(
        managed_bionic_servers(&config).len(),
        1,
        "pairing again appended a second QC Remote server: {:?}",
        bionic_servers_of(&config)
    );
    assert_eq!(
        bionic_document(&config),
        after_first,
        "pairing an already paired document changed it"
    );

    fs::remove_dir_all(directory).expect("remove the Bionic duplicate fixture");
}

/// A document that already carries two of this app's entries - which no
/// version of this writer produces, and which is what a broken duplicate
/// guard leaves behind - is collapsed rather than added to.
#[test]
fn a_document_already_holding_two_of_our_servers_is_collapsed_to_one() {
    let directory = bionic_fixture("collapse");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("ng-mcp.json");
    let executable = directory.join("QC Remote.exe");
    let theirs = their_own_bionic_server();
    seed_bionic_document(
        &config,
        vec![
            bionic_route_server(&executable, "lm-studio:0.1.100", Some("first-id")),
            theirs.clone(),
            bionic_route_server(&executable, "lm-studio:0.1.101", Some("second-id")),
        ],
    );

    pair_bionic(&config, &executable);

    let servers = bionic_servers_of(&config);
    assert_eq!(
        servers.len(),
        2,
        "the duplicate was not collapsed: {servers:?}"
    );
    assert_eq!(
        servers[1], theirs,
        "collapsing the duplicates moved the owner's server"
    );
    assert_eq!(
        servers[0]["id"],
        json!("first-id"),
        "the surviving entry keeps the first id, not the duplicate's"
    );

    fs::remove_dir_all(directory).expect("remove the Bionic collapse fixture");
}

/// The `id` is the host's, and a rewrite hands it back.
///
/// Bionic mints a UUID per server it creates, so this app's entry can be
/// carrying an id this app did not choose - after the owner recreated it
/// through Bionic's UI, or after any future version of Bionic rewrote its
/// own file. Minting a fresh one over it would be this app overwriting a
/// key belonging to the host, and it would make the start-up refresh read
/// its own freshly written route as a stranger's.
#[test]
fn an_id_this_app_did_not_choose_is_kept_and_the_refresh_still_recognises_the_route() {
    let directory = bionic_fixture("identity");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("ng-mcp.json");
    let executable = directory.join("QC Remote.exe");
    const THEIR_ID: &str = "c41f7b02-5d18-4ee9-9a37-6f0d2e58b1c4";
    seed_bionic_document(
        &config,
        vec![bionic_route_server(
            &executable,
            "lm-studio:0.1.100",
            Some(THEIR_ID),
        )],
    );

    let target = ManagedRouteTarget {
        provider: LM_STUDIO_ROUTE,
        label: LM_STUDIO_CONFIG_OWNER,
        config_path: config.clone(),
        executable_path: executable.clone(),
        client_id: LM_STUDIO_CLIENT_ID,
        client_prefix: LM_STUDIO_CLIENT_PREFIX,
        format: ManagedRouteFormat::Bionic,
    };
    assert_eq!(
        refresh_managed_route(&target).decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        },
        "an entry carrying the host's own id is still this app's entry"
    );
    assert_eq!(
        bionic_servers_of(&config)[0]["id"],
        json!(THEIR_ID),
        "the rewrite replaced an id the host had minted"
    );
    assert_eq!(
        refresh_managed_route(&target).decision,
        RouteRefreshDecision::Current,
        "the route this pass just wrote must read as current, or it is \
             rewritten on every app start for ever"
    );

    fs::remove_dir_all(directory).expect("remove the Bionic identity fixture");
}

/// Which document an installation reads, and the one reading that decides
/// it.
///
/// The numbering series is also what heads the row and what the update gate
/// judged the install in, so the application the panel names and the file
/// the pairing lands in cannot disagree.
#[test]
fn a_bionic_install_and_a_legacy_install_read_different_documents() {
    let installed = |version: Option<&str>| DesktopInstallation {
        executable: None,
        version: version.map(str::to_owned),
    };

    // `1.1.0.0` is a representative Bionic version: VS_FIXEDFILEINFO zeroes
    // Bionic's patch and build fields while the version string beside it
    // reads `1.1.6+3`.
    let bionic = lm_studio_mcp_document(Some(&installed(Some("1.1.0.0"))))
        .expect("resolve a Bionic installation's document");
    assert!(bionic.bionic);
    assert!(
        bionic
            .path
            .ends_with(Path::new(".lmstudio/apps/bionic/.internal/ng-mcp.json")),
        "{:?}",
        bionic.path
    );

    let legacy = lm_studio_mcp_document(Some(&installed(Some("0.3.31"))))
        .expect("resolve a pre-rename installation's document");
    assert!(!legacy.bionic);
    assert!(
        legacy.path.ends_with(Path::new(".lmstudio/mcp.json")),
        "{:?}",
        legacy.path
    );

    // No install, and a version too strange to file, belong to no series.
    // Both take Bionic - what lmstudio.ai ships today, and what the row
    // already calls such an install - so the label and the document stay
    // one reading.
    for version in [None, Some("preview"), Some("")] {
        assert!(
            lm_studio_mcp_document(Some(&installed(version)))
                .expect("resolve an unreadable version's document")
                .bionic,
            "{version:?}"
        );
    }
    assert!(
        lm_studio_mcp_document(None)
            .expect("resolve an absent installation's document")
            .bionic
    );
}

/// A pre-rename install still gets the document it reads, unchanged.
#[test]
fn a_legacy_lm_studio_is_still_written_the_mcp_servers_document_it_reads() {
    let directory = bionic_fixture("legacy");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let executable = directory.join("QC Remote.exe");
    let legacy = LmStudioMcpDocument {
        path: directory.join("mcp.json"),
        bionic: false,
    };
    write_lm_studio_route(&legacy, &executable, LM_STUDIO_CLIENT_ID).expect("pair LM Studio");

    let document = bionic_document(&legacy.path);
    assert_eq!(
        document["mcpServers"]["qc-remote"]["command"],
        json!(executable.to_string_lossy()),
        "a pre-rename install must still get the map-shaped document: {document}"
    );
    assert_eq!(
        document["mcpServers"]["qc-remote"]["disabled"],
        json!(false)
    );
    assert!(
        document.get("servers").is_none(),
        "Bionic's array must not appear in a pre-rename document: {document}"
    );
    assert!(
        configured_lm_studio_route(&legacy, &executable)
            .expect("read the legacy route")
            .launcher_current,
        "the legacy reader must still read the legacy writer's output"
    );

    let bionic = LmStudioMcpDocument {
        path: directory.join("ng-mcp.json"),
        bionic: true,
    };
    write_lm_studio_route(&bionic, &executable, LM_STUDIO_CLIENT_ID).expect("pair Bionic");
    assert!(
        bionic_document(&bionic.path).get("mcpServers").is_none(),
        "the pre-rename map must not appear in a Bionic document"
    );
    assert!(
        configured_lm_studio_route(&bionic, &executable)
            .expect("read the Bionic route")
            .launcher_current
    );

    fs::remove_dir_all(directory).expect("remove the LM Studio family fixture");
}

/// A document this app cannot account for is left exactly as it is.
///
/// Each of these is a file somebody or something else owns the contents of.
/// Replacing one with a document this app built from scratch would be the
/// data loss this whole merge exists to avoid, wearing the clothes of a
/// repair - so every entry point refuses, and the proof that nothing was
/// written is the bytes plus the absence of a backup, since the writer
/// copies the live file aside before replacing it.
#[test]
fn a_bionic_document_this_app_cannot_account_for_is_refused_rather_than_replaced() {
    let directory = bionic_fixture("malformed");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let executable = directory.join("QC Remote.exe");
    for (name, contents) in [
        ("not-json", "{ \"servers\": [ "),
        ("root-is-an-array", "[{\"name\":\"qc-remote\"}]"),
        ("root-is-a-string", "\"servers\""),
        (
            "servers-is-an-object",
            "{\"servers\":{\"qc-remote\":{\"enabled\":true}}}",
        ),
    ] {
        let config = directory.join(format!("{name}.json"));
        fs::write(&config, contents).expect("seed a malformed document");

        assert!(
            write_bionic_mcp_route(
                &config,
                &executable,
                LM_STUDIO_CLIENT_ID,
                LM_STUDIO_CONFIG_OWNER
            )
            .is_err(),
            "{name}: pairing rewrote a document this app cannot read"
        );
        assert!(
            read_bionic_route_result(&config, &executable).is_err(),
            "{name}: the status read claimed to understand it"
        );
        assert!(
            remove_bionic_mcp_route(&config, LM_STUDIO_CONFIG_OWNER).is_err(),
            "{name}: unpairing rewrote a document this app cannot read"
        );
        assert_eq!(
            fs::read_to_string(&config).expect("the refused document"),
            contents,
            "{name}: the document was modified"
        );
        assert!(
            config_backups(&config).is_empty(),
            "{name}: a backup exists, so a writer ran on it"
        );
    }

    fs::remove_dir_all(directory).expect("remove the malformed Bionic fixture");
}

fn read_bionic_route_result(
    config: &Path,
    executable: &Path,
) -> Result<ConfiguredJsonMcpRoute, String> {
    configured_bionic_route(
        config,
        executable,
        LM_STUDIO_CLIENT_PREFIX,
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_CONFIG_OWNER,
    )
}

/// Renaming the row must not rename the identity underneath it.
///
/// The row is headed `Bionic` now, and the mistake the rename invites is to
/// "finish the job" by renaming `lm-studio:` to match. That is a cosmetic
/// change quietly breaking a functional one. The prefix is written into
/// every route's `--local-mcp-client` argument and its
/// `QC_REMOTE_LOCAL_MCP_CLIENT`; stripping it is how this app recovers the
/// version a pairing was written by, which is what tells its own route from
/// a configuration somebody else wrote; and the full id is the key the
/// durable attach history records a host's bridge under. Move it and every
/// pairing on disk becomes a stranger's and its history is orphaned.
///
/// So the identity is asserted against the **literal** string shipped
/// builds wrote, never against the constant. A test that read the constant
/// would follow a rename and stay green through exactly the mistake it
/// exists to catch.
#[test]
fn renaming_the_row_must_not_rename_the_client_identity_pairings_are_keyed_by() {
    // Both halves of the separation, in one place, so neither can be read
    // as the other. The row's name is the newest numbering series' product
    // name, which the rename did move.
    assert_eq!(
        LM_STUDIO_SUPPORTED_VERSIONS[0].name, "Bionic",
        "the row is headed by the newest series' product name"
    );
    // `app_version_series` returns the *first* row whose floor a version
    // reaches, so every table it reads has to be sorted newest-first. That
    // was stated in prose and pinned only for element zero, which a series
    // inserted in the middle does not disturb: a later 2.x line added after
    // the 1.0.0 row would send every 2.x install into the Bionic series,
    // taking Bionic's minimum and Bionic's name. Assert the order itself,
    // for every table, rather than one entry of one of them.
    // Counted, because `windows(2)` over a one-row table yields nothing:
    // Jan declares a single series today, so a loop that only asserted
    // inside its body would assert nothing at all for it and still pass.
    // The total says how many adjacent pairs were actually compared.
    let tables = [LM_STUDIO_SUPPORTED_VERSIONS, JAN_SUPPORTED_VERSIONS];
    let mut rows = 0usize;
    let mut compared = 0usize;
    for series in tables {
        assert!(
            !series.is_empty(),
            "a supported-version table with no rows classifies nothing",
        );
        rows += series.len();
        for pair in series.windows(2) {
            assert_eq!(
                compare_versions(pair[0].series_begins_at, pair[1].series_begins_at),
                Some(Ordering::Greater),
                "{} must be declared before {}: app_version_series takes the first row a version reaches",
                pair[0].name,
                pair[1].name,
            );
            compared += 1;
        }
    }
    // `compared >= 1` is the anti-vacuity claim and is not a formality: a
    // table with one row is non-empty yet yields no windows, so asserting
    // the tables are non-empty would pass while comparing nothing. Only the
    // count says the loop body actually ran.
    assert!(compared >= 1, "no adjacent pair was compared at all");
    assert_eq!(
        compared,
        rows - tables.len(),
        "every adjacent pair in every table must have been compared",
    );
    // The identity, which it did not.
    assert!(
        LM_STUDIO_CLIENT_ID.starts_with("lm-studio:"),
        "the client id every pairing and its attach history are keyed by \
             followed the label: {LM_STUDIO_CLIENT_ID}"
    );

    let directory = installation_fixture("lm-studio-identity-outlives-the-label");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let executable = directory.join("qc-remote.exe");

    // A pairing exactly as a shipped build left it on disk, written out
    // rather than produced by today's writer, so this cannot pass by both
    // sides moving together.
    const SHIPPED_IDENTITY: &str = "lm-studio:0.1.100";
    let existing = directory.join("shipped-mcp.json");
    fs::write(
        &existing,
        serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                CLAUDE_EXTENSION_NAME: {
                    "command": executable.to_string_lossy(),
                    "args": ["--local-mcp-bridge", CLIENT_ID_ARGUMENT, SHIPPED_IDENTITY],
                    "env": { LOCAL_MCP_CLIENT_ENV: SHIPPED_IDENTITY },
                    "disabled": false
                }
            }
        }))
        .expect("serialize a pairing an earlier build wrote"),
    )
    .expect("seed a pairing an earlier build wrote");

    let route = configured_json_mcp_route(
        &existing,
        &executable,
        LM_STUDIO_CLIENT_PREFIX,
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_CONFIG_OWNER,
        false,
    )
    .expect("read a pairing an earlier build wrote");
    assert_eq!(
        route.managed_version.as_deref(),
        Some("0.1.100"),
        "the prefix this build strips no longer matches the one on disk, so \
             this app cannot recognise its own pairing"
    );
    let status = local_json_integration_status(
        true,
        None,
        Ok(route),
        "lmStudioInstalled",
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_SUPPORTED_VERSIONS,
    );
    assert_eq!(
        status["state"], "update-available",
        "a pairing this app wrote must read as its own and one release \
             behind, never as a configuration it does not recognise"
    );
    assert_eq!(
        status["integrationInstalled"], true,
        "the row must keep the control that removes an existing pairing"
    );

    // And the route this build writes stays in the same family, so the next
    // release keeps recognising this one's work.
    let written = directory.join("written-mcp.json");
    write_json_mcp_route(
        &written,
        &executable,
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_CONFIG_OWNER,
        false,
    )
    .expect("pair this build");
    let document: Value = serde_json::from_slice(&fs::read(&written).expect("configuration"))
        .expect("valid configuration JSON");
    let announced = document["mcpServers"][CLAUDE_EXTENSION_NAME]["env"][LOCAL_MCP_CLIENT_ENV]
        .as_str()
        .expect("the managed route names its host in the environment");
    assert!(
        announced.starts_with("lm-studio:"),
        "the environment identity left the family every pairing uses: {announced}"
    );
    let arguments = json_route_arguments(&written);
    let named = borrowed(&arguments)
        .windows(2)
        .find(|pair| pair[0] == CLIENT_ID_ARGUMENT)
        .map(|pair| pair[1].to_owned())
        .expect("the managed route names its host on the command line");
    assert!(
        named.starts_with("lm-studio:"),
        "the command-line identity left the family every pairing uses: {named}"
    );

    fs::remove_dir_all(directory).expect("remove the identity fixture");
}

/// A host that is installed, paired, and whose route this build wrote, so
/// the only thing left that can move `state` is the version gate.
fn version_gate_status(
    version: Option<&str>,
    installed_key: &str,
    client_id: &str,
    supported: &[AppVersionSeries],
) -> Value {
    local_json_integration_status(
        true,
        Some(DesktopInstallation {
            executable: None,
            version: version.map(str::to_owned),
        }),
        Ok(ConfiguredJsonMcpRoute {
            configured: true,
            launcher_current: true,
            managed_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            enabled: true,
        }),
        installed_key,
        client_id,
        supported,
    )
}

fn lm_studio_gate(version: &str, supported: &[AppVersionSeries]) -> Value {
    version_gate_status(
        Some(version),
        "lmStudioInstalled",
        LM_STUDIO_CLIENT_ID,
        supported,
    )
}

/// 0.3.17 is the LM Studio release this pairing needs, and an installation
/// still carrying that numbering is still held to it. The rename did not
/// retire the requirement, only the series it was written in.
#[test]
fn a_legacy_lm_studio_below_its_series_minimum_is_still_told_to_update() {
    for version in ["0.3.16", "0.3.16.0", "0.2.99", "0.0.1"] {
        let status = lm_studio_gate(version, LM_STUDIO_SUPPORTED_VERSIONS);
        assert_eq!(status["state"], "app-update-required", "{version}");
        assert_eq!(status["appVersionSeries"], "LM Studio", "{version}");
    }
}

/// The same threshold from the other side - and the component counts are
/// the part that can silently invert it. `executable_version` reads
/// VS_FIXEDFILEINFO, which always yields four numbers, so a real 0.3.17
/// install presents as `0.3.17.0` against a three-number threshold.
/// `compare_versions` zero-extends the shorter side, which is the only
/// reason that is Equal rather than Less; a comparison that treated the
/// missing component as unknown would accuse every legacy install.
#[test]
fn a_legacy_lm_studio_at_or_above_its_series_minimum_is_not_told_to_update() {
    for version in ["0.3.17", "0.3.17.0", "0.3.18", "0.9.0"] {
        let status = lm_studio_gate(version, LM_STUDIO_SUPPORTED_VERSIONS);
        assert_eq!(status["state"], "current", "{version}");
        assert_eq!(status["appVersionSeries"], "LM Studio", "{version}");
    }
}

/// The case the whole table exists for: a real Bionic install.
///
/// `1.1.0.0` is what `executable_version` returns for
/// an installed Bionic, whose VS_FIXEDFILEINFO fixed block zeroes the
/// patch and build fields even though the version string beside it reads
/// `1.1.6+3`. The gate sees `1.1.0.0`, so this is the number it must not
/// reject - and it is also why no Bionic threshold finer than the minor
/// component could ever be satisfied.
#[test]
fn the_bionic_build_this_machine_reports_is_never_told_to_update() {
    let status = lm_studio_gate("1.1.0.0", LM_STUDIO_SUPPORTED_VERSIONS);
    assert_ne!(status["state"], "app-update-required");
    assert_eq!(status["state"], "current");
    assert_eq!(status["appVersionSeries"], "Bionic");
}

/// Both sides of the boundary chosen for the Bionic series.
///
/// It is the floor of the series itself, so every Bionic release clears it;
/// the versions below it are not early Bionic releases but the legacy LM
/// Studio line, and they are judged by the legacy threshold instead.
#[test]
fn the_bionic_series_boundary_separates_the_two_numbering_lines() {
    for version in ["1.0.0", "1.0.0.0", "1.1.0.0", "2.0", "9.9.9.9"] {
        let status = lm_studio_gate(version, LM_STUDIO_SUPPORTED_VERSIONS);
        assert_eq!(status["appVersionSeries"], "Bionic", "{version}");
        assert_eq!(status["state"], "current", "{version}");
    }
    for version in ["0.9.9.9", "0.3.17", "0.3.16"] {
        let status = lm_studio_gate(version, LM_STUDIO_SUPPORTED_VERSIONS);
        assert_eq!(status["appVersionSeries"], "LM Studio", "{version}");
    }
}

/// Proof that the two rows are actually separate, rather than one
/// threshold a Bionic number happens to clear because 1 is greater than 0.
///
/// Raise the legacy minimum to a version LM Studio never shipped: the
/// legacy verdict flips and the Bionic verdict does not move. A single
/// threshold cannot produce that pair of answers.
#[test]
fn raising_the_legacy_minimum_does_not_move_the_bionic_verdict() {
    const RAISED_LEGACY: &[AppVersionSeries] = &[
        AppVersionSeries {
            name: "Bionic",
            series_begins_at: "1.0.0",
            minimum: "1.0.0",
        },
        AppVersionSeries {
            name: "LM Studio",
            series_begins_at: "0",
            minimum: "0.99.0",
        },
    ];
    assert_eq!(
        lm_studio_gate("0.3.17", LM_STUDIO_SUPPORTED_VERSIONS)["state"],
        "current"
    );
    assert_eq!(
        lm_studio_gate("0.3.17", RAISED_LEGACY)["state"],
        "app-update-required"
    );
    assert_eq!(
        lm_studio_gate("1.1.0.0", RAISED_LEGACY)["state"],
        "current",
        "the Bionic row answers for a Bionic version, whatever the legacy row says"
    );
}

/// The gate is permissive about what it cannot read. An unparsable version
/// and an installation that reports none are both unknowns, and an unknown
/// must never become an accusation that the owner update a current app.
#[test]
fn an_unreadable_app_version_is_not_an_accusation() {
    let unparsable = lm_studio_gate("bionic-nightly", LM_STUDIO_SUPPORTED_VERSIONS);
    assert_eq!(unparsable["state"], "current");
    assert!(unparsable["appVersionSeries"].is_null());

    let unreported = version_gate_status(
        None,
        "lmStudioInstalled",
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_SUPPORTED_VERSIONS,
    );
    assert_eq!(unreported["state"], "current");
    assert!(unreported["appVersionSeries"].is_null());
}

/// Jan has renamed nothing and renumbered nothing, so its one row must
/// keep gating exactly as it did - including the 0.8.x it has since
/// reached, which is the same 0.x line the threshold was set for.
#[test]
fn jan_is_gated_on_its_single_numbering_series() {
    let below = version_gate_status(
        Some("0.6.9"),
        "janInstalled",
        JAN_CLIENT_ID,
        JAN_SUPPORTED_VERSIONS,
    );
    assert_eq!(below["state"], "app-update-required");
    assert_eq!(below["appVersionSeries"], "Jan");
    for version in ["0.6.10", "0.6.10.0", "0.7.0", "0.8.4", "0.8.4.0"] {
        let status = version_gate_status(
            Some(version),
            "janInstalled",
            JAN_CLIENT_ID,
            JAN_SUPPORTED_VERSIONS,
        );
        assert_eq!(status["state"], "current", "{version}");
        assert_eq!(status["appVersionSeries"], "Jan", "{version}");
    }
}

#[test]
fn the_jan_route_names_its_host_where_the_host_cannot_drop_it() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("qc-remote.exe");

    write_json_mcp_route(&config, &executable, JAN_CLIENT_ID, "Jan", true).expect("pair Jan");
    let arguments = json_route_arguments(&config);
    assert!(
        arguments_name_client(&borrowed(&arguments), JAN_CLIENT_ID),
        "the written route must name its host on the command line: {arguments:?}"
    );
    assert!(
        configured_json_mcp_route(&config, &executable, "jan:", JAN_CLIENT_ID, "Jan", true)
            .expect("inspect the new route")
            .launcher_current,
        "the route this app just wrote must read as current"
    );

    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                CLAUDE_EXTENSION_NAME: {
                    "command": executable.to_string_lossy(),
                    "args": ["--local-mcp-bridge"],
                    "env": { LOCAL_MCP_CLIENT_ENV: JAN_CLIENT_ID },
                    "active": true
                }
            }
        }))
        .expect("serialize an environment-only pairing"),
    )
    .expect("seed an environment-only pairing");
    let legacy =
        configured_json_mcp_route(&config, &executable, "jan:", JAN_CLIENT_ID, "Jan", true)
            .expect("inspect the legacy route");
    assert!(legacy.configured && legacy.enabled);
    assert!(!legacy.launcher_current);
    assert_eq!(
        legacy.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let status = local_json_integration_status(
        true,
        None,
        Ok(legacy),
        "janInstalled",
        JAN_CLIENT_ID,
        JAN_SUPPORTED_VERSIONS,
    );
    assert_eq!(status["state"], "update-available");
    assert_eq!(status["integrationInstalled"], true);

    fs::remove_dir_all(directory).expect("remove Jan identity fixture");
}

/// Antigravity was the first route to carry the argument, and its reader
/// never required it - so a pairing written by a build that predates the
/// writer read as current forever and was never offered the rewrite.
#[test]
fn the_antigravity_route_names_its_host_where_the_host_cannot_drop_it() {
    let directory = crate::tests::unique_fixture_directory("client-package");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    fs::create_dir_all(&directory).expect("fixture directory");
    let config = directory.join("mcp_config.json");
    let executable = directory.join("qc-remote.exe");
    fs::write(&executable, b"executable fixture").expect("write launcher fixture");

    write_antigravity_config(&config, &executable, ANTIGRAVITY_CLIENT_ID)
        .expect("pair Antigravity");
    let arguments = json_route_arguments(&config);
    assert!(
        arguments_name_client(&borrowed(&arguments), ANTIGRAVITY_CLIENT_ID),
        "the written route must name its host on the command line: {arguments:?}"
    );
    assert!(
        configured_antigravity_route(&config, &executable)
            .expect("inspect the new route")
            .launcher_current,
        "the route this app just wrote must read as current"
    );

    fs::write(
        &config,
        serde_json::to_vec_pretty(&json!({
            "mcpServers": {
                CLAUDE_EXTENSION_NAME: {
                    "command": executable.to_string_lossy(),
                    "args": ["--local-mcp-bridge"],
                    "env": { LOCAL_MCP_CLIENT_ENV: ANTIGRAVITY_CLIENT_ID }
                }
            }
        }))
        .expect("serialize an environment-only pairing"),
    )
    .expect("seed an environment-only pairing");
    let legacy =
        configured_antigravity_route(&config, &executable).expect("inspect the legacy route");
    assert!(legacy.configured && legacy.enabled);
    assert!(!legacy.launcher_current);
    assert_eq!(
        legacy.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let status = antigravity_status_document(
        true,
        &AntigravityIntegration {
            route: legacy,
            permission_current: true,
            legacy_route_present: false,
        },
        &current_device_knowledge_skill(),
        None,
        false,
        observed(false, false),
    );
    assert_eq!(status["state"], "update-available");
    assert_eq!(status["integrationInstalled"], true);

    fs::remove_dir_all(directory).expect("remove Antigravity identity fixture");
}

/// The row that writes nothing still has to be observable, and the block it
/// hands the user is the only place its identity can come from.

#[test]
fn the_pasted_stdio_setup_names_its_host_where_the_host_cannot_drop_it() {
    let setup = other_mcp_stdio_setup().expect("standard STDIO setup");
    let configuration: Value = serde_json::from_str(
        setup["configuration"]
            .as_str()
            .expect("the payload carries a configuration to paste"),
    )
    .expect("the pasted configuration is valid JSON");
    let arguments = configuration["mcpServers"][CLAUDE_EXTENSION_NAME]["args"]
        .as_array()
        .expect("the pasted route declares arguments")
        .iter()
        .filter_map(|argument| argument.as_str())
        .collect::<Vec<_>>();
    assert!(
        arguments_name_client(&arguments, OTHER_MCP_CLIENT_ID),
        "the pasted route must name its host on the command line: {arguments:?}"
    );
    assert_eq!(
        configuration["mcpServers"][CLAUDE_EXTENSION_NAME]["env"][LOCAL_MCP_CLIENT_ENV],
        OTHER_MCP_CLIENT_ID,
        "the environment copy stays for the hosts that do propagate it"
    );
}
/// A scratch directory for one of the route-refresh tests.
fn refresh_fixture(name: &str) -> PathBuf {
    crate::tests::unique_fixture_directory(&format!("client-refresh-{name}"))
}

#[cfg(target_os = "linux")]
#[test]
fn linux_codex_and_xdg_paths_follow_absolute_user_overrides() {
    let home = Path::new("/home/tester");
    assert_eq!(
        codex_config_path_in(home, None),
        Path::new("/home/tester/.codex/config.toml")
    );
    assert_eq!(
        codex_config_path_in(home, Some(Path::new("/mnt/config/codex"))),
        Path::new("/mnt/config/codex/config.toml")
    );
    assert_eq!(
        codex_config_path_in(home, Some(Path::new("relative-codex"))),
        Path::new("/home/tester/.codex/config.toml"),
        "a relative CODEX_HOME must not resolve against the app working directory"
    );
    assert_eq!(
        xdg_directory_from(Some(Path::new("/mnt/config")), home, ".config"),
        Path::new("/mnt/config")
    );
    assert_eq!(
        xdg_directory_from(Some(Path::new("relative")), home, ".config"),
        Path::new("/home/tester/.config"),
        "relative XDG paths are ignored"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_jan_pairing_remains_unavailable_even_if_flatpak_is_installed() {
    assert!(jan_installation().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_jan_uses_documented_xdg_data_location_and_custom_settings_path() {
    let root = refresh_fixture("linux-jan-path");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(root.clone());
    let config_home = root.join("config");
    let data_home = root.join("data");
    assert_eq!(
        jan_data_directory_in(&config_home, &data_home),
        data_home.join("Jan").join("data")
    );

    let jan_config = config_home.join("Jan");
    fs::create_dir_all(&jan_config).expect("create isolated Jan config directory");
    fs::write(
        jan_config.join("settings.json"),
        serde_json::to_vec(&json!({ "data_folder": root.join("custom-data") }))
            .expect("serialize Jan data-folder setting"),
    )
    .expect("write isolated Jan settings");
    assert_eq!(
        jan_data_directory_in(&config_home, &data_home),
        root.join("custom-data")
    );

    fs::write(
        jan_config.join("settings.json"),
        br#"{"data_folder":"relative-data"}"#,
    )
    .expect("write relative path fixture");
    assert_eq!(
        jan_data_directory_in(&config_home, &data_home),
        data_home.join("Jan").join("data"),
        "a relative data_folder must not resolve against the app working directory"
    );
    fs::remove_dir_all(root).expect("remove isolated Jan path fixture");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_codex_detection_only_accepts_executable_absolute_path_entries() {
    use std::os::unix::fs::PermissionsExt;

    let root = refresh_fixture("linux-path-detection");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(root.clone());
    let first = root.join("first");
    let second = root.join("second");
    fs::create_dir_all(&first).expect("create first PATH fixture");
    fs::create_dir_all(&second).expect("create second PATH fixture");
    let non_executable = first.join("codex");
    let executable = second.join("codex");
    fs::write(&non_executable, b"not executable").expect("write non-executable");
    fs::write(&executable, b"executable fixture").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
        .expect("make executable fixture searchable");

    let search_path = std::env::join_paths([&first, &second]).expect("join isolated PATH");
    assert_eq!(where_command_in("codex", &search_path), vec![executable]);
    fs::remove_dir_all(root).expect("remove isolated PATH fixture");
}

/// The four production targets, pointed at a temporary directory instead of
/// the user's profile. Nothing else about them is changed: the same struct
/// `managed_route_targets` builds, so the tests exercise the code that runs.
fn chatgpt_target(directory: &Path) -> ManagedRouteTarget {
    ManagedRouteTarget {
        provider: CHATGPT_ROUTE,
        label: "ChatGPT",
        config_path: directory.join("config.toml"),
        executable_path: directory.join("QC Remote.exe"),
        client_id: CODEX_CLIENT_ID,
        client_prefix: "codex:",
        format: ManagedRouteFormat::Toml,
    }
}

fn antigravity_target(directory: &Path) -> ManagedRouteTarget {
    ManagedRouteTarget {
        provider: ANTIGRAVITY_ROUTE,
        label: "Antigravity",
        config_path: directory.join("mcp_config.json"),
        executable_path: directory.join("QC Remote.exe"),
        client_id: ANTIGRAVITY_CLIENT_ID,
        client_prefix: "antigravity:",
        format: ManagedRouteFormat::Antigravity,
    }
}

fn lm_studio_target(directory: &Path) -> ManagedRouteTarget {
    ManagedRouteTarget {
        provider: LM_STUDIO_ROUTE,
        label: LM_STUDIO_CONFIG_OWNER,
        config_path: directory.join("mcp.json"),
        executable_path: directory.join("QC Remote.exe"),
        client_id: LM_STUDIO_CLIENT_ID,
        client_prefix: LM_STUDIO_CLIENT_PREFIX,
        format: ManagedRouteFormat::Json {
            uses_active_field: false,
        },
    }
}

/// Bionic's `ng-mcp.json`, whose `servers` is an array. The same struct
/// `managed_route_targets` builds for an installation in the 1.x series.
fn bionic_target(directory: &Path) -> ManagedRouteTarget {
    ManagedRouteTarget {
        provider: LM_STUDIO_ROUTE,
        label: LM_STUDIO_CONFIG_OWNER,
        config_path: directory.join("ng-mcp.json"),
        executable_path: directory.join("QC Remote.exe"),
        client_id: LM_STUDIO_CLIENT_ID,
        client_prefix: LM_STUDIO_CLIENT_PREFIX,
        format: ManagedRouteFormat::Bionic,
    }
}

fn jan_target(directory: &Path) -> ManagedRouteTarget {
    ManagedRouteTarget {
        provider: JAN_ROUTE,
        label: "Jan",
        config_path: directory.join("mcp.json"),
        executable_path: directory.join("QC Remote.exe"),
        client_id: JAN_CLIENT_ID,
        client_prefix: "jan:",
        format: ManagedRouteFormat::Json {
            uses_active_field: true,
        },
    }
}

/// Puts one managed server entry into a JSON host's configuration, beside a
/// server the user already had, so every check below also shows that the
/// user's own entry survives whatever the pass decides.
fn seed_json_route(config: &Path, server: Value) {
    let mut servers = serde_json::Map::new();
    servers.insert(
        "their-own-server".to_string(),
        json!({ "command": "their-own-server.exe" }),
    );
    servers.insert(CLAUDE_EXTENSION_NAME.to_string(), server);
    let document = json!({ "mcpServers": Value::Object(servers) });
    fs::write(
        config,
        serde_json::to_vec_pretty(&document).expect("fixture JSON"),
    )
    .expect("seed managed route fixture");
}

/// Pair through the production writer, then ask the unattended pass what it
/// makes of the result.
///
/// Every format has to answer "current" here, or the pass would rewrite a
/// route it had only just written - on every app start, for ever. The
/// proof that nothing was written is the absence of a backup: all three
/// writers copy the live file aside before replacing it, so a backup on
/// disk is a write that happened, and comparing the bytes alone would pass
/// a rewrite that produced the same text.
fn assert_a_freshly_paired_route_is_left_alone(target: &ManagedRouteTarget) {
    target
        .rewrite()
        .expect("pair through the production writer");
    let before = fs::read(&target.config_path).expect("paired configuration");
    let outcome = refresh_managed_route(target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Current,
        "{} pairs at this build, so there is nothing to freshen",
        target.label
    );
    assert!(outcome.failure.is_none());
    assert_eq!(
        fs::read(&target.config_path).expect("configuration after the pass"),
        before,
        "{}'s configuration was rewritten when it was already current",
        target.label
    );
    assert!(
        config_backups(&target.config_path).is_empty(),
        "{}'s configuration was written: a backup only exists because a writer ran",
        target.label
    );
}

#[test]
fn a_route_that_already_names_this_build_is_never_rewritten() {
    let directory = refresh_fixture("current");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    assert_a_freshly_paired_route_is_left_alone(&chatgpt_target(&directory.join("codex")));
    assert_a_freshly_paired_route_is_left_alone(&antigravity_target(
        &directory.join("antigravity"),
    ));
    assert_a_freshly_paired_route_is_left_alone(&lm_studio_target(&directory.join("lm-studio")));
    assert_a_freshly_paired_route_is_left_alone(&bionic_target(&directory.join("bionic")));
    assert_a_freshly_paired_route_is_left_alone(&jan_target(&directory.join("jan")));
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_stale_route_this_app_wrote_is_brought_up_to_date_without_being_asked() {
    let directory = refresh_fixture("stale");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = lm_studio_target(&directory);
    seed_json_route(
        &target.config_path,
        json_mcp_route_server(&target.executable_path, "lm-studio:0.1.100", false),
    );

    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        }
    );
    assert!(outcome.failure.is_none());
    assert_eq!(outcome.report()["outcome"], json!("rewritten"));
    assert_eq!(outcome.report()["pairedVersion"], json!("0.1.100"));

    // The app's own route reader is what the panel asks, so ask it: the row
    // that said "Update available" says "current" now, with nobody having
    // pressed anything.
    let route = configured_json_mcp_route(
        &target.config_path,
        &target.executable_path,
        "lm-studio:",
        LM_STUDIO_CLIENT_ID,
        "LM Studio",
        false,
    )
    .expect("read the freshened route");
    assert!(route.configured && route.launcher_current && route.enabled);
    assert_eq!(
        route.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );

    let document: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("freshened config"))
            .expect("valid freshened config");
    assert_eq!(
        document["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server.exe"),
        "a route the user configured themselves must survive the rewrite"
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_stale_chatgpt_route_is_freshened_through_its_own_toml_writer() {
    let directory = refresh_fixture("stale-chatgpt");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = chatgpt_target(&directory);
    fs::write(&target.config_path, "model = \"keep-me\"\n").expect("seed Codex configuration");
    write_codex_config(
        &target.config_path,
        &target.executable_path,
        "codex:0.1.100",
    )
    .expect("seed a route this app wrote one release ago");

    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        }
    );
    assert!(outcome.failure.is_none());

    let route = configured_chatgpt_route(&target.config_path, &target.executable_path)
        .expect("read the freshened ChatGPT route");
    assert!(route.configured && route.launcher_current && route.approval_mode_current);
    assert_eq!(
        route.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let contents = fs::read_to_string(&target.config_path).expect("freshened Codex config");
    assert!(
        contents.contains("model = \"keep-me\""),
        "the rest of the user's Codex configuration must survive the rewrite"
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_stale_antigravity_route_is_freshened_through_its_own_writer() {
    let directory = refresh_fixture("stale-antigravity");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = antigravity_target(&directory);
    seed_json_route(
        &target.config_path,
        antigravity_route_server(&target.executable_path, "antigravity:0.1.100"),
    );

    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        }
    );
    let route = configured_antigravity_route(&target.config_path, &target.executable_path)
        .expect("read the freshened Antigravity route");
    assert_eq!(
        route.managed_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    // `launcher_current` additionally requires the paired executable to
    // exist, which a temporary fixture has no reason to; the identity is
    // what this pass owns and what the row keys its version on.
    assert!(route.configured && route.enabled);
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_route_pointing_at_another_command_is_left_completely_alone() {
    let directory = refresh_fixture("foreign-command");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());

    // Ours in every other respect - our arguments, our identity, a version
    // this build is newer than - and a command that is not this app.
    fs::create_dir_all(directory.join("lm-studio")).expect("LM Studio fixture directory");
    let lm_studio = lm_studio_target(&directory.join("lm-studio"));
    let server = json_mcp_route_server(
        Path::new("C:\\Their\\own-bridge.exe"),
        "lm-studio:0.1.100",
        false,
    );
    seed_json_route(&lm_studio.config_path, server);
    let before = fs::read(&lm_studio.config_path).expect("seeded configuration");
    let outcome = refresh_managed_route(&lm_studio);
    assert_eq!(outcome.decision, RouteRefreshDecision::Customised);
    assert_eq!(outcome.report()["outcome"], json!("customised"));
    assert_eq!(
        fs::read(&lm_studio.config_path).expect("configuration after the pass"),
        before,
        "a command that is not this app's makes the route somebody else's"
    );
    assert!(config_backups(&lm_studio.config_path).is_empty());

    // The same fact in the TOML host, written by this app's own writer
    // against a different executable.
    let chatgpt = chatgpt_target(&directory.join("codex"));
    write_codex_config(
        &chatgpt.config_path,
        Path::new("C:\\Their\\own-bridge.exe"),
        "codex:0.1.100",
    )
    .expect("seed a route naming another command");
    let before = fs::read(&chatgpt.config_path).expect("seeded Codex configuration");
    let outcome = refresh_managed_route(&chatgpt);
    assert_eq!(outcome.decision, RouteRefreshDecision::Customised);
    assert_eq!(
        fs::read(&chatgpt.config_path).expect("Codex configuration after the pass"),
        before
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_managed_route_with_edited_arguments_is_repaired_and_other_servers_survive() {
    let directory = refresh_fixture("customised-args");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = lm_studio_target(&directory);
    let mut server = json_mcp_route_server(&target.executable_path, "lm-studio:0.1.100", false);
    server["args"] = json!([
        "--local-mcp-bridge",
        CLIENT_ID_ARGUMENT,
        "lm-studio:0.1.100",
        "--their-own-flag"
    ]);
    seed_json_route(&target.config_path, server);
    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        }
    );
    let document: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("repaired configuration"))
            .expect("valid repaired configuration");
    assert_eq!(
        document["mcpServers"][CLAUDE_EXTENSION_NAME]["args"],
        json!(["--local-mcp-bridge", CLIENT_ID_ARGUMENT, target.client_id])
    );
    assert_eq!(
        document["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server.exe")
    );
    assert_eq!(config_backups(&target.config_path).len(), 1);
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_managed_route_disabled_in_the_host_is_restored_by_startup_repair() {
    let directory = refresh_fixture("disabled");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = lm_studio_target(&directory);
    let mut server = json_mcp_route_server(&target.executable_path, "lm-studio:0.1.100", false);
    server["disabled"] = Value::Bool(true);
    seed_json_route(&target.config_path, server);
    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        },
    );
    let document: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("repaired configuration"))
            .expect("valid repaired configuration");
    assert_eq!(
        document["mcpServers"][CLAUDE_EXTENSION_NAME]["disabled"],
        Value::Bool(false)
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_managed_route_with_a_moved_executable_path_is_repaired_at_the_same_version() {
    let directory = refresh_fixture("moved-executable-path");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = lm_studio_target(&directory);
    let old_path = PathBuf::from("E:\\QC Remote\\QC Remote.exe");
    seed_json_route(
        &target.config_path,
        json_mcp_route_server(
            &old_path,
            &format!("{}{}", target.client_prefix, env!("CARGO_PKG_VERSION")),
            false,
        ),
    );

    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: env!("CARGO_PKG_VERSION").to_string()
        }
    );
    let document: Value =
        serde_json::from_slice(&fs::read(&target.config_path).expect("repaired configuration"))
            .expect("valid repaired configuration");
    assert_eq!(
        document["mcpServers"][CLAUDE_EXTENSION_NAME]["command"],
        target.executable_path.to_string_lossy().as_ref()
    );
    assert_eq!(
        document["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server.exe")
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn an_unpaired_provider_is_not_paired_by_the_unattended_pass() {
    let directory = refresh_fixture("unpaired");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());

    // A host that has never been paired has no configuration at all.
    let missing = lm_studio_target(&directory.join("missing"));
    fs::create_dir_all(directory.join("missing")).expect("missing-host directory");
    assert_eq!(
        refresh_managed_route(&missing).decision,
        RouteRefreshDecision::NotPaired
    );
    assert!(
        !missing.config_path.exists(),
        "checking an unpaired host must not create its configuration"
    );

    // And one the user configured for other servers only.
    let other = lm_studio_target(&directory.join("other-servers"));
    fs::create_dir_all(directory.join("other-servers")).expect("other-servers directory");
    let document = json!({ "mcpServers": { "their-own-server": { "command": "theirs.exe" } } });
    fs::write(
        &other.config_path,
        serde_json::to_vec_pretty(&document).expect("fixture JSON"),
    )
    .expect("seed an unpaired configuration");
    let before = fs::read(&other.config_path).expect("seeded configuration");
    assert_eq!(
        refresh_managed_route(&other).decision,
        RouteRefreshDecision::NotPaired
    );
    assert_eq!(
        fs::read(&other.config_path).expect("configuration after the pass"),
        before,
        "an unpaired host is not paired by a check"
    );
    assert!(config_backups(&other.config_path).is_empty());
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_route_naming_a_newer_build_than_this_one_is_not_downgraded() {
    let directory = refresh_fixture("newer");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = lm_studio_target(&directory);
    seed_json_route(
        &target.config_path,
        json_mcp_route_server(&target.executable_path, "lm-studio:99.0.0", false),
    );
    let before = fs::read(&target.config_path).expect("seeded configuration");

    let outcome = refresh_managed_route(&target);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::PairedWithNewerBuild {
            paired: "99.0.0".to_string()
        }
    );
    assert_eq!(
        fs::read(&target.config_path).expect("configuration after the pass"),
        before,
        "an older app must not quietly relabel a newer app's route as its own"
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn the_unattended_pass_never_reaches_claude_desktop() {
    let targets = managed_route_targets().expect("resolve the managed routes");
    assert_eq!(
        targets
            .iter()
            .map(|target| target.provider)
            .collect::<Vec<_>>(),
        vec![CHATGPT_ROUTE, ANTIGRAVITY_ROUTE, LM_STUDIO_ROUTE, JAN_ROUTE],
        "these four config-file routes are the whole of what this app rewrites unattended"
    );
    assert!(
        !targets
            .iter()
            .any(|target| target.client_prefix.starts_with("claude")),
        "Claude Desktop pairs by installing a .mcpb extension through Claude's own \
             installer UI, which must stay something the owner starts"
    );
    assert!(
        !targets.iter().any(|target| target
            .config_path
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("claude")),
        "no unattended write may land in a Claude Desktop directory"
    );
}

#[test]
fn a_refused_rewrite_leaves_the_previous_route_working_and_is_reported_once_per_reason() {
    let directory = refresh_fixture("refused-write");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = ManagedRouteTarget {
        // A key of this pass's own, so recording an outcome here cannot
        // disturb another test's reading of the shared status record.
        provider: "test-only-refused-write",
        ..lm_studio_target(&directory)
    };
    seed_json_route(
        &target.config_path,
        json_mcp_route_server(&target.executable_path, "lm-studio:0.1.100", false),
    );
    let before = fs::read(&target.config_path).expect("the route already on disk");
    // A directory where the writer stages its replacement: the write is
    // refused before anything reaches the live path.
    let pinned = PinnedStageSequence::new();
    fs::create_dir(staged_config_path(
        &directory,
        &target.config_path,
        "qc-remote",
    ))
    .expect("block the staged write");

    let outcome = refresh_managed_route(&target);
    drop(pinned);
    assert_eq!(
        outcome.decision,
        RouteRefreshDecision::Stale {
            paired: "0.1.100".to_string()
        }
    );
    let failure = outcome
        .failure
        .clone()
        .expect("a refused rewrite must never be swallowed");
    assert_eq!(
        fs::read(&target.config_path).expect("the route after the refused write"),
        before,
        "losing a working pairing while freshening it is worse than leaving it stale"
    );
    assert_eq!(outcome.report()["outcome"], json!("failed"));
    assert_eq!(outcome.report()["error"], json!(&failure));

    // One line when it breaks, one when the reason changes, and silence
    // while the same reason repeats.
    assert!(record_route_refresh(&target, &outcome));
    assert!(
        !record_route_refresh(&target, &outcome),
        "the same reason must not be announced twice"
    );
    assert_eq!(
        route_refresh_report(target.provider)["error"],
        json!(&failure),
        "and the status carries the reason whether or not a line was printed"
    );

    // The reason is the rate limiter's key, so anything in it that varies
    // per attempt turns the policy back into a line per attempt.
    assert!(
        !failure.contains(&std::process::id().to_string()),
        "the failure names the staged file's process id: {failure}"
    );

    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

#[test]
fn a_configuration_this_app_cannot_read_is_reported_and_left_untouched() {
    let directory = refresh_fixture("unreadable");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let target = lm_studio_target(&directory);
    let invalid = "{ this is not JSON";
    fs::write(&target.config_path, invalid).expect("seed an invalid configuration");

    let outcome = refresh_managed_route(&target);
    assert_eq!(outcome.decision, RouteRefreshDecision::Unreadable);
    let failure = outcome
        .failure
        .clone()
        .expect("a configuration this app cannot read is not swallowed either");
    assert_eq!(outcome.report()["outcome"], json!("unreadable"));
    assert_eq!(outcome.report()["error"], json!(&failure));
    assert_eq!(
        fs::read_to_string(&target.config_path).expect("configuration after the pass"),
        invalid,
        "a configuration this app cannot parse is not a configuration it may replace"
    );
    fs::remove_dir_all(directory).expect("remove route refresh fixture");
}

/// Names the next staged file on this thread in advance, until dropped.
struct PinnedStageSequence;

impl PinnedStageSequence {
    fn new() -> Self {
        PINNED_CONFIG_STAGE_SEQUENCE.with(|pinned| pinned.set(Some(u64::MAX)));
        Self
    }
}

impl Drop for PinnedStageSequence {
    fn drop(&mut self) {
        PINNED_CONFIG_STAGE_SEQUENCE.with(|pinned| pinned.set(None));
    }
}

/// Every staged file still in `directory`.
fn staged_leftovers(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .expect("fixture directory")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

/// Two writers in one process used to stage into the same
/// `.{file}.qc-remote.{pid}.tmp`: one replaced the other's staged bytes,
/// and the second rename then failed with "Could not install" over a write
/// that had landed. Nothing serialised them either, so their backups and
/// renames interleaved freely.
#[test]
fn concurrent_route_writers_in_one_process_never_collide() {
    let directory = refresh_fixture("concurrent-writers");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = directory.join("mcp.json");
    let executable = directory.join("QC Remote.exe");
    seed_json_route(
        &config,
        json_mcp_route_server(&executable, "lm-studio:0.1.100", false),
    );
    const WRITERS: usize = 8;
    const ROUNDS: usize = 25;
    let failures = std::thread::scope(|scope| {
        let start = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let arrived = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Declared inside the scope so unwind releases workers BEFORE its join.
        let _release_on_unwind = ClientFixtureRelease(start.clone());
        let writers = (0..WRITERS)
            .map(|index| {
                let start = start.clone();
                let arrived = arrived.clone();
                let config = config.clone();
                let executable = executable.clone();
                scope.spawn(move || {
                    arrived.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    wait_client_fixture_gate(&start);
                    (0..ROUNDS)
                        .filter_map(|round| {
                            let result = if (index + round) % 3 == 0 {
                                remove_json_mcp_route(&config, "Test").map(|_| ())
                            } else {
                                write_json_mcp_route(
                                    &config,
                                    &executable,
                                    LM_STUDIO_CLIENT_ID,
                                    "Test",
                                    false,
                                )
                            };
                            result.err()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(5);
        while arrived.load(std::sync::atomic::Ordering::Acquire) != WRITERS {
            assert!(
                Instant::now() < deadline,
                "client fixture workers did not arrive"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        start.store(true, std::sync::atomic::Ordering::Release);
        writers
            .into_iter()
            .flat_map(|writer| writer.join().expect("writer thread"))
            .collect::<Vec<_>>()
    });
    assert!(
        failures.is_empty(),
        "concurrent writers reported failures over writes that had landed: {failures:?}"
    );
    let document = parse_json_mcp_config(
        &fs::read_to_string(&config).expect("the configuration after the writers"),
        "Test",
    )
    .expect("the configuration is still one whole document");
    assert_eq!(
        document["mcpServers"]["their-own-server"]["command"],
        json!("their-own-server.exe"),
        "the user's own server survives every interleaving"
    );
    assert!(
        staged_leftovers(&directory).is_empty(),
        "no staged file outlives the writers: {:?}",
        staged_leftovers(&directory)
    );
    fs::remove_dir_all(directory).expect("remove concurrent writer fixture");
}

/// The start-up refresh used to read a stale route, decide to
/// rewrite it, and write it back - so an Unpair that landed between the
/// read and the write was undone, and a removal the owner watched succeed
/// came back on its own. The refresh now takes the configuration lock
/// before its read, so it waits for an Unpair in flight and then finds
/// nothing to freshen.
#[test]
fn an_unpair_in_flight_is_never_undone_by_the_start_up_refresh() {
    let directory = refresh_fixture("unpair-during-refresh");
    let _fixture_guard = crate::tests::RemoveDirOnDrop::new(directory.clone());
    let config = lm_studio_target(&directory).config_path;
    seed_json_route(
        &config,
        json_mcp_route_server(
            &lm_studio_target(&directory).executable_path,
            "lm-studio:0.1.100",
            false,
        ),
    );

    // The owner's Unpair is mid-flight: it holds the lock while it works.
    let outcome = std::thread::scope(|scope| {
        // This lock must live inside the scope: an assertion panic drops it
        // before scope joins the refresh waiting for that same lock.
        let unpair = config_write_lock();
        let refresh = scope.spawn({
            let directory = directory.clone();
            move || {
                let target = ManagedRouteTarget {
                    provider: "test-only-unpair-during-refresh",
                    ..lm_studio_target(&directory)
                };
                refresh_managed_route(&target)
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !refresh.is_finished(),
            "the refresh must wait for an Unpair already in flight"
        );
        // Re-entrant on this thread, as the refresh's own rewrite is.
        assert!(remove_json_mcp_route(&config, LM_STUDIO_CONFIG_OWNER).expect("unpair"));
        drop(unpair);

        refresh.join().expect("refresh thread")
    });
    assert_eq!(outcome.decision, RouteRefreshDecision::NotPaired);
    assert_eq!(outcome.failure, None);
    let document = parse_json_mcp_config(
        &fs::read_to_string(&config).expect("the configuration after the refresh"),
        "Test",
    )
    .expect("configuration");
    assert!(
        document["mcpServers"].get(CLAUDE_EXTENSION_NAME).is_none(),
        "the refresh wrote a removed route back: {document}"
    );
    fs::remove_dir_all(directory).expect("remove unpair-during-refresh fixture");
}

struct ClientFixtureRelease(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for ClientFixtureRelease {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}
fn wait_client_fixture_gate(start: &std::sync::atomic::AtomicBool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !start.load(std::sync::atomic::Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "client fixture gate was not released"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn partial_client_fixture_setup_unwind_releases_and_joins_its_started_writer() {
    let completed = std::sync::atomic::AtomicBool::new(false);
    let failed = std::panic::catch_unwind(|| {
        std::thread::scope(|scope| {
            let start = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let _release_on_unwind = ClientFixtureRelease(start.clone());
            let started = start.clone();
            let completed = &completed;
            scope.spawn(move || {
                wait_client_fixture_gate(&started);
                completed.store(true, std::sync::atomic::Ordering::Release);
            });
            panic!("injected next client worker setup failure");
        });
    });
    assert!(failed.is_err());
    assert!(completed.load(std::sync::atomic::Ordering::Acquire));
}

/// A wedged tool is killed at the deadline instead of holding
/// a blocking-pool thread, and the status call with it, indefinitely.
#[cfg(target_os = "windows")]
#[test]
fn an_external_tool_that_never_answers_is_killed_at_the_deadline() {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let started = Instant::now();
    let result = run_with_timeout(
        Command::new(crate::system_tools::powershell_exe())
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 60",
            ])
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW),
        Duration::from_millis(500),
    );
    let error = result.expect_err("a tool past its deadline must not report an answer");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the call waited {:?} on a tool given 500 ms",
        started.elapsed()
    );
}

#[cfg(target_os = "windows")]
#[test]
fn an_external_tool_that_answers_returns_its_output_and_status() {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = run_with_timeout(
        Command::new(crate::system_tools::where_exe())
            .arg("where.exe")
            .current_dir(crate::system_tools::system32_directory())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_TIMEOUT,
    )
    .expect("where.exe answers");
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("where.exe"),
        "standard output is captured: {output:?}"
    );
    let missing = run_with_timeout(
        Command::new(crate::system_tools::where_exe())
            .arg("qc-remote-no-such-tool.exe")
            .current_dir(crate::system_tools::system32_directory())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_TIMEOUT,
    )
    .expect("where.exe answers for a missing name too");
    assert!(
        !missing.status.success(),
        "the exit status is the tool's own"
    );
    assert!(
        !missing.stderr.is_empty(),
        "a piped standard error is captured"
    );
}

/// Every wait on an external tool in this module goes through
/// `run_with_timeout`; a bare `.output()` or `.status()` waits for ever.
#[test]
fn no_status_read_waits_on_an_external_tool_without_a_deadline() {
    let source = include_str!("client_packages.rs");
    let production = source
        .find("\n#[path = \"client_packages_tests.rs\"]\nmod tests;")
        .map(|end| &source[..end])
        .expect("the moved test module marker");
    let unbounded = production
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            line.contains(".output()") || line.starts_with(".status()")
        })
        .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
        .collect::<Vec<_>>();
    assert!(
        unbounded.is_empty(),
        "external tools waited on without a deadline: {unbounded:?}"
    );
}

#[cfg(target_os = "macos")]
mod macos_application_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "qc-macos-app-search-{}-{sequence}-{nonce}",
                std::process::id(),
            ));
            std::fs::create_dir(&path).expect("create isolated app-search fixture");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove app-search fixture");
        }
    }

    fn write_bundle(root: &Path, executable: &str) -> PathBuf {
        let bundle = root.join("Claude.app");
        let contents = bundle.join("Contents");
        let executable_directory = contents.join("MacOS");
        std::fs::create_dir_all(&executable_directory).expect("create app bundle directories");
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>{executable}</string><key>CFBundleShortVersionString</key><string>1.2.3</string></dict></plist>"
            ),
        )
        .expect("write app bundle metadata");
        let executable_path = executable_directory.join(executable);
        if Path::new(executable)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            std::fs::write(&executable_path, b"test executable").expect("write app executable");
        }
        executable_path
    }

    #[test]
    fn finds_a_user_installed_app_bundle_and_reads_its_version() {
        let scratch = Scratch::new();
        let executable = write_bundle(&scratch.0, "Claude");

        let installation =
            macos_application_installation_in(&CLAUDE_DESKTOP_SEARCH, &[scratch.0.clone()])
                .expect("find the app bundle");

        assert_eq!(installation.executable, Some(executable));
        assert_eq!(installation.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn refuses_app_bundle_executable_paths_that_escape_the_bundle() {
        let scratch = Scratch::new();
        write_bundle(&scratch.0, "../../outside");

        assert!(
            macos_application_installation_in(&CLAUDE_DESKTOP_SEARCH, &[scratch.0.clone()])
                .is_none()
        );
        assert!(!scratch.0.join("outside").is_file());
    }
}
