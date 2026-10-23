use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(target_os = "windows")]
use std::ffi::c_void;
use std::{
    cell::Cell,
    cmp::Ordering,
    collections::BTreeMap,
    fs,
    io::Cursor,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    sync::{atomic::AtomicU64, Mutex, MutexGuard, OnceLock, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::local_access::{
    antigravity_bridge_connected, antigravity_bridge_evidence, local_client_evidence,
    should_report_failure, LocalClientEvidence,
};
use crate::local_stdio::CLIENT_ID_ARGUMENT;

mod device_knowledge_skill;
use device_knowledge_skill::*;
#[cfg(windows)]
mod owned_tool;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
use std::process::Stdio;

const CLAUDE_EXTENSION_NAME: &str = "qc-remote";
// MCPB derives the installed extension id from author + name. These are
// persistent identifiers, NOT release branding: changing either installs a
// second server instead of updating local.mcpb.freevia-eood.qc-remote.
const CLAUDE_EXTENSION_AUTHOR: &str = "Freevia EOOD";
const CLAUDE_EXTENSION_FILE: &str = "QC-Remote-Claude.mcpb";
const LEGACY_CHATGPT_RUNNER_FILE: &str = "chatgpt-local-mcp.ps1";
// The Windows build is distributed through the Microsoft Store, whose policy
// 10.1.5 forbids promoting software acquired outside it. These download
// destinations, and the commands that open them, exist on the other platforms
// only.
#[cfg(not(windows))]
const ANTIGRAVITY_DOWNLOAD_URL: &str = "https://antigravity.google/download";
#[cfg(not(windows))]
const LM_STUDIO_DOWNLOAD_URL: &str = "https://lmstudio.ai/download";
#[cfg(not(windows))]
const JAN_DOWNLOAD_URL: &str = "https://jan.ai/download";
const ANTIGRAVITY_MCP_PERMISSION: &str = "mcp(qc-remote/*)";
const LOCAL_MCP_CLIENT_ENV: &str = "QC_REMOTE_LOCAL_MCP_CLIENT";
// Public managed-route metadata, never a credential or authorization input.
const LOCAL_MCP_CLIENT_HEADER: &str = "X-QC-Remote-Client";
const ANTIGRAVITY_CLIENT_ID: &str = concat!("antigravity:", env!("CARGO_PKG_VERSION"));
/// The shared ChatGPT Desktop / Codex route used to carry no client identity at
/// all, so its "Connected" light was really "some client touched /rpc": a
/// single tool call from Antigravity, LM Studio, Jan or any other host lit it
/// up and suppressed its restart prompt. The managed route now names itself
/// like every other provider. (The panel no longer has a "Restart needed"
/// row at all - telling the owner to restart a host was the wrong answer and
/// was removed; the flag this describes now only changes a clause in the
/// paired row's sentence.)
const CODEX_CLIENT_ID: &str = concat!("codex:", env!("CARGO_PKG_VERSION"));
/// Claude Desktop's `.mcpb` runs the Node bridge. The identity is stamped into
/// the extension manifest so the per-client liveness map can key on it as soon
/// as that bridge reports a heartbeat.
const CLAUDE_CLIENT_ID: &str = concat!("claude:", env!("CARGO_PKG_VERSION"));
/// The identity half of the LM Studio / Bionic pairing - an identity, not a
/// label, which is why the rename to Bionic did not touch it.
///
/// The prefix is written into the route's `--local-mcp-client` argument and its
/// `QC_REMOTE_LOCAL_MCP_CLIENT`, it is what [`configured_json_mcp_route`]
/// strips to recover the version a pairing was written by, and the full id is
/// the key the durable attach history in `local_access.rs` records a host's
/// bridge under. Every pairing already on disk carries `lm-studio:`.
///
/// **Do not rename it with the label.** `managed_version` would come back
/// `None`, so this app would read its own working route as
/// `different-configuration` - a stranger's file - and the attach history that
/// keeps a paired row calm between chats would be orphaned. The row's name is
/// derived from `appVersionSeries` instead, in
/// `local-integration-presentation.ts`, and
/// `renaming_the_row_must_not_rename_the_client_identity_pairings_are_keyed_by`
/// fails if the two are ever moved together.
const LM_STUDIO_CLIENT_PREFIX: &str = "lm-studio:";
const LM_STUDIO_CLIENT_ID: &str = concat!("lm-studio:", env!("CARGO_PKG_VERSION"));
/// Whose configuration file these writers name when one of them fails.
///
/// Deliberately *not* renamed to Bionic. This string only ever appears as a
/// possessive in front of "MCP configuration", and the file it is possessive
/// about is `%USERPROFILE%\.lmstudio\mcp.json` - a path the rename did not
/// move, whose owner is the company, which is still called LM Studio (it is
/// `CompanyName` in Bionic's own version resource). One of these sentences can
/// reach the row, quoted, when the start-up route refresh fails; naming Bionic
/// there would point the reader at a directory that does not exist.
const LM_STUDIO_CONFIG_OWNER: &str = "LM Studio";
/// The numbering series Bionic ships, named once so the version table, the
/// route family and the row's label are one reading rather than three strings
/// that can drift apart. `local-integration-presentation.ts` compares the
/// `appVersionSeries` this produces against the *legacy* name for the same
/// reason, and `a_bionic_install_and_a_legacy_install_read_different_documents`
/// pins the pair.
const BIONIC_SERIES_NAME: &str = "Bionic";
/// Bionic's `connection.type` for a native child process. The only value this
/// app ever writes: the bridge is a STDIO server and nothing else.
const BIONIC_STDIO_CONNECTION_TYPE: &str = "stdio";
/// The `id` this app mints for its own entry in Bionic's document.
///
/// Bionic generates a UUID per server it creates, so the key has to be *some*
/// UUID. It must not be a fresh one per write: a new UUID
/// on every Pair and on every start-up refresh is how a file grows a duplicate
/// QC Remote server per launch, which the owner would read as this panel
/// corrupting his configuration. A constant is stable by construction, needs no
/// generator, and makes the written document a pure function of the executable
/// and the client id - which is what lets the start-up refresh rebuild it and
/// compare.
///
/// It is a default, not an assertion of ownership: when the document already
/// holds this app's entry, [`write_bionic_mcp_route`] carries that entry's own
/// `id` forward, whatever it is, because Bionic minted it and may key its own
/// per-server state on it.
const BIONIC_MANAGED_SERVER_ID: &str = "5f0b6a1e-9c3d-4f47-b8a2-1d4e7c6b0a93";
const JAN_CLIENT_ID: &str = concat!("jan:", env!("CARGO_PKG_VERSION"));
const OTHER_MCP_CLIENT_ID: &str = concat!("other:", env!("CARGO_PKG_VERSION"));
const ANTIGRAVITY_BRIDGE_PREFIX: &str = "qc-remote-antigravity-bridge-";
const WINDOWS_APP_IDENTIFIER: &str = "org.freevia.qcremote";
const ANTIGRAVITY_REFRESH_RPC_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RefreshMcpServers";
const ANTIGRAVITY_STATE_RPC_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/GetMcpServerStates";

/// The longest a status read may wait on `reg.exe`, `where.exe` or PowerShell.
///
/// Every one of them answers in well under a second on a healthy machine. A
/// wedged WMI or Appx service used to hold a blocking-pool thread, and the
/// panel's status call with it, for as long as the service stayed wedged.
#[cfg(windows)]
const EXTERNAL_TOOL_TIMEOUT: Duration = Duration::from_secs(10);
const ANTIGRAVITY_OPERATION_BUDGET: Duration = Duration::from_secs(10);
#[cfg(any(windows, test))]
const ANTIGRAVITY_MAX_PORTS: usize = 16;
thread_local! { static TOOL_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) }; }
fn with_tool_deadline<T>(deadline: Instant, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TOOL_DEADLINE.with(|current| current.set(self.0));
        }
    }
    let previous = TOOL_DEADLINE.with(|current| current.replace(Some(deadline)));
    let _restore = Restore(previous);
    action()
}
fn remaining(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("Google Antigravity refresh reached its total operation deadline.".into())
    } else {
        Ok(left)
    }
}
async fn within_antigravity_budget<T>(
    deadline: Instant,
    operation: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(remaining(deadline)?, operation)
        .await
        .map_err(|_| {
            "Google Antigravity refresh reached its total operation deadline.".to_string()
        })?
}
#[cfg(any(windows, test))]
fn validate_antigravity_ports(ports: &[u16]) -> Result<(), String> {
    if ports.is_empty()
        || ports.len() > ANTIGRAVITY_MAX_PORTS
        || ports.contains(&0)
        || ports
            .iter()
            .enumerate()
            .any(|(index, port)| ports[..index].contains(port))
    {
        Err("The running Antigravity service returned invalid local endpoints.".into())
    } else {
        Ok(())
    }
}
// Five provider rows may refresh together; eight original actors provide
// bounded headroom without making a healthy initial panel race its own cap.
// Synchronous OS I/O may outlive its caller, but never its original owner.
static CLIENT_INSPECTION_BUDGET: qc_native_platform::thread_owner::Budget =
    qc_native_platform::thread_owner::Budget::new(8);
const CLIENT_INSPECTION_TIMEOUT: Duration = Duration::from_secs(10);
async fn inspect_client_until<T: Send + 'static>(
    deadline: Instant,
    action: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let result = crate::owned_blocking::run(
        "qc-client-inspection",
        &CLIENT_INSPECTION_BUDGET,
        deadline,
        move |context| {
            if !context.current() {
                return Err("Client inspection expired before discovery".into());
            }
            with_tool_deadline(deadline, action)
        },
    )
    .await
    .map_err(|error| format!("Client inspection failed: {error}"))??;
    // Discovery helpers historically swallow individual lookup failure as
    // absence. Never turn a total deadline into a false not-installed answer.
    if Instant::now() >= deadline {
        return Err("Client inspection reached its total operation deadline".into());
    }
    Ok(result)
}
async fn discover_antigravity_until(
    deadline: Instant,
) -> Result<RunningAntigravityLanguageServer, String> {
    inspect_client_until(deadline, running_antigravity_language_server).await
}

/// Runs `command` to completion like [`Command::output`], but gives up after
/// `timeout`: the child is killed and the call returns `ErrorKind::TimedOut`,
/// which every caller already treats as "the tool gave no answer".
///
/// Standard output is always captured. Standard error is captured only when
/// the caller asked for a pipe; a caller that set it to null keeps it null.
/// Windows pipe pumps share the process deadline; no detached reader threads.
/// Descendants are contained before execution and retired even on normal exit.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn run_with_timeout(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let timeout = TOOL_DEADLINE.with(|deadline| {
        deadline
            .get()
            .map(|deadline| timeout.min(deadline.saturating_duration_since(Instant::now())))
            .unwrap_or(timeout)
    });
    if timeout.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "status-tool operation deadline expired before launch",
        ));
    }
    #[cfg(windows)]
    {
        owned_tool::run(command, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (command, timeout);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Windows status-tool inspection is unavailable on this platform",
        ))
    }
}

#[derive(Debug, serde::Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
#[serde(rename_all = "camelCase")]
struct RunningAntigravityLanguageServer {
    executable_path: PathBuf,
    version: String,
    csrf_token: String,
    ports: Vec<u16>,
}

// A software envelope for local third-party configuration/manifest documents,
// not a vendor schema or firmware maximum. Bound actual held-stream bytes,
// including growth after open; metadata alone is not an allocation limit.
const CLIENT_DOCUMENT_MAX_BYTES: usize = 16 * 1024 * 1024;

fn read_client_document_from(reader: impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut bounded = std::io::Read::take(reader, (CLIENT_DOCUMENT_MAX_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut bounded, &mut bytes)?;
    if bytes.len() > CLIENT_DOCUMENT_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Client configuration exceeded its 16 MiB software document envelope",
        ));
    }
    Ok(bytes)
}

fn read_client_document_bytes(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    read_client_document_from(fs::File::open(path)?)
}

fn read_client_document_text(path: impl AsRef<Path>) -> std::io::Result<String> {
    String::from_utf8(read_client_document_bytes(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

const BRIDGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/local-clients/bridge.cjs"
));
/// QC Remote rewrites this slot in the bridge with the tool contract digest the
/// app was built from, so an installed client can detect skew against a newer app.
const CONTRACT_SLOT: &str = "__QC_REMOTE_TOOL_CONTRACT_SHA256__";
const VERSION_SLOT: &str = "__QC_REMOTE_APP_VERSION__";
/// The one canonical on-disk bridge every manual client configuration points at.
/// The installer and both example configurations are generated from these, so
/// there is nothing to keep in sync by hand.
#[cfg(target_os = "windows")]
const BRIDGE_DIRECTORY: &str = r"QC Remote\integrations";
#[cfg(not(target_os = "windows"))]
const BRIDGE_DIRECTORY: &str = "QC Remote/integrations";
const BRIDGE_FILE_NAME: &str = "local-mcp-bridge.cjs";
fn claude_manifest() -> Result<Vec<u8>, String> {
    serde_json::to_vec_pretty(&json!({
        "manifest_version": "0.3",
        "name": CLAUDE_EXTENSION_NAME,
        "display_name": "QC Remote",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Control a Quad Cortex through the running QC Remote app.",
        "long_description": "A local-only bridge between Claude and QC Remote. QC Remote remains the sole owner of the Quad Cortex USB session and enforces its selected control access level. Claude manages tool approvals; choose Always allow after installation if you want uninterrupted control.",
        "author": { "name": CLAUDE_EXTENSION_AUTHOR },
        "server": {
            "type": "node",
            "entry_point": "server/index.cjs",
            "mcp_config": {
                "command": "node",
                // Names this route the way every managed route names itself, so
                // the per-client liveness map has a key to record it under -
                // through both channels, because a host that retains the
                // configured environment without passing it to the child it
                // spawns leaves the bridge heartbeating anonymously.
                "args": ["${__dirname}/server/index.cjs", CLIENT_ID_ARGUMENT, CLAUDE_CLIENT_ID],
                "env": { LOCAL_MCP_CLIENT_ENV: CLAUDE_CLIENT_ID }
            }
        },
        "tools_generated": true,
        "keywords": ["music", "guitar", "quad-cortex", "mcp", "local"],
        "compatibility": {
            "platforms": [if cfg!(target_os = "macos") { "darwin" } else if cfg!(target_os = "linux") { "linux" } else { "win32" }],
            "runtimes": { "node": ">=18.0.0" }
        }
    }))
    .map_err(|error| format!("Could not create the Claude extension manifest: {error}"))
}

/// Stamps the running app's tool contract digest into the bridge so an installed
/// client can warn when it is talking to a QC Remote built from a different one.
#[cfg(all(target_os = "macos", feature = "macos-app-store"))]
fn bridge_source() -> Result<Vec<u8>, String> {
    // Claude's Node runtime is not an App Group member. Delegate STDIO to our
    // signed native bridge instead of asking Node to read protected group state.
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the signed MCP bridge: {error}"))?;
    let executable = serde_json::to_string(&executable)
        .map_err(|error| format!("Could not encode the MCP bridge path: {error}"))?;
    Ok(format!(r#"// QC Remote {version}; tool contract {contract}
"use strict";
const {{ spawn }} = require("node:child_process");
const child = spawn({executable}, ["--local-mcp-bridge", ...process.argv.slice(2)], {{ stdio: "inherit", windowsHide: true }});
child.on("error", (error) => {{ console.error("QC Remote native bridge:", error.message); process.exitCode = 1; }});
child.on("exit", (code, signal) => {{ process.exitCode = code ?? (128 + (require("node:os").constants.signals[signal] ?? 0)); }});
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => child.kill(signal));
"#, version = env!("CARGO_PKG_VERSION"), contract = crate::local_access::tool_contract_sha256()).into_bytes())
}

#[cfg(not(all(target_os = "macos", feature = "macos-app-store")))]
fn bridge_source() -> Result<Vec<u8>, String> {
    let source = std::str::from_utf8(BRIDGE)
        .map_err(|error| format!("The local MCP bridge is not valid UTF-8: {error}"))?;
    Ok(source
        .replace(CONTRACT_SLOT, &crate::local_access::tool_contract_sha256())
        .replace(VERSION_SLOT, env!("CARGO_PKG_VERSION"))
        .into_bytes())
}

fn write_zip_file(
    archive: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    path: &str,
    contents: &[u8],
) -> Result<(), String> {
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    archive
        .start_file(path, options)
        .map_err(|error| format!("Could not create {path} in the client package: {error}"))?;
    archive
        .write_all(contents)
        .map_err(|error| format!("Could not write {path} in the client package: {error}"))
}

fn claude_desktop_extension() -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    write_zip_file(&mut archive, "manifest.json", &claude_manifest()?)?;
    write_zip_file(&mut archive, "server/index.cjs", &bridge_source()?)?;
    archive
        .finish()
        .map(|cursor| cursor.into_inner())
        .map_err(|error| format!("Could not finish the Claude extension: {error}"))
}

fn client_readme() -> String {
    #[cfg(target_os = "macos")]
    let (skill_codex, skill_antigravity, antigravity_config, installer, installed) = (
        "$HOME/.codex/skills",
        "$HOME/.gemini/config/skills",
        "$HOME/.gemini/config/mcp_config.json",
        "codex/install.sh",
        "~/Library/Application Support/QC Remote/integrations/local-mcp-bridge.cjs",
    );
    #[cfg(not(target_os = "macos"))]
    let (skill_codex, skill_antigravity, antigravity_config, installer, installed) = (
        "%USERPROFILE%\\.codex\\skills",
        "%USERPROFILE%\\.gemini\\config\\skills",
        "%USERPROFILE%\\.gemini\\config\\mcp_config.json",
        "codex\\install.ps1",
        "%LOCALAPPDATA%\\QC Remote\\integrations\\local-mcp-bridge.cjs",
    );
    format!(
        "# QC Remote local MCP clients\n\nVersion {}. Keep QC Remote open and connect the Quad Cortex before using these integrations. QC Remote remains the only USB owner.\n\n## Claude Desktop\n\nInstall the included `.mcpb` in Claude Desktop: Settings > Extensions > Advanced settings > Install Extension. The extension exposes the QC Device Knowledge skill as MCP resources.\n\n## ChatGPT Desktop and Codex\n\nOpen LOCAL in QC Remote and choose **Pair**. The app updates only its managed `qc-remote` MCP entry and installs the bundled skill under `{skill_codex}`. Existing skill contents are backed up before replacement. The managed route follows QC Remote upgrades automatically.\n\n## Google Antigravity\n\nOpen LOCAL in QC Remote and choose **Pair**. The app installs the bundled skill under `{skill_antigravity}` and updates its managed MCP entry. For manual setup, merge `antigravity/mcp_config.example.json` into `{antigravity_config}`. Before saving, replace the `command` value with the full path to the QC Remote executable inside your installed app bundle (for example, `/Applications/QC Remote.app/Contents/MacOS/QC Remote`).\n\n## Codex CLI manual fallback\n\nRun `{installer}`. It installs the bridge at `{installed}`, installs the bundled skill, and registers the bridge with Codex.\n\n## VS Code\n\nRun `{installer}` once to install the bridge at `{installed}`, then adapt `vscode/mcp.example.json`.\n",
        env!("CARGO_PKG_VERSION"),
        skill_codex = skill_codex, skill_antigravity = skill_antigravity,
        antigravity_config = antigravity_config, installer = installer, installed = installed
    )
}

fn codex_installer() -> String {
    #[cfg(target_os = "macos")]
    {
        return r##"#!/bin/sh
set -eu
command -v node >/dev/null 2>&1 || { echo "Node.js 18 or newer is required." >&2; exit 1; }
bridge_source="$(cd "$(dirname "$0")/../claude/server" && pwd)/index.cjs"
bridge_target="$HOME/Library/Application Support/QC Remote/integrations/local-mcp-bridge.cjs"
skill_source="$(cd "$(dirname "$0")/../skills/qc-device-knowledge" && pwd)"
skill_target="$HOME/.codex/skills/qc-device-knowledge"
mkdir -p "$(dirname "$bridge_target")"
cp "$bridge_source" "$bridge_target"
mkdir -p "$(dirname "$skill_target")"
if [ -e "$skill_target" ]; then mv "$skill_target" "$skill_target.backup.$(date -u +%Y%m%d%H%M%S)"; fi
cp -R "$skill_source" "$skill_target"
if ! command -v codex >/dev/null 2>&1; then
  echo "Bridge installed at '$bridge_target'. Install Codex CLI, then configure an STDIO MCP server with command node and that path." >&2
  exit 0
fi
configured_servers="$(codex mcp list --json)" || { echo "Codex could not read its existing MCP configuration." >&2; exit 1; }
entry_action="$(printf '%s' "$configured_servers" | node -e '
const fs = require("node:fs");
const servers = JSON.parse(fs.readFileSync(0, "utf8"));
if (!Array.isArray(servers)) throw new Error("Unexpected Codex MCP list format");
const entry = servers.find((server) => server && server.name === "qc-remote");
if (!entry) process.stdout.write("add");
else if (Array.isArray(entry.args) && entry.args.includes("--local-mcp-bridge")) process.stdout.write("managed-native");
else if ((Array.isArray(entry.args) && entry.args.some((arg) => String(arg).endsWith("local-mcp-bridge.cjs"))) || entry.env?.QC_REMOTE_LOCAL_MCP_CLIENT) process.stdout.write("replace");
else process.stdout.write("foreign");
')"
case "$entry_action" in
  managed-native)
    echo "Codex is already paired with QC Remote's native bridge, which updates with the app. That entry was left unchanged."
    exit 0
    ;;
  replace)
    codex mcp remove qc-remote || { echo "Codex could not replace the previous QC Remote MCP configuration." >&2; exit 1; }
    ;;
  foreign)
    echo "An MCP server named 'qc-remote' already exists and was left unchanged. Remove that entry yourself, then re-run this installer." >&2
    exit 1
    ;;
  add) ;;
  *) echo "Codex returned an unknown QC Remote MCP entry state." >&2; exit 1 ;;
esac
codex mcp add qc-remote -- node "$bridge_target" '{{clientArgument}}' '{{clientId}}'
echo "QC Remote MCP was added. Restart the client, then keep QC Remote open."
"##.replace("{{clientArgument}}", CLIENT_ID_ARGUMENT).replace("{{clientId}}", CODEX_CLIENT_ID);
    }
    #[cfg(not(target_os = "macos"))]
    {
        codex_installer_template()
            .replace("{{bridgeDirectory}}", BRIDGE_DIRECTORY)
            .replace("{{bridgeFileName}}", BRIDGE_FILE_NAME)
            .replace("{{clientArgument}}", CLIENT_ID_ARGUMENT)
            .replace("{{clientId}}", CODEX_CLIENT_ID)
    }
}

fn codex_installer_template() -> &'static str {
    r#"[CmdletBinding()]
param()
$ErrorActionPreference = "Stop"
$serverName = "qc-remote"
# --- qc-remote owned installer transactions ---
function Read-QcConfigBytes {
  param([string]$Path)
  $stream = $null
  try {
    try { $stream = [IO.File]::OpenRead($Path) }
    catch [IO.FileNotFoundException] { return $null }
    catch [IO.DirectoryNotFoundException] { return $null }
    if ($stream.Length -gt 8MB) { throw "Existing configuration exceeds the 8 MiB installer safety budget." }
    $memory = New-Object IO.MemoryStream
    try {
      $buffer = New-Object byte[] 16384
      while (($count = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
        if ($memory.Length + $count -gt 8MB) { throw "Existing configuration exceeds the 8 MiB installer safety budget." }
        $memory.Write($buffer, 0, $count)
      }
      return ,$memory.ToArray()
    } finally { $memory.Dispose() }
  } finally { if ($null -ne $stream) { $stream.Dispose() } }
}
function Test-QcBytesEqual {
  param($Left, $Right)
  if ($null -eq $Left -or $null -eq $Right) { return ($null -eq $Left -and $null -eq $Right) }
  return [Convert]::ToBase64String($Left).Equals([Convert]::ToBase64String($Right), [StringComparison]::Ordinal)
}
function Write-QcFileAtomically {
  param([string]$Path, [byte[]]$Bytes, [bool]$AllowReplace = $true, [scriptblock]$Verify = {
    param($HeldFile, $ExpectedSid)
    # Verify the ACTUAL already-held file object before credential bytes exist.
    # Never reopen the stage path to verify a potentially different object.
    $actual = if ('System.IO.FileSystemAclExtensions' -as [type]) { [IO.FileSystemAclExtensions]::GetAccessControl($HeldFile) } else { $HeldFile.GetAccessControl() }
    $rules = @($actual.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    return ($actual.GetOwner([Security.Principal.SecurityIdentifier]).Value -ceq $ExpectedSid.Value -and
      $actual.AreAccessRulesProtected -and $rules.Count -eq 1 -and
      $rules[0].IdentityReference.Value -ceq $ExpectedSid.Value -and -not $rules[0].IsInherited -and
      $rules[0].AccessControlType -eq [Security.AccessControl.AccessControlType]::Allow -and
      $rules[0].FileSystemRights -eq [Security.AccessControl.FileSystemRights]::FullControl -and
      $rules[0].InheritanceFlags -eq [Security.AccessControl.InheritanceFlags]::None -and
      $rules[0].PropagationFlags -eq [Security.AccessControl.PropagationFlags]::None)
  })
  $stage = Join-Path ([IO.Path]::GetDirectoryName($Path)) (".qc-install-" + [Guid]::NewGuid().ToString("N") + ".tmp")
  $file = $null
  $owned = $false; $failure = $null
  try {
    # Owner-private DACL is supplied to exclusive creation BEFORE any config
    # credential bytes exist. Never trust a custom CODEX_HOME's inheritance.
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = New-Object Security.AccessControl.FileSecurity
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid, [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.AccessControlType]::Allow)))
    if ('System.IO.FileSystemAclExtensions' -as [type]) {
      $file = [IO.FileSystemAclExtensions]::Create((New-Object IO.FileInfo($stage)), [IO.FileMode]::CreateNew, [Security.AccessControl.FileSystemRights]::FullControl, [IO.FileShare]::None, 4096, [IO.FileOptions]::None, $acl)
    } else {
      $file = New-Object IO.FileStream($stage, [IO.FileMode]::CreateNew, [Security.AccessControl.FileSystemRights]::FullControl, [IO.FileShare]::None, 4096, [IO.FileOptions]::None, $acl)
    }
    $owned = $true
    if (-not (& $Verify $file $sid)) { throw "The held stage is not owner-private; no credential bytes were written." }
    $file.Write($Bytes, 0, $Bytes.Length)
    $file.Flush($true)
    $file.Dispose(); $file = $null
    if ($AllowReplace -and [IO.File]::Exists($Path)) { [IO.File]::Replace($stage, $Path, $null) }
    else { [IO.File]::Move($stage, $Path) }
    $owned = $false
  } catch { $failure = $_; throw }
  finally {
    $cleanupFailure = $null
    try { if ($null -ne $file) { $file.Dispose() } }
    catch { $cleanupFailure = "Held file close failed: $_" }
    if ($owned) {
      try { [IO.File]::Delete($stage) }
      catch {
        $cleanupFailure = "$cleanupFailure Owned file stage cleanup failed: $_ Stage retained at '$stage'."
      }
    }
    if ($null -ne $cleanupFailure) { throw "$failure $cleanupFailure" }
  }
}
function Invoke-QcRemoteRouteChange {
  param([string]$ConfigPath, [scriptblock]$Add)
  $original = Read-QcConfigBytes $ConfigPath
  $recovery = $null
  if ($null -ne $original) {
    $recovery = Join-Path ([IO.Path]::GetDirectoryName($ConfigPath)) ("qc-remote-recovery-" + [Guid]::NewGuid().ToString("N") + ".toml")
    Write-QcFileAtomically $recovery $original $false
  }
  try {
    # Verified by the isolated real CLI fixture: mcp-add replaces its named
    # existing entry. Never remove the working route before a fallible add.
    & $Add
  } catch {
    $primary = $_
    # Codex does not share this installer's lease. A compare/read followed by
    # replace is NOT CAS against its ordinary concurrent atomic writes. Never
    # silently clobber them or claim an unproved automatic rollback.
    if ($null -ne $recovery) { throw "$primary Automatic rollback was not attempted. Original recovery document retained at '$recovery'." }
    throw "$primary No original configuration existed; inspect '$ConfigPath' for any partial CLI update."
  }
  if ($null -ne $recovery) { [IO.File]::Delete($recovery) }
}
function Get-QcSkillInventory {
  param([string]$Root)
  if ([IO.File]::GetAttributes($Root) -band [IO.FileAttributes]::ReparsePoint) { throw "Skill root is a reparse entry." }
  $rows = New-Object 'Collections.Generic.List[string]'
  $pending = New-Object 'Collections.Generic.Stack[object]'
  $pending.Push([pscustomobject]@{ Path = $Root; Depth = 0 })
  $entries = 0; $paths = 0L; $total = 0L
  $prefix = $Root.TrimEnd('\') + '\'
  while ($pending.Count -gt 0) {
    $directory = $pending.Pop()
    # Lazy enumeration: never materialize an entire oversized directory first.
    foreach ($path in [IO.Directory]::EnumerateFileSystemEntries($directory.Path)) {
      $entries++
      $relative = $path.Substring($prefix.Length)
      $paths += [Text.Encoding]::UTF8.GetByteCount($relative)
      if ($entries -gt 16384 -or $paths -gt 4MB) { throw "Skill inventory exceeds the installer safety budget." }
      $attributes = [IO.File]::GetAttributes($path)
      if ($attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Skill contains a reparse entry." }
      if ($attributes -band [IO.FileAttributes]::Directory) {
        if ($directory.Depth -ge 64) { throw "Skill directory depth exceeds the installer safety budget." }
        $rows.Add("D:$relative")
        $pending.Push([pscustomobject]@{ Path = $path; Depth = ($directory.Depth + 1) })
      } else {
        $stream = [IO.File]::OpenRead($path)
        $hash = $null
        try {
          $hash = [Security.Cryptography.SHA256]::Create()
          if ($stream.Length -gt 64MB) { throw "Skill file exceeds the installer safety budget." }
          $bytes = 0L; $buffer = New-Object byte[] 16384
          while (($count = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $bytes += $count; $total += $count
            if ($bytes -gt 64MB -or $total -gt 256MB) { throw "Skill bytes exceed the installer safety budget." }
            $null = $hash.TransformBlock($buffer, 0, $count, $buffer, 0)
          }
          $null = $hash.TransformFinalBlock((New-Object byte[] 0), 0, 0)
          $rows.Add("F:$relative`:" + ([BitConverter]::ToString($hash.Hash).Replace('-', '')))
        } finally { if ($null -ne $hash) { $hash.Dispose() }; $stream.Dispose() }
      }
    }
  }
  return @($rows | Sort-Object)
}
function Install-QcSkillTransaction {
  param([string]$Source, [string]$Target, [string]$BackupRoot, [scriptblock]$Copy, [scriptblock]$Move)
  $stage = Join-Path ([IO.Path]::GetDirectoryName($Target)) (".qc-skill-install-" + [Guid]::NewGuid().ToString("N"))
  $owned = $false; $backup = $null; $retired = $false; $failure = $null
  try {
    New-Item -ItemType Directory -Path $stage -ErrorAction Stop | Out-Null
    $owned = $true
    $before = @(Get-QcSkillInventory $Source)
    & $Copy $Source $stage
    $after = @(Get-QcSkillInventory $stage)
    if (($before -join "`n") -cne ($after -join "`n")) { throw "The staged skill does not match the complete source inventory." }
    if (Test-Path -LiteralPath $Target) {
      New-Item -ItemType Directory -Force -Path $BackupRoot | Out-Null
      $backup = Join-Path $BackupRoot ("qc-device-knowledge-" + [Guid]::NewGuid().ToString("N"))
      & $Move $Target $backup
      $retired = $true
    }
    try { & $Move $stage $Target; $owned = $false }
    catch {
      $primary = $_
      if ($retired) {
        try { & $Move $backup $Target; $retired = $false }
        catch { throw "$primary Skill rollback failed: $_ Original skill retained at '$backup'." }
      }
      throw $primary
    }
    if ($retired) { Write-Host "The previous device knowledge skill was backed up at '$backup'." }
  } catch { $failure = $_; throw }
  finally {
    if ($owned) {
      try { Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction Stop }
      catch {
        if ($null -ne $failure) { throw "$failure Owned skill stage cleanup failed: $_ Stage retained at '$stage'." }
        throw
      }
    }
  }
}
# --- end qc-remote owned installer transactions ---
$bridgeSource = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\claude\server\index.cjs")).Path
$skillSource = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\skills\qc-device-knowledge")).Path
$integrationDirectory = Join-Path $env:LOCALAPPDATA "{{bridgeDirectory}}"
$bridgeTarget = Join-Path $integrationDirectory "{{bridgeFileName}}"
$codexHome = if ([string]::IsNullOrWhiteSpace($env:CODEX_HOME)) { Join-Path $env:USERPROFILE ".codex" } else { $env:CODEX_HOME }
$skillRoot = Join-Path $codexHome "skills"
$skillTarget = Join-Path $skillRoot "qc-device-knowledge"
$skillBackupRoot = Join-Path $integrationDirectory "skill-backups\codex"
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw "Node.js 18 or newer is required." }
# Install the canonical bridge first so every instruction below names a file that exists.
New-Item -ItemType Directory -Force -Path $integrationDirectory | Out-Null
Write-QcFileAtomically $bridgeTarget ([IO.File]::ReadAllBytes($bridgeSource))
New-Item -ItemType Directory -Force -Path $skillRoot | Out-Null
Install-QcSkillTransaction $skillSource $skillTarget $skillBackupRoot {
  param($source, $stage)
  Get-ChildItem -LiteralPath $source -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse -Force -ErrorAction Stop
  }
} { param($source, $target) [IO.Directory]::Move($source, $target) }
if (-not (Get-Command codex -ErrorAction SilentlyContinue)) { throw "The Codex CLI is not on PATH. The bridge is installed at '$bridgeTarget'; add an STDIO MCP server manually with command 'node' and that path as its argument." }
$configuredServers = & codex mcp list --json | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Codex could not read the existing MCP configuration." }
# --- qc-remote entry classification ---
# An entry named qc-remote is the normal state after pairing inside the app, so
# refusing outright was wrong; so was the old remedy, which told the user to
# delete the managed native route and replaced it with this slower node one.
function Get-QcRemoteEntryAction {
  param($Entry, [string]$BridgeFileName)
  if ($null -eq $Entry) { return "add" }
  $entryArgs = @($Entry.args)
  # The in-app pairing launches QC Remote's own executable in bridge mode.
  if ($entryArgs -contains "--local-mcp-bridge") { return "managed-native" }
  # A previous run of this installer, which registers the JavaScript bridge.
  if ($entryArgs | Where-Object { "$_".EndsWith($BridgeFileName) }) { return "replace" }
  if ($Entry.env -and $Entry.env."QC_REMOTE_LOCAL_MCP_CLIENT") { return "replace" }
  return "foreign"
}
# --- end qc-remote entry classification ---
$existing = $configuredServers | Where-Object { $_.name -eq $serverName } | Select-Object -First 1
switch (Get-QcRemoteEntryAction -Entry $existing -BridgeFileName "{{bridgeFileName}}") {
  "managed-native" {
    Write-Host "Codex is already paired with QC Remote's native bridge, which needs no Node and updates itself with the app. That entry was left unchanged; the JavaScript bridge at '$bridgeTarget' is now up to date for VS Code and other manual clients."
    return
  }
  "replace" {
    # mcp-add updates this managed existing entry without a destructive remove.
  }
  "foreign" {
    throw "An MCP server named '$serverName' already exists, was not installed by QC Remote, and was left unchanged. The bridge at '$bridgeTarget' is now up to date; run 'codex mcp remove qc-remote' and re-run this installer to point Codex at it."
  }
}
$configPath = Join-Path $codexHome "config.toml"
Invoke-QcRemoteRouteChange $configPath {
  & codex mcp add $serverName -- node $bridgeTarget '{{clientArgument}}' '{{clientId}}'
  if ($LASTEXITCODE -ne 0) { throw "Codex could not add the QC Remote MCP configuration." }
}
Write-Host "QC Remote MCP was added. Restart the client, then keep QC Remote open."
"#
}

fn local_client_archive() -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let bridge = bridge_source()?;
    let claude_package = claude_desktop_extension()?;
    let claude_name = format!("QC-Remote-Claude-{}.mcpb", env!("CARGO_PKG_VERSION"));
    let readme = client_readme();
    let installer = codex_installer();
    let bridge_path = escaped_bridge_path();
    // The manual node routes name themselves too. Identity is not an in-app
    // detail: a hand-configured client that announces nothing can never be
    // reported as attached, which is indistinguishable from a broken setup.
    let codex_example = format!(
        "[mcp_servers.\"qc-remote\"]\ncommand = \"node\"\nargs = [\"{bridge_path}\", \"{ca}\", \"{ci}\"]\nstartup_timeout_sec = 15\ntool_timeout_sec = 300\n",
        ca = CLIENT_ID_ARGUMENT,
        ci = CODEX_CLIENT_ID
    );
    let vscode_example = format!(
        "{{\n  \"servers\": {{\n    \"qc-remote\": {{\n      \"type\": \"stdio\",\n      \"command\": \"node\",\n      \"args\": [\"{bridge_path}\", \"{ca}\", \"{ci}\"]\n    }}\n  }}\n}}\n",
        ca = CLIENT_ID_ARGUMENT,
        ci = OTHER_MCP_CLIENT_ID
    );
    // The client identity is part of the documented manual route, not an
    // in-app detail: `configured_antigravity_route` requires it before it will
    // call a route current, so an example without it produced a working server
    // that the app permanently labelled "different-configuration". It is
    // carried twice for the same reason the managed route carries it twice -
    // Antigravity does not pass `env` to a native STDIO child, so only
    // the argument reaches the heartbeat.
    #[cfg(target_os = "macos")]
    let antigravity_executable = "/path/to/QC Remote.app/Contents/MacOS/QC Remote";
    #[cfg(not(target_os = "macos"))]
    let antigravity_executable = r"C:\Users\<you>\AppData\Local\QC Remote\qc-remote.exe";
    let antigravity_example = format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                CLAUDE_EXTENSION_NAME: {
                    "command": antigravity_executable,
                    "args": ["--local-mcp-bridge", CLIENT_ID_ARGUMENT, ANTIGRAVITY_CLIENT_ID],
                    "env": { LOCAL_MCP_CLIENT_ENV: ANTIGRAVITY_CLIENT_ID }
                }
            }
        }))
        .map_err(|error| format!("Could not encode the Antigravity example: {error}"))?
    );
    let installer_path = if cfg!(target_os = "macos") {
        "codex/install.sh"
    } else {
        "codex/install.ps1"
    };
    let files: [(&str, &[u8]); 6] = [
        ("README.md", readme.as_bytes()),
        (installer_path, installer.as_bytes()),
        ("codex/mcp-config.example.toml", codex_example.as_bytes()),
        ("claude/server/index.cjs", bridge.as_slice()),
        (
            "antigravity/mcp_config.example.json",
            antigravity_example.as_bytes(),
        ),
        ("vscode/mcp.example.json", vscode_example.as_bytes()),
    ];
    for (path, contents) in files {
        write_zip_file(&mut archive, path, contents)?;
    }
    for file in DEVICE_KNOWLEDGE_SKILL_FILES {
        write_zip_file(
            &mut archive,
            &format!("skills/{DEVICE_KNOWLEDGE_SKILL_NAME}/{}", file.path),
            file.contents,
        )?;
    }
    write_zip_file(
        &mut archive,
        &format!("skills/{DEVICE_KNOWLEDGE_SKILL_NAME}/{DEVICE_KNOWLEDGE_SKILL_MARKER}"),
        &serde_json::to_vec_pretty(&bundled_device_knowledge_skill_marker())
            .map_err(|error| format!("Could not encode the bundled skill marker: {error}"))?,
    )?;
    write_zip_file(&mut archive, &claude_name, &claude_package)?;
    archive
        .finish()
        .map(|cursor| cursor.into_inner())
        .map_err(|error| format!("Could not finish the local client package: {error}"))
}

fn save_package(
    title: &str,
    extension: &str,
    file_name: String,
    contents: Vec<u8>,
) -> Result<Value, String> {
    let Some(mut path) = rfd::FileDialog::new()
        .set_title(title)
        .add_filter(title, &[extension])
        .set_file_name(&file_name)
        .save_file()
    else {
        return Ok(json!({ "cancelled": true }));
    };
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    write_package_bytes_with(&path, &contents, |file, bytes| file.write_all(bytes))
        .map_err(|error| format!("Could not save {}: {error}", display_path(&path)))?;
    Ok(json!({ "cancelled": false, "fileName": file_name }))
}

fn write_package_bytes_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    crate::workspace::write_document_with(path, |file| write(file, bytes))
}

fn display_path(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("the client package")
        .to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesktopInstallation {
    executable: Option<PathBuf>,
    version: Option<String>,
}

#[cfg(target_os = "windows")]
fn executable_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    #[repr(C)]
    struct VsFixedFileInfo {
        signature: u32,
        struct_version: u32,
        file_version_ms: u32,
        file_version_ls: u32,
        product_version_ms: u32,
        product_version_ls: u32,
        file_flags_mask: u32,
        file_flags: u32,
        file_os: u32,
        file_type: u32,
        file_subtype: u32,
        file_date_ms: u32,
        file_date_ls: u32,
    }

    #[link(name = "version")]
    extern "system" {
        fn GetFileVersionInfoSizeW(file_name: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(
            file_name: *const u16,
            handle: u32,
            length: u32,
            data: *mut c_void,
        ) -> i32;
        fn VerQueryValueW(
            block: *const c_void,
            sub_block: *const u16,
            buffer: *mut *mut c_void,
            length: *mut u32,
        ) -> i32;
    }

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut ignored = 0;
    // SAFETY: `wide` is NUL terminated and remains alive for both Win32 calls.
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut ignored) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0_u8; size as usize];
    // SAFETY: `data` is allocated to the exact size requested by Windows.
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast::<c_void>()) }
        == 0
    {
        return None;
    }
    let root = ['\\' as u16, 0];
    let mut fixed = std::ptr::null_mut::<c_void>();
    let mut fixed_size = 0;
    // SAFETY: the version block is owned by `data`; Windows returns a pointer
    // inside it that is valid until `data` is dropped below.
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast::<c_void>(),
            root.as_ptr(),
            &mut fixed,
            &mut fixed_size,
        )
    } == 0
        || fixed.is_null()
        || fixed_size < std::mem::size_of::<VsFixedFileInfo>() as u32
    {
        return None;
    }
    // SAFETY: the size check above guarantees a complete VS_FIXEDFILEINFO.
    let fixed = unsafe { &*fixed.cast::<VsFixedFileInfo>() };
    (fixed.signature == 0xFEEF04BD).then(|| {
        format!(
            "{}.{}.{}.{}",
            fixed.file_version_ms >> 16,
            fixed.file_version_ms & 0xffff,
            fixed.file_version_ls >> 16,
            fixed.file_version_ls & 0xffff
        )
    })
}

#[cfg(not(target_os = "windows"))]
fn executable_version(_path: &Path) -> Option<String> {
    None
}

fn version_from_executable_path(path: &Path) -> Option<String> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("app-"))
        .filter(|version| version_numbers(version).is_some())
        .map(str::to_owned)
        .or_else(|| executable_version(path))
}

#[cfg(any(target_os = "windows", test))]
fn package_version(package_name: &str, package_prefixes: &[&str]) -> Option<String> {
    let matched_prefix = package_prefixes.iter().find(|candidate| {
        package_name
            .to_ascii_lowercase()
            .starts_with(&candidate.to_ascii_lowercase())
    })?;
    package_name
        .get(matched_prefix.len()..)?
        .split('_')
        .next()
        .filter(|candidate| version_numbers(candidate).is_some())
        .map(str::to_owned)
}

#[cfg(target_os = "windows")]
fn registry_package_installations(
    package_prefixes: &[&str],
    executable_candidates: &[&str],
) -> Vec<DesktopInstallation> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const PACKAGES_KEY: &str = r"HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
    let mut installations = Vec::new();
    for prefix in package_prefixes {
        let Ok(output) = run_with_timeout(
            Command::new(crate::system_tools::reg_exe())
                .args([
                    "query",
                    PACKAGES_KEY,
                    "/f",
                    prefix.trim_end_matches('_'),
                    "/k",
                ])
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW),
            EXTERNAL_TOOL_TIMEOUT,
        ) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let key = line.trim();
            if !key.starts_with("HKEY_") {
                continue;
            }
            let Some(package_name) = key.rsplit('\\').next() else {
                continue;
            };
            let Some(version) = package_version(package_name, package_prefixes) else {
                continue;
            };
            let root = run_with_timeout(
                Command::new(crate::system_tools::reg_exe())
                    .args(["query", key, "/v", "PackageRootFolder"])
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW),
                EXTERNAL_TOOL_TIMEOUT,
            )
            .ok()
            .filter(|result| result.status.success())
            .and_then(|result| {
                String::from_utf8_lossy(&result.stdout)
                    .lines()
                    .find_map(|line| {
                        line.split_once("REG_SZ")
                            .map(|(_, value)| PathBuf::from(value.trim()))
                    })
            });
            let executable = root.as_deref().and_then(|root| {
                executable_candidates
                    .iter()
                    .map(|candidate| root.join(candidate))
                    .find(|candidate| candidate.is_file())
            });
            installations.push(DesktopInstallation {
                executable,
                version: Some(version),
            });
        }
    }
    installations.sort_by(|left, right| match (&left.version, &right.version) {
        (Some(left), Some(right)) => {
            compare_versions(left, right).unwrap_or_else(|| left.cmp(right))
        }
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    });
    installations.dedup();
    installations
}

#[cfg(not(target_os = "windows"))]
fn registry_package_installations(
    _package_prefixes: &[&str],
    _executable_candidates: &[&str],
) -> Vec<DesktopInstallation> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn registered_app_path(executable_name: &str) -> Option<PathBuf> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    [
        format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths\{executable_name}"),
        format!(r"HKLM\Software\Microsoft\Windows\CurrentVersion\App Paths\{executable_name}"),
        format!(r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\{executable_name}"),
    ]
    .into_iter()
    .find_map(|key| {
        let output = run_with_timeout(Command::new(crate::system_tools::reg_exe())
            .args(["query", &key, "/ve"])
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            , EXTERNAL_TOOL_TIMEOUT)
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| {
                line.split_once("REG_SZ")
                    .map(|(_, value)| PathBuf::from(value.trim().trim_matches('"')))
            })
            .filter(|path| path.is_file())
    })
}

#[cfg(target_os = "windows")]
fn where_command(executable_name: &str) -> Vec<PathBuf> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // where.exe searches its current directory before PATH. Run it from
    // System32 rather than from wherever this app was started, and drop
    // anything it finds there: only an installed program counts.
    let searched_first = crate::system_tools::system32_directory();
    let Ok(output) = run_with_timeout(
        Command::new(crate::system_tools::where_exe())
            .arg(executable_name)
            .current_dir(&searched_first)
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_TIMEOUT,
    ) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    crate::system_tools::installed_where_results(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from),
        &searched_first,
    )
    .into_iter()
    .filter(|path| path.is_file())
    .collect()
}

#[cfg(not(target_os = "windows"))]
fn where_command(executable_name: &str) -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let Some(path) = std::env::var_os("PATH") else {
            return Vec::new();
        };
        where_command_in(executable_name, &path)
    }
    #[cfg(target_os = "macos")]
    {
        let mut directories = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(home) = user_home_directory() {
            directories.extend([
                home.join(".local").join("bin"),
                home.join(".npm-global").join("bin"),
                home.join("bin"),
            ]);
        }
        directories.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]);
        let mut unique = Vec::new();
        for directory in directories {
            if directory.is_absolute() && !unique.contains(&directory) {
                unique.push(directory);
            }
        }
        let Ok(search_path) = std::env::join_paths(unique) else {
            return Vec::new();
        };
        where_command_in(executable_name, &search_path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = executable_name;
        Vec::new()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn where_command_in(executable_name: &str, search_path: &std::ffi::OsStr) -> Vec<PathBuf> {
    std::env::split_paths(search_path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(executable_name))
        .filter(|candidate| {
            fs::metadata(candidate)
                .is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
        })
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn registered_app_path(_executable_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "windows")]
fn chatgpt_process_start_times() -> Vec<u128> {
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const INVALID_HANDLE_VALUE: isize = -1;
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessEntry32W {
        dwSize: u32,
        cntUsage: u32,
        th32ProcessID: u32,
        th32DefaultHeapID: usize,
        th32ModuleID: u32,
        cntThreads: u32,
        th32ParentProcessID: u32,
        pcPriClassBase: i32,
        dwFlags: u32,
        szExeFile: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    // SAFETY: the returned snapshot handle is checked before use and closed.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut entry: ProcessEntry32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<ProcessEntry32W>() as u32;
    let mut starts = Vec::new();
    // SAFETY: `entry` has the documented size and remains valid during iteration.
    let mut present = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while present {
        let name_end = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..name_end]);
        if name.eq_ignore_ascii_case("ChatGPT.exe") || name.eq_ignore_ascii_case("Codex.exe") {
            // SAFETY: the process handle is checked and closed after querying.
            let process =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID) };
            if process != 0 {
                let mut creation = FileTime { low: 0, high: 0 };
                let mut exit = FileTime { low: 0, high: 0 };
                let mut kernel = FileTime { low: 0, high: 0 };
                let mut user = FileTime { low: 0, high: 0 };
                // SAFETY: all FILETIME pointers refer to initialized writable values.
                if unsafe {
                    GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user)
                } != 0
                {
                    let windows_ticks = ((creation.high as u64) << 32) | creation.low as u64;
                    if let Some(unix_ticks) = windows_ticks.checked_sub(WINDOWS_TO_UNIX_EPOCH_100NS)
                    {
                        starts.push((unix_ticks / 10_000) as u128);
                    }
                }
                // SAFETY: `process` is a valid handle returned by OpenProcess.
                unsafe { CloseHandle(process) };
            }
        }
        // SAFETY: the snapshot and entry remain valid until CloseHandle below.
        present = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: `snapshot` is a valid handle returned above.
    unsafe { CloseHandle(snapshot) };
    starts
}

#[cfg(not(target_os = "windows"))]
fn chatgpt_process_start_times() -> Vec<u128> {
    Vec::new()
}

/// How to recognise one host, as data.
///
/// Every provider used to carry its own search, and that is the per-provider
/// fork of shared logic this repository has already been bitten by: see
/// [`crate::local_access::antigravity_bridge_connected`], whose second
/// liveness atomic let one host miss a correction made to the other five. It
/// is also why LM Studio went unrecognised after its rename to Bionic - the
/// new name would have had to be taught to a search nothing else used, and a
/// mechanism its search lacked was a mechanism all six lacked. Adding either
/// here reaches every host at once.
struct DesktopAppSearch {
    /// Bare executable file names. The `App Paths` registration, `where.exe`,
    /// a Store execution alias and an `Uninstall` entry's `InstallLocation`
    /// are each keyed on or joined with one of these.
    ///
    /// Only names that launch the application itself belong here. A companion
    /// CLI on `PATH` - LM Studio ships `lms.exe` - would otherwise be resolved
    /// as the app and then started in its place.
    executable_names: &'static [&'static str],
    /// Paths relative to each root in [`installation_roots`], in preference
    /// order: the first candidate that names a file wins, across every root,
    /// so a provider states its preference here rather than depending on the
    /// order of the roots.
    relative_candidates: &'static [&'static str],
    /// The `DisplayName` values this application may register in any of the
    /// three `Uninstall` hives, compared case-insensitively and in full.
    display_names: &'static [&'static str],
    /// The application's own record of where it installed itself.
    self_records: &'static [SelfInstallRecord],
    /// MSIX package family name prefixes, as the AppModel repository spells
    /// them, with the executable candidates to try inside a package root.
    /// Empty for an application that ships no Store package: the name-free
    /// Store mechanisms - the execution alias, `App Paths`, `PATH` - still
    /// apply, and a family name is not worth guessing.
    package_prefixes: &'static [&'static str],
    package_executables: &'static [&'static str],
}

/// An application's own note of where it put itself.
///
/// LM Studio writes one, and it is the only mechanism that survived its
/// rename: the binary, the directory and the uninstaller all became `Bionic`,
/// no `Uninstall` key, no MSIX package and no `App Paths` entry was ever
/// written, and this file still names the executable exactly. It is
/// authoritative, it survives the next rename too, and it is one file read.
struct SelfInstallRecord {
    /// Path to the record, relative to `%USERPROFILE%`.
    relative_path: &'static str,
    /// The top-level member holding the executable path.
    member: &'static str,
}

/// Every directory a Windows installer may put an application under, per-user
/// and per-machine, in the order they are searched.
///
/// `%LOCALAPPDATA%\Programs` is where a per-user Electron or Squirrel
/// installer lands by default - Claude, ChatGPT, Antigravity, Bionic and Jan
/// all install this way. `%ProgramW6432%` is the 64-bit program
/// directory seen from either architecture; inside a 64-bit process it is the
/// same string as `%ProgramFiles%`, which is why the list is de-duplicated
/// rather than assumed distinct.
fn installation_roots_from(
    local_app_data: Option<&Path>,
    program_w6432: Option<&Path>,
    program_files: Option<&Path>,
    program_files_x86: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(local) = local_app_data {
        roots.push(local.join("Programs"));
        roots.push(local.to_path_buf());
    }
    for root in [program_w6432, program_files, program_files_x86]
        .into_iter()
        .flatten()
    {
        roots.push(root.to_path_buf());
    }
    let mut unique: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

fn installation_roots() -> Vec<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_w6432 = std::env::var_os("ProgramW6432").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    installation_roots_from(
        local_app_data.as_deref(),
        program_w6432.as_deref(),
        program_files.as_deref(),
        program_files_x86.as_deref(),
    )
}

/// Squirrel keeps each release in its own `app-<version>` directory beside the
/// stub launcher it also writes, and a fresh or interrupted update can leave
/// only the versioned directory. The newest of them is the installed one.
///
/// Claude and Antigravity each had their own copy of this search. It is a
/// property of the packager, not of either application, so it is derived from
/// the candidate itself: an application not packaged that way simply has no
/// `app-*` sibling to find.
fn newest_versioned_sibling(candidate: &Path) -> Option<PathBuf> {
    let parent = candidate.parent()?;
    let file_name = candidate.file_name()?.to_owned();
    let mut versioned = fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let version = name.to_str()?.strip_prefix("app-")?.to_owned();
            let executable = entry.path().join(&file_name);
            executable.is_file().then_some((version, executable))
        })
        .collect::<Vec<_>>();
    versioned.sort_by(|(left, _), (right, _)| {
        compare_versions(left, right)
            .unwrap_or_else(|| left.cmp(right))
            .reverse()
    });
    versioned
        .into_iter()
        .next()
        .map(|(_, executable)| executable)
}

/// The first path this search names that is a file on disk.
fn desktop_app_paths_in(roots: &[PathBuf], search: &DesktopAppSearch) -> Option<PathBuf> {
    search.relative_candidates.iter().find_map(|relative| {
        roots.iter().find_map(|root| {
            let candidate = root.join(relative);
            if candidate.is_file() {
                return Some(candidate);
            }
            newest_versioned_sibling(&candidate)
        })
    })
}

/// A Microsoft Store application's execution alias.
///
/// A Store package's real files live under `%ProgramFiles%\WindowsApps\<full
/// package name>`, whose ACL denies the signed-in user, so walking it is not a
/// mechanism to depend on. The alias is the part of a Store install an
/// ordinary process can point at and start.
///
/// The alias is a zero-length `AppExecLink` reparse point: `Path::is_file`
/// reports `true`, `metadata()` succeeds and reports length `0`, and
/// `symlink_metadata()` reports `is_symlink() == false`. So the
/// existence probe every mechanism here shares must stay a plain file test:
/// growing it a "and is not empty" clause would silently reject every Store
/// app, and a symlink-following special case would be dead code.
fn store_alias_path(local_app_data: &Path, executable_names: &[&str]) -> Option<PathBuf> {
    let aliases = local_app_data.join("Microsoft").join("WindowsApps");
    executable_names
        .iter()
        .map(|name| aliases.join(name))
        .find(|path| path.is_file())
}

/// The executable an application's own install record names.
///
/// Untrusted input, so every condition is checked rather than assumed: the
/// document parses, the member is present and is a string, the path is
/// absolute - a relative one would resolve against this process's working
/// directory, which is not the application's - and it names a file that is
/// there now. A record that fails any of them yields nothing and the caller
/// carries on down the chain.
fn self_recorded_executable(contents: &str, member: &str) -> Option<PathBuf> {
    let recorded = serde_json::from_str::<Value>(contents).ok()?;
    let path = PathBuf::from(recorded.get(member)?.as_str()?);
    (path.is_absolute() && path.is_file()).then_some(path)
}

fn self_recorded_path(user_profile: &Path, records: &[SelfInstallRecord]) -> Option<PathBuf> {
    records.iter().find_map(|record| {
        let contents = read_client_document_text(user_profile.join(record.relative_path)).ok()?;
        self_recorded_executable(&contents, record.member)
    })
}

/// A path as an `Uninstall` entry stores it.
///
/// Two wrappers, both present in Jan's own registry entries. Jan registers
/// `InstallLocation` as `"C:\Users\...\Programs\Jan"` and `DisplayIcon` as
/// `"C:\Users\...\Programs\Jan\jan.exe"`, quotation marks included in the
/// value; and `DisplayIcon` conventionally carries the index of the icon
/// inside the file as a `,0` suffix. A resolver that takes either value
/// literally looks for a directory whose name begins with a quotation mark.
///
/// Only a trailing all-digit field is an index, so a directory named
/// `Foo, Inc` survives, and the index is stripped on both sides of unquoting
/// because installers write it inside the quotes as well as outside them.
fn unwrap_registry_path(value: &str) -> Option<String> {
    fn strip_icon_index(value: &str) -> &str {
        let Some((head, index)) = value.rsplit_once(',') else {
            return value;
        };
        let index = index.trim();
        if head.is_empty() || index.is_empty() || !index.chars().all(|digit| digit.is_ascii_digit())
        {
            return value;
        }
        head.trim_end()
    }
    let outer = strip_icon_index(value.trim());
    let unquoted = outer
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(outer);
    let unwrapped = strip_icon_index(unquoted.trim()).trim();
    (!unwrapped.is_empty()).then(|| unwrapped.to_owned())
}

/// One `Uninstall` entry, in the values that say where the application is and
/// what it calls its own version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct UninstallEntry {
    display_version: Option<String>,
    install_location: Option<String>,
    display_icon: Option<String>,
}

/// Where a directory holding this application would keep its executable.
///
/// A relative directory is dropped rather than searched: it would resolve
/// against this process's working directory, so a `DisplayIcon` of `app.exe`
/// must never be able to match an unrelated `app.exe` next to QC Remote.
fn push_executable_candidates(
    candidates: &mut Vec<PathBuf>,
    directory: &Path,
    executable_names: &[&str],
) {
    if !directory.is_absolute() {
        return;
    }
    for name in executable_names {
        candidates.push(directory.join(name));
        // The subdirectory an Electron packager puts the binary in.
        candidates.push(directory.join("app").join(name));
    }
}

/// The executable an `Uninstall` entry points at, or nothing when its paths
/// are gone.
///
/// `DisplayIcon` is tried first, because an installer that writes it usually
/// writes the executable itself; when it names a `.ico` instead, the directory
/// it sits in is still where the application lives. `InstallLocation` follows.
///
/// Returning `None` for an entry whose paths no longer exist is the point. An
/// uninstaller that left its key behind must not be reported as an
/// installation, because everything downstream - the pair button, the launch
/// button, the minimum-version gate - then acts on a path that is not there.
fn uninstall_entry_executable(
    entry: &UninstallEntry,
    executable_names: &[&str],
) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(icon) = entry
        .display_icon
        .as_deref()
        .and_then(unwrap_registry_path)
        .map(PathBuf::from)
    {
        if icon.is_absolute()
            && icon
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            candidates.push(icon.clone());
        }
        if let Some(directory) = icon.parent() {
            push_executable_candidates(&mut candidates, directory, executable_names);
        }
    }
    if let Some(location) = entry
        .install_location
        .as_deref()
        .and_then(unwrap_registry_path)
    {
        push_executable_candidates(&mut candidates, Path::new(&location), executable_names);
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// One `<name>    REG_SZ    <data>` line, when it carries the named value.
///
/// The type token is what separates the name from the data, rather than the
/// run of spaces `reg.exe` pads with, because a `DisplayName` may itself
/// contain one. Requiring the token also keeps `DisplayName` from matching
/// `DisplayNameLocalized`.
#[cfg(any(target_os = "windows", test))]
fn registry_value(line: &str, name: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(name)?.trim_start();
    let (kind, data) = rest.split_once(char::is_whitespace)?;
    kind.starts_with("REG_").then(|| data.trim().to_owned())
}

/// The `DisplayName` of every entry in a `reg.exe query <hive> /s /v
/// DisplayName` dump, paired with the key that carries it.
#[cfg(any(target_os = "windows", test))]
fn uninstall_display_names(output: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut key: Option<&str> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("HKEY_") {
            key = Some(trimmed);
            continue;
        }
        if let (Some(current), Some(display_name)) = (key, registry_value(trimmed, "DisplayName")) {
            entries.push((current.to_owned(), display_name));
        }
    }
    entries
}

/// One entry's location values, out of a `reg.exe query <key>` dump.
#[cfg(any(target_os = "windows", test))]
fn uninstall_entry_from_key_dump(output: &str) -> UninstallEntry {
    let mut entry = UninstallEntry::default();
    for line in output.lines() {
        if let Some(value) = registry_value(line, "DisplayVersion") {
            entry.display_version = Some(value);
        } else if let Some(value) = registry_value(line, "InstallLocation") {
            entry.install_location = Some(value);
        } else if let Some(value) = registry_value(line, "DisplayIcon") {
            entry.display_icon = Some(value);
        }
    }
    entry
}

/// The three `Uninstall` hives: per-user, per-machine, and the 32-bit view of
/// per-machine that a WOW64 installer writes to. `HKCU` is read first because
/// it is where a per-user install lands and because it is the smallest.
#[cfg(target_os = "windows")]
const UNINSTALL_KEYS: &[&str] = &[
    r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
];

/// Every registered `DisplayName`, and when the sweep that read them ran.
struct UninstallIndex {
    entries: Vec<(String, String)>,
    read_at: Instant,
}

/// How long the two mechanisms that shell out may reuse their last answer.
///
/// One panel refresh asks every provider for its status at once, and a
/// provider found only by a registry sweep - or not found at all - reaches
/// them every time. The window is what keeps that burst to one sweep: short
/// enough that somebody who installs an application and looks again sees it,
/// long enough that the concurrent reads share a single answer. The cheap
/// mechanisms are never cached, so a host installed where its installer puts
/// it is always reported live.
const INSTALLATION_LOOKUP_WINDOW: Duration = Duration::from_secs(5);

fn uninstall_index() -> Vec<(String, String)> {
    static INDEX: OnceLock<Mutex<Option<UninstallIndex>>> = OnceLock::new();
    let Ok(held) = INDEX.get_or_init(|| Mutex::new(None)).lock() else {
        return read_uninstall_index();
    };
    if let Some(index) = held
        .as_ref()
        .filter(|index| index.read_at.elapsed() < INSTALLATION_LOOKUP_WINDOW)
    {
        return index.entries.clone();
    }
    // Never hold this shared cache mutex across registry/OS work. Concurrent
    // cold lookups may duplicate a bounded sweep, but cannot queue every
    // provider behind another actor's stalled synchronous I/O.
    drop(held);
    let entries = read_uninstall_index();
    if let Ok(mut held) = INDEX.get_or_init(|| Mutex::new(None)).lock() {
        *held = Some(UninstallIndex {
            entries: entries.clone(),
            read_at: Instant::now(),
        });
    }
    entries
}

#[cfg(target_os = "windows")]
fn read_uninstall_index() -> Vec<(String, String)> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut entries = Vec::new();
    for key in UNINSTALL_KEYS {
        // Asking for the one value is what makes this affordable: `/s` over
        // the per-machine hive costs 2.75 s and prints 206 kB; `/s /v
        // DisplayName` costs 0.22 s and prints 27 kB.
        let Ok(output) = run_with_timeout(
            Command::new(crate::system_tools::reg_exe())
                .args(["query", key, "/s", "/v", "DisplayName"])
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW),
            EXTERNAL_TOOL_TIMEOUT,
        ) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        entries.extend(uninstall_display_names(&String::from_utf8_lossy(
            &output.stdout,
        )));
    }
    entries
}

#[cfg(not(target_os = "windows"))]
fn read_uninstall_index() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(target_os = "windows")]
fn read_uninstall_entry(key: &str) -> UninstallEntry {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    run_with_timeout(
        Command::new(crate::system_tools::reg_exe())
            .args(["query", key])
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_TIMEOUT,
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| uninstall_entry_from_key_dump(&String::from_utf8_lossy(&output.stdout)))
    .unwrap_or_default()
}

#[cfg(not(target_os = "windows"))]
fn read_uninstall_entry(_key: &str) -> UninstallEntry {
    UninstallEntry::default()
}

/// The application as the `Uninstall` registry describes it.
///
/// This is the most general mechanism there is. MSI, Inno Setup, NSIS,
/// Squirrel and WinGet all register here, per-user under `HKCU` and
/// per-machine under `HKLM` and its 32-bit view, and it is how Jan is found by
/// this app. An application that installs itself somewhere nobody guessed,
/// or into a custom MSI target directory, is still found - its entry says
/// where it went.
fn uninstall_registry_installation(search: &DesktopAppSearch) -> Option<DesktopInstallation> {
    if search.display_names.is_empty() {
        return None;
    }
    uninstall_index().iter().find_map(|(key, display_name)| {
        if !search
            .display_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(display_name))
        {
            return None;
        }
        let entry = read_uninstall_entry(key);
        let executable = uninstall_entry_executable(&entry, search.executable_names)?;
        // The entry's own `DisplayVersion` is a fallback for a binary with no
        // version resource, not an invention: it is the number the installer
        // registered. A value that is not a version is dropped rather than
        // displayed.
        let version = version_from_executable_path(&executable).or_else(|| {
            entry
                .display_version
                .clone()
                .filter(|value| version_numbers(value).is_some())
        });
        Some(DesktopInstallation {
            executable: Some(executable),
            version,
        })
    })
}

/// The executable inside an MSIX package's install location, as
/// `Get-AppxPackage` reports it.
#[cfg(target_os = "windows")]
fn msix_executable_from_install_location(
    value: &[u8],
    executable_candidates: &[&str],
) -> Option<PathBuf> {
    let install_location = String::from_utf8_lossy(value).trim().to_owned();
    if install_location.is_empty() {
        return None;
    }
    let install_location = PathBuf::from(install_location);
    executable_candidates
        .iter()
        .map(|relative| install_location.join(relative))
        .find(|path| path.is_file())
}

/// The newest registered MSIX package's executable, if this account can read
/// it at all.
#[cfg(target_os = "windows")]
fn registered_msix_path(search: &DesktopAppSearch) -> Option<PathBuf> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if search.package_prefixes.is_empty() {
        return None;
    }
    let lookups = search
        .package_prefixes
        .iter()
        .map(|prefix| {
            format!(
                "Get-AppxPackage -Name '{}' -ErrorAction SilentlyContinue",
                prefix.trim_end_matches('_')
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let script = format!(
        "@({lookups}) | Sort-Object Version -Descending | Select-Object -First 1 -ExpandProperty InstallLocation"
    );
    let output = run_with_timeout(
        Command::new(crate::system_tools::powershell_exe())
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_TIMEOUT,
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    msix_executable_from_install_location(&output.stdout, search.package_executables)
}

#[cfg(not(target_os = "windows"))]
fn registered_msix_path(_search: &DesktopAppSearch) -> Option<PathBuf> {
    None
}

/// Which mechanism answered.
///
/// Reported because the order is a property worth asserting and is otherwise
/// invisible: two mechanisms can both hold a valid answer at the same time -
/// Bionic has a default path *and* a self-record, both naming a real file -
/// and nothing in the returned path says which one was consulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallationMechanism {
    DefaultPath,
    SelfRecord,
    StoreAlias,
    AppPaths,
    SearchPath,
    UninstallRegistry,
    PackageRegistration,
}

/// One resolution path, shared by every provider.
///
/// Ordered cheapest and most certain first, so a host installed where its
/// installer usually puts it costs a handful of `stat` calls and never reaches
/// a registry sweep or a PowerShell start:
///
/// 1. [`InstallationMechanism::DefaultPath`] - the paths a default installer
///    uses, under every per-user and per-machine root, including the newest
///    Squirrel `app-<version>` directory beside each;
/// 2. [`InstallationMechanism::SelfRecord`] - the application's own record of
///    where it installed itself;
/// 3. [`InstallationMechanism::StoreAlias`] - a Microsoft Store execution
///    alias;
/// 4. [`InstallationMechanism::AppPaths`] - the `App Paths` registration;
/// 5. [`InstallationMechanism::SearchPath`] - `PATH`;
/// 6. [`InstallationMechanism::UninstallRegistry`] - all three hives;
/// 7. [`InstallationMechanism::PackageRegistration`] - the MSIX registration,
///    which is the one mechanism that can report an installation without a
///    path. See the comment on that step.
///
/// Steps 1 to 6 each gate on `is_file`, so a mechanism that yields a path that
/// is not there does not answer: the chain carries on rather than reporting an
/// installation nothing can point at.
///
/// The roots, the profile and the local application data are arguments rather
/// than environment reads so that the order can be exercised against a
/// fixture instead of against whatever this machine happens to have installed.
fn resolve_desktop_app_in(
    roots: &[PathBuf],
    local_app_data: Option<&Path>,
    user_profile: Option<&Path>,
    search: &DesktopAppSearch,
) -> Option<(InstallationMechanism, DesktopInstallation)> {
    let found = desktop_app_paths_in(roots, search)
        .map(|path| (InstallationMechanism::DefaultPath, path))
        .or_else(|| {
            user_profile
                .and_then(|profile| self_recorded_path(profile, search.self_records))
                .map(|path| (InstallationMechanism::SelfRecord, path))
        })
        .or_else(|| {
            local_app_data
                .and_then(|local| store_alias_path(local, search.executable_names))
                .map(|path| (InstallationMechanism::StoreAlias, path))
        })
        .or_else(|| {
            search
                .executable_names
                .iter()
                .copied()
                .find_map(registered_app_path)
                .map(|path| (InstallationMechanism::AppPaths, path))
        })
        .or_else(|| {
            search
                .executable_names
                .iter()
                .copied()
                .flat_map(where_command)
                .next()
                .map(|path| (InstallationMechanism::SearchPath, path))
        });
    if let Some((mechanism, executable)) = found {
        return Some((
            mechanism,
            DesktopInstallation {
                version: version_from_executable_path(&executable),
                executable: Some(executable),
            },
        ));
    }
    if let Some(installation) = uninstall_registry_installation(search) {
        return Some((InstallationMechanism::UninstallRegistry, installation));
    }
    package_registration_installation(search)
        .map(|installation| (InstallationMechanism::PackageRegistration, installation))
}

/// The application as the MSIX package registration describes it.
///
/// This is the one mechanism that can report an installation without a path. A
/// Store package's files sit under `%ProgramFiles%\WindowsApps\<full package
/// name>`, whose ACL denies the signed-in user on most packages, so the
/// executable is frequently not pointable-at even though the package is
/// certainly installed - the registration is the evidence. `executable` then
/// stays `None` rather than naming a file that cannot be opened, and nothing
/// is invented: the version is the one in the package family name, and when
/// that does not parse, `version` stays `None` and the status document simply
/// carries no `appVersion`.
///
/// The version comes from the family name even when a readable executable was
/// found, because a packaged Electron application's executable carries its
/// framework's version resource, not its own - the FileVersion on disk names
/// the bundled Chromium build, not the application's release.
fn package_registration_installation(search: &DesktopAppSearch) -> Option<DesktopInstallation> {
    if search.package_prefixes.is_empty() {
        return None;
    }
    if let Some(cached) = cached_package_registration(search.package_prefixes) {
        return cached;
    }
    let registered =
        registry_package_installations(search.package_prefixes, search.package_executables).pop();
    let installation = match registered {
        // The repository named a readable executable, so there is nothing left
        // to ask: a path and the package's own version.
        Some(installation) if installation.executable.is_some() => Some(installation),
        // The package is registered and its root did not yield a file. Ask the
        // package manager for the install location before giving up on a path,
        // and keep the declared version either way.
        Some(installation) => Some(DesktopInstallation {
            executable: registered_msix_path(search),
            version: installation.version,
        }),
        None => registered_msix_path(search).map(|executable| DesktopInstallation {
            version: version_from_executable_path(&executable),
            executable: Some(executable),
        }),
    };
    remember_package_registration(search.package_prefixes, installation.clone());
    installation
}

/// One package family's last answer. `installation` is itself an `Option`
/// because "not installed" is worth remembering for the same window as
/// "installed": it is the answer that costs the most to reach.
struct PackageRegistrationMemo {
    installation: Option<DesktopInstallation>,
    read_at: Instant,
}

/// The last answer for each package family.
///
/// This step costs 1.4 s for ChatGPT - the only provider here that is *only*
/// findable through it - so without a window a
/// panel refresh spends that on every poll. The lock is held for the map reads
/// and writes alone and never across the `reg.exe` or PowerShell work, so one
/// slow family cannot hold up another.
fn package_registration_cache() -> &'static Mutex<BTreeMap<String, PackageRegistrationMemo>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, PackageRegistrationMemo>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// `Some(answer)` when the window has not elapsed, `None` when this family has
/// to be looked up again.
fn cached_package_registration(package_prefixes: &[&str]) -> Option<Option<DesktopInstallation>> {
    let key = package_prefixes.join(" ");
    let cache = package_registration_cache().lock().ok()?;
    cache
        .get(&key)
        .filter(|memo| memo.read_at.elapsed() < INSTALLATION_LOOKUP_WINDOW)
        .map(|memo| memo.installation.clone())
}

fn remember_package_registration(
    package_prefixes: &[&str],
    installation: Option<DesktopInstallation>,
) {
    if let Ok(mut cache) = package_registration_cache().lock() {
        cache.insert(
            package_prefixes.join(" "),
            PackageRegistrationMemo {
                installation,
                read_at: Instant::now(),
            },
        );
    }
}

fn resolve_desktop_app(search: &DesktopAppSearch) -> Option<DesktopInstallation> {
    #[cfg(target_os = "macos")]
    {
        return macos_application_installation(search);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let user_profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
        resolve_desktop_app_in(
            &installation_roots(),
            local_app_data.as_deref(),
            user_profile.as_deref(),
            search,
        )
        .map(|(_, installation)| installation)
    }
}

#[cfg(target_os = "macos")]
fn macos_application_installation(search: &DesktopAppSearch) -> Option<DesktopInstallation> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = user_home_directory() {
        roots.push(home.join("Applications"));
    }
    macos_application_installation_in(search, &roots)
}

#[cfg(target_os = "macos")]
fn macos_application_installation_in(
    search: &DesktopAppSearch,
    roots: &[PathBuf],
) -> Option<DesktopInstallation> {
    let mut names: Vec<&str> = Vec::new();
    for display_name in search.display_names {
        for name in [
            Some(*display_name),
            display_name.strip_prefix("OpenAI "),
            display_name.strip_prefix("Google "),
            display_name.strip_suffix(" Desktop"),
        ]
        .into_iter()
        .flatten()
        {
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for root in roots {
        for name in &names {
            let bundle = root.join(format!("{name}.app"));
            let info_path = bundle.join("Contents").join("Info.plist");
            let Ok(plist) = plist::Value::from_file(info_path) else {
                continue;
            };
            let Some(info) = plist.as_dictionary() else {
                continue;
            };
            let Some(executable) = info
                .get("CFBundleExecutable")
                .and_then(plist::Value::as_string)
                .filter(|name| {
                    !name.is_empty()
                        && Path::new(name)
                            .components()
                            .all(|component| matches!(component, Component::Normal(_)))
                })
            else {
                continue;
            };
            let executable_path = bundle.join("Contents").join("MacOS").join(executable);
            if !executable_path.is_file() {
                continue;
            }
            let version = info
                .get("CFBundleShortVersionString")
                .and_then(plist::Value::as_string)
                .or_else(|| {
                    info.get("CFBundleVersion")
                        .and_then(plist::Value::as_string)
                })
                .filter(|version| version_numbers(version).is_some())
                .map(str::to_owned);
            return Some(DesktopInstallation {
                executable: Some(executable_path),
                version,
            });
        }
    }
    None
}

fn desktop_application_command(executable: &Path) -> Command {
    #[cfg(target_os = "macos")]
    {
        let bundle = executable
            .ancestors()
            .find(|path| path.extension().is_some_and(|extension| extension == "app"))
            .unwrap_or(executable);
        let mut command = Command::new("/usr/bin/open");
        command.arg("-a").arg(bundle).arg("--args");
        command
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new(executable)
    }
}

/// Deliver an install bundle as a document. macOS sends existing applications
/// an open-file event; --args only supplies argv when launching a new process.
fn desktop_document_command(executable: &Path, document: &Path) -> Command {
    #[cfg(target_os = "macos")]
    {
        let bundle = executable
            .ancestors()
            .find(|path| path.extension().is_some_and(|extension| extension == "app"))
            .unwrap_or(executable);
        let mut command = Command::new("/usr/bin/open");
        command.arg("-a").arg(bundle).arg(document);
        command
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut command = desktop_application_command(executable);
        command.arg(document);
        command
    }
}

fn spawn_desktop_command(command: Command) -> Result<(), String> {
    #[cfg(unix)]
    {
        crate::broker_tray_control::spawn_owned(command)
    }
    #[cfg(not(unix))]
    {
        let mut command = command;
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

const CLAUDE_DESKTOP_SEARCH: DesktopAppSearch = DesktopAppSearch {
    executable_names: &["claude.exe"],
    relative_candidates: &[r"AnthropicClaude\claude.exe", r"Claude\Claude.exe"],
    display_names: &["Claude", "Claude Desktop"],
    self_records: &[],
    package_prefixes: &["Claude_", "Anthropic.Claude_"],
    package_executables: &[r"app\claude.exe", "claude.exe"],
};

const CHATGPT_DESKTOP_SEARCH: DesktopAppSearch = DesktopAppSearch {
    // Codex.exe on PATH is the interactive CLI (including the copy bundled by
    // the desktop app), not the desktop window. Looking it up here made Open
    // launch a terminal with TERM=dumb instead of the installed ChatGPT app.
    // Keep Codex.exe in the known desktop and MSIX locations below, but never
    // accept an arbitrary PATH/App Paths/Store alias match as the desktop UI.
    executable_names: &["ChatGPT.exe"],
    relative_candidates: &[
        r"ChatGPT\ChatGPT.exe",
        r"OpenAI\ChatGPT\ChatGPT.exe",
        r"Codex\Codex.exe",
        r"OpenAI\Codex\Codex.exe",
    ],
    display_names: &["ChatGPT", "OpenAI ChatGPT", "Codex", "OpenAI Codex"],
    self_records: &[],
    package_prefixes: &["OpenAI.ChatGPT_", "OpenAI.Codex_"],
    package_executables: &[
        r"app\ChatGPT.exe",
        r"app\Codex.exe",
        "ChatGPT.exe",
        "Codex.exe",
    ],
};

const ANTIGRAVITY_SEARCH: DesktopAppSearch = DesktopAppSearch {
    executable_names: &["Antigravity.exe"],
    relative_candidates: &[
        r"Antigravity\Antigravity.exe",
        r"Google\Antigravity\Antigravity.exe",
    ],
    display_names: &["Antigravity", "Google Antigravity"],
    self_records: &[],
    package_prefixes: &[],
    package_executables: &[],
};

/// LM Studio, which ships as `Bionic` since its rename.
///
/// The legacy spellings stay: an installation made before the rename still has
/// `LM Studio.exe` under `LM Studio\`, and nothing about the rename removes
/// it. Both layouts resolve here, and which one is found also decides the name
/// the row shows: the version this search reports is filed into a numbering
/// series by [`app_version_series`], and the row is headed by that series'
/// product name. So an owner who still has the old build is still shown
/// "LM Studio", and everyone else is shown "Bionic".
///
/// Not every Bionic build registers an `Uninstall` key, an MSIX package, or
/// an `App Paths` entry; the direct candidate and the self-record are what
/// find those that don't, and the `Bionic` display name is here for the
/// installer build that does register one.
const LM_STUDIO_SEARCH: DesktopAppSearch = DesktopAppSearch {
    executable_names: &["Bionic.exe", "LM Studio.exe"],
    relative_candidates: &[
        r"Bionic\Bionic.exe",
        r"LM Studio\LM Studio.exe",
        r"LMStudio\LM Studio.exe",
    ],
    display_names: &["Bionic", "LM Studio"],
    self_records: &[SelfInstallRecord {
        relative_path: r".lmstudio\.internal\app-install-location.json",
        member: "path",
    }],
    package_prefixes: &[],
    package_executables: &[],
};

/// LM Studio numbers its releases in two unrelated series, so one threshold
/// cannot describe both halves of the product.
///
/// **LM Studio, `0.x`.** 0.3.17 is the release that made this pairing possible
/// at all.
/// An installation made before the rename still carries that numbering and is
/// still held to it.
///
/// **Bionic, `1.x`.** The rename restarted the count, and `0.3.17` says nothing
/// about this line - it only happens to pass because 1 is greater than 0. The
/// minimum here is the floor of the series itself, so every Bionic build
/// qualifies, and that is a deliberate choice against two pieces of evidence:
///
/// * Nothing in this repository establishes a Bionic version that cannot do
///   what QC Remote needs.
/// * The version resource is coarser than any fine threshold could use.
///   `executable_version` reads VS_FIXEDFILEINFO, and Bionic's fixed block
///   is `1.1.0.0` while the version *string* beside it reads `1.1.6+3`: the
///   patch and build fields are zeroed. A threshold anywhere above `1.1.0`
///   would therefore reject a current install - exactly the false "App update
///   required" this table exists to prevent.
///
/// Raise the Bionic minimum only when a specific version is known to lack
/// something QC Remote needs. Do not raise it because a newer one exists.
const LM_STUDIO_SUPPORTED_VERSIONS: &[AppVersionSeries] = &[
    AppVersionSeries {
        name: BIONIC_SERIES_NAME,
        series_begins_at: "1.0.0",
        minimum: "1.0.0",
    },
    AppVersionSeries {
        name: "LM Studio",
        series_begins_at: "0",
        minimum: "0.3.17",
    },
];

/// Jan. The binary on disk is lowercase `jan.exe` while its `DisplayIcon` and
/// this candidate say `Jan.exe`; Windows compares paths case-insensitively, so
/// one spelling resolves both, and that is why Jan was already recognised.
#[cfg(any(target_os = "windows", target_os = "macos", test))]
const JAN_SEARCH: DesktopAppSearch = DesktopAppSearch {
    executable_names: &["Jan.exe"],
    relative_candidates: &[r"Jan\Jan.exe"],
    display_names: &["Jan"],
    self_records: &[],
    package_prefixes: &[],
    package_executables: &[],
};

/// Jan has renamed nothing and renumbered nothing: it was on `0.6.x` when
/// 0.6.10 was chosen, and the current release reports `0.8.4` - the same
/// line continued. One series, so one row - and the row says so, rather than
/// leaving the reader to infer it from a bare string.
const JAN_SUPPORTED_VERSIONS: &[AppVersionSeries] = &[AppVersionSeries {
    name: "Jan",
    series_begins_at: "0",
    minimum: "0.6.10",
}];

fn claude_desktop_installation() -> Option<DesktopInstallation> {
    resolve_desktop_app(&CLAUDE_DESKTOP_SEARCH)
}

fn claude_uri_handler_registered() -> bool {
    #[cfg(target_os = "windows")]
    {
        // Both the classic installer and Anthropic's MSIX package register the
        // documented claude:// protocol. The MSIX executable itself lives in a
        // protected WindowsApps directory, so the protocol is the stable signal.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        run_with_timeout(
            Command::new(crate::system_tools::reg_exe())
                // Packaged MSIX apps register the protocol root but do not expose
                // the classic shell/open/command subkey used by unpackaged apps.
                .args(["query", r"HKCR\claude"])
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW),
            EXTERNAL_TOOL_TIMEOUT,
        )
        .is_ok_and(|output| output.status.success())
    }

    #[cfg(not(target_os = "windows"))]
    false
}

/// The launch paths ask for the Store executable on its own, so each keeps a
/// name - but both now go through the one shared lookup rather than carrying a
/// PowerShell command and an executable list of their own.
fn claude_msix_path() -> Option<PathBuf> {
    registered_msix_path(&CLAUDE_DESKTOP_SEARCH)
}

fn chatgpt_desktop_installation() -> Option<DesktopInstallation> {
    resolve_desktop_app(&CHATGPT_DESKTOP_SEARCH)
}

#[cfg(target_os = "windows")]
fn chatgpt_msix_path() -> Option<PathBuf> {
    registered_msix_path(&CHATGPT_DESKTOP_SEARCH)
}

fn package_directory_contains(directory: &Path, package_prefixes: &[&str]) -> bool {
    fs::read_dir(directory).ok().is_some_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            package_prefixes
                .iter()
                .any(|prefix| name.starts_with(&prefix.to_ascii_lowercase()))
        })
    })
}

fn chatgpt_package_registered() -> bool {
    let package_prefixes = ["OpenAI.Codex_", "OpenAI.ChatGPT_"];
    if std::env::var_os("LOCALAPPDATA").is_some_and(|local_app_data| {
        package_directory_contains(
            &PathBuf::from(local_app_data).join("Packages"),
            &package_prefixes,
        )
    }) {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        package_prefixes.iter().any(|prefix| {
            run_with_timeout(Command::new(crate::system_tools::reg_exe())
                .args([
                    "query",
                    r"HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages",
                    "/f",
                    prefix.trim_end_matches('_'),
                    "/k",
                ])
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                , EXTERNAL_TOOL_TIMEOUT)
                .is_ok_and(|output| output.status.success())
        })
    }
    #[cfg(not(target_os = "windows"))]
    false
}

fn chatgpt_desktop_installed() -> bool {
    chatgpt_desktop_installation().is_some()
        || chatgpt_package_registered()
        || codex_cli_executable().is_some()
}

fn codex_cli_executable() -> Option<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        where_command("codex").into_iter().next()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn antigravity_installation() -> Option<DesktopInstallation> {
    resolve_desktop_app(&ANTIGRAVITY_SEARCH)
}

fn lm_studio_installation() -> Option<DesktopInstallation> {
    resolve_desktop_app(&LM_STUDIO_SEARCH)
}

fn jan_installation() -> Option<DesktopInstallation> {
    #[cfg(target_os = "linux")]
    {
        // Jan's Linux distribution is sandboxed. Detecting its Flatpak ref
        // does not prove it can launch this host executable over stdio or read
        // host-managed configuration, so Linux pairing remains unavailable.
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        resolve_desktop_app(&JAN_SEARCH)
    }
}

#[cfg(target_os = "windows")]
fn running_antigravity_language_server() -> Result<RunningAntigravityLanguageServer, String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DISCOVERY_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$process = Get-CimInstance Win32_Process -Filter "Name='language_server.exe'" |
  Where-Object { $_.CommandLine -match '--override_ide_name\s+antigravity(?:\s|$)' } |
  Select-Object -First 1
if (-not $process) { exit 3 }
$version = [regex]::Match($process.CommandLine, '--override_ide_version\s+(\S+)').Groups[1].Value
$token = [regex]::Match($process.CommandLine, '--csrf_token\s+(\S+)').Groups[1].Value
if (-not $version -or -not $token) { exit 4 }
$ports = @(Get-NetTCPConnection -State Listen -OwningProcess $process.ProcessId |
  Select-Object -ExpandProperty LocalPort |
  Sort-Object -Unique)
[pscustomobject]@{
  executablePath = $process.ExecutablePath
  version = $version
  csrfToken = $token
  ports = $ports
} | ConvertTo-Json -Compress
"#;

    let output = run_with_timeout(
        Command::new(crate::system_tools::powershell_exe())
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                DISCOVERY_SCRIPT,
            ])
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_TIMEOUT,
    )
    .map_err(|error| format!("Could not inspect the running Antigravity service: {error}"))?;
    if !output.status.success() {
        return Err("Google Antigravity is not running.".to_string());
    }
    let server: RunningAntigravityLanguageServer = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Could not read the running Antigravity service: {error}"))?;
    validate_antigravity_language_server(&server)?;
    Ok(server)
}

#[cfg(not(target_os = "windows"))]
fn running_antigravity_language_server() -> Result<RunningAntigravityLanguageServer, String> {
    Err("Automatic Antigravity refresh is available on Windows only.".to_string())
}

#[cfg(target_os = "windows")]
fn validate_antigravity_language_server(
    server: &RunningAntigravityLanguageServer,
) -> Result<(), String> {
    validate_antigravity_ports(&server.ports)?;
    let app_executable = antigravity_installation()
        .and_then(|installation| installation.executable)
        .ok_or_else(|| "Google Antigravity is not installed for this user account.".to_string())?;
    let app_directory = app_executable
        .parent()
        .ok_or_else(|| "Could not validate the Google Antigravity installation.".to_string())?;
    let expected_server = app_directory
        .join("resources")
        .join("bin")
        .join("language_server.exe");
    if !server
        .executable_path
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected_server.to_string_lossy())
    {
        return Err("The running Antigravity service does not belong to the installed app.".into());
    }
    // Antigravity keeps this protobuf contract across its own releases, and an
    // exact-version allowlist stranded every working route the moment the IDE
    // updated, refusing the refresh as unverified, and
    // Reconnect - which goes through here too - could never recover. What the
    // endpoint is trusted on is unchanged: the token and ports come from the
    // installed executable's own process, checked above, the request stays on
    // loopback, and an endpoint or payload that ever changes fails closed.
    if server.version.len() > 64 || version_numbers(&server.version).is_none() {
        return Err("The running Antigravity service returned an invalid version.".into());
    }
    if server.csrf_token.is_empty()
        || server.csrf_token.len() > 128
        || !server
            .csrf_token
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-' || value == b'_')
    {
        return Err("The running Antigravity service returned an invalid security token.".into());
    }
    Ok(())
}

/// Asks a running Antigravity to re-read its MCP configuration.
///
/// **This is user-initiated only, and deliberately so.** Both callers are a
/// button: Pair and Reconnect, through
/// [`ensure_and_refresh_antigravity_local_integration`], which rewrites the
/// route and then asks the host to read what was just written. Starting a
/// process on the user's behalf is a real act, and it is justified at exactly
/// that moment and no other.
///
/// An automatic pass at app start was considered and rejected. Two reasons,
/// both about the trigger rather than the mechanism:
///
/// - **Absence is the universal state at start-up.** Every host's bridge is a
///   child of the host, and nothing has announced itself yet, so "paired but
///   no bridge attached" is true of every provider on every launch. A pass
///   keyed on it would not be repairing an exception; it would be spawning
///   processes behind the user's back as a matter of routine.
/// - **The one positive signal available does not distinguish the case.**
///   Antigravity's own state document reports a departed bridge as
///   `MCP_SERVER_STATUS_ERROR` / `exit status 0xffffffff`, and that is exactly
///   what a bridge killed with the app produces - which is how an STDIO bridge
///   normally ends, because every host's bridge runs the same `qc-remote.exe`
///   image the app does and a by-name stop ends all of them.
///   So the error status cannot tell a broken route from a bridge that ended
///   the ordinary way, and keying on it would fire just as routinely.
///
/// What the app owes the user here is an honest row, not a respawn: the
/// durable attach record in `local_access.rs` is what makes a host that has
/// worked before read as idle rather than as never connected. If a trigger is
/// ever added, it must rest on evidence that this host *tried and failed* to
/// start a bridge - never on the absence of one.
///
/// [`ANTIGRAVITY_STATE_RPC_PATH`] is where that evidence would come from, and
/// the shape it has to take is worth writing down because the obvious reading
/// of it is wrong. A state document reporting
/// `MCP_SERVER_STATUS_ERROR` / `exit status 0xffffffff` is **not** on its own a
/// fault to show the user: that is what the host records when its bridge is
/// killed along with the app, which is how an STDIO bridge ordinarily ends
/// here. Surfacing it as a fault would put the same false accusation this
/// record exists to remove back on the row in a new costume, and it would fire
/// after every app restart.
///
/// The discriminator is the `tools` array beside it. A state carrying an error
/// *and* a populated tool list is a bridge that connected, enumerated this
/// app's tools and then ended - history, and good news. A state carrying an
/// error and no tools at all would be a host that tried and never reached us,
/// which is a real `action-required`. That second case has not been observed
/// yet, so the field's behaviour in it is unknown - whether
/// `tools` comes back empty, absent, or stale from an earlier success - and
/// building the distinction on the unobserved half would be guessing again.
/// Capture one state document for a route that has never worked before
/// writing this, bound the length of any `error` string before it reaches the
/// UI, and keep it incapable of producing a `Connected` row: only a heartbeat
/// does that.
async fn refresh_running_antigravity_mcp() -> Value {
    let deadline = Instant::now() + ANTIGRAVITY_OPERATION_BUDGET;
    let server = match discover_antigravity_until(deadline).await {
        Ok(server) => server,
        Err(error) => {
            return json!({
                "refreshAttempted": false,
                "refreshSucceeded": false,
                "connectionActive": antigravity_bridge_connected(),
                "refreshError": error,
            });
        }
    };

    // Antigravity's local certificate is generated per session. The endpoint is
    // accepted only after discovery has tied its owning process and token to the
    // installed Antigravity executable above, and every request remains loopback.
    let client = match crate::loopback_http::loopback_self_signed_client_builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return json!({
                "refreshAttempted": false,
                "refreshSucceeded": false,
                "connectionActive": antigravity_bridge_connected(),
                "refreshError": format!("Could not prepare the Antigravity refresh: {error}"),
            });
        }
    };

    let mut last_error = "The running Antigravity service did not accept the refresh.".to_string();
    let mut refreshed = false;
    'ports: for port in &server.ports {
        let url = format!("https://127.0.0.1:{port}{ANTIGRAVITY_REFRESH_RPC_PATH}");
        for attempt in 0..2 {
            let budget = match remaining(deadline) {
                Ok(budget) => budget,
                Err(error) => {
                    last_error = error;
                    break 'ports;
                }
            };
            let request = client
                .post(&url)
                .timeout(budget)
                .header("x-codeium-csrf-token", &server.csrf_token)
                .header("connect-protocol-version", "1")
                .json(&json!({ "shallow": false, "serverName": "qc-remote" }));
            match within_antigravity_budget(deadline, async {
                request.send().await.map_err(|error| error.to_string())
            })
            .await
            {
                Ok(response) if response.status().is_success() => {
                    refreshed = true;
                    break 'ports;
                }
                Ok(response) => {
                    last_error = format!(
                        "Google Antigravity rejected the QC Remote refresh (HTTP {}).",
                        response.status().as_u16()
                    );
                }
                Err(error) => {
                    last_error =
                        format!("Could not reach the running Antigravity service: {error}");
                }
            }
            if attempt == 0 {
                if let Ok(left) = remaining(deadline) {
                    tokio::time::sleep(left.min(Duration::from_millis(250))).await;
                }
            }
        }
    }

    if refreshed {
        while remaining(deadline).is_ok() {
            if antigravity_bridge_connected()
                || antigravity_server_reports_ready(&server, &client, deadline)
                    .await
                    .unwrap_or(false)
            {
                return json!({
                    "refreshAttempted": true,
                    "refreshSucceeded": true,
                    "connectionActive": true,
                });
            }
            if let Ok(left) = remaining(deadline) {
                tokio::time::sleep(left.min(Duration::from_millis(250))).await;
            }
        }
        return json!({
            "refreshAttempted": true,
            "refreshSucceeded": true,
            "connectionActive": false,
            "refreshError": "Google Antigravity refreshed QC Remote, but its connection did not become ready.",
        });
    }

    json!({
        "refreshAttempted": true,
        "refreshSucceeded": false,
        "connectionActive": antigravity_bridge_connected(),
        "refreshError": last_error,
    })
}

// Antigravity state uses the same local-document software envelope as its
// persisted configuration; this is not a vendor schema or firmware maximum.
async fn read_antigravity_state_response(
    mut response: reqwest::Response,
    deadline: Instant,
    max_bytes: usize,
) -> Result<Value, String> {
    let oversized =
        || "Antigravity's MCP state exceeded its software document envelope.".to_string();
    remaining(deadline)?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(oversized());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = within_antigravity_budget(deadline, async {
        response.chunk().await.map_err(|error| error.to_string())
    })
    .await?
    {
        // Metadata is only an early refusal. Bound every actual chunk BEFORE
        // retaining it, including chunked/close-delimited bodies and growth.
        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
            return Err(oversized());
        }
        bytes.extend_from_slice(&chunk);
    }
    remaining(deadline)?;
    let document = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    // Parsing is synchronous; do not turn work completed after the ordinary
    // total deadline into a fresh ready observation. No hard CPU cancellation.
    remaining(deadline)?;
    Ok(document)
}

/// Whether Antigravity's own MCP state names this app's route as ready.
///
/// The heartbeat is the app's evidence that a bridge is alive, but it can only
/// speak for a child that knows which host launched it.  Antigravity's state
/// answers the same question from the other end, so a route it is actually
/// serving cannot read as unconnected.
fn antigravity_state_has_current_qc_remote(document: &Value, expected_executable: &Path) -> bool {
    document
        .get("states")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|state| {
            let Some(spec) = state.get("spec").and_then(Value::as_object) else {
                return false;
            };
            state.get("status").and_then(Value::as_str) == Some("MCP_SERVER_STATUS_READY")
                && spec.get("serverName").and_then(Value::as_str) == Some(CLAUDE_EXTENSION_NAME)
                && spec
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| {
                        command.eq_ignore_ascii_case(expected_executable.to_string_lossy().as_ref())
                    })
                && spec
                    .get("args")
                    .and_then(Value::as_array)
                    .is_some_and(|args| {
                        args.iter()
                            .filter_map(Value::as_str)
                            .any(|argument| argument == "--local-mcp-bridge")
                    })
        })
}

async fn antigravity_server_reports_ready(
    server: &RunningAntigravityLanguageServer,
    client: &reqwest::Client,
    deadline: Instant,
) -> Result<bool, String> {
    let expected_executable = antigravity_bridge_path()?;
    let mut accepted_response = false;
    let mut last_error = "The running Antigravity service did not report MCP state.".to_string();
    for port in &server.ports {
        let budget = remaining(deadline)?;
        let url = format!("https://127.0.0.1:{port}{ANTIGRAVITY_STATE_RPC_PATH}");
        let request = client
            .post(url)
            .timeout(budget)
            .header("x-codeium-csrf-token", &server.csrf_token)
            .header("connect-protocol-version", "1")
            .json(&json!({}));
        match within_antigravity_budget(deadline, async {
            request.send().await.map_err(|error| error.to_string())
        })
        .await
        {
            Ok(response) if response.status().is_success() => {
                accepted_response = true;
                match read_antigravity_state_response(response, deadline, CLIENT_DOCUMENT_MAX_BYTES)
                    .await
                {
                    Ok(document)
                        if antigravity_state_has_current_qc_remote(
                            &document,
                            &expected_executable,
                        ) =>
                    {
                        return Ok(true);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        last_error = format!("Could not read Antigravity's MCP state: {error}");
                    }
                }
            }
            Ok(response) => {
                last_error = format!(
                    "Google Antigravity rejected the MCP state request (HTTP {}).",
                    response.status().as_u16()
                );
            }
            Err(error) => {
                last_error = format!("Could not reach the running Antigravity service: {error}");
            }
        }
    }
    if accepted_response {
        Ok(false)
    } else {
        Err(last_error)
    }
}

/// The heartbeat first, because it costs nothing and covers every host.  Only
/// when it is silent is Antigravity asked directly, and only the LOCAL panel
/// polls this - the process scan behind it must not run on the app's own clock.
async fn antigravity_runtime_connected_until(deadline: Instant) -> bool {
    if antigravity_bridge_connected() {
        return true;
    }
    let server = match discover_antigravity_until(deadline).await {
        Ok(server) => server,
        _ => return false,
    };
    let client = match crate::loopback_http::loopback_self_signed_client_builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    antigravity_server_reports_ready(&server, &client, deadline)
        .await
        .unwrap_or(false)
}

fn user_home_directory() -> Option<PathBuf> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    {
        qc_native_platform::app_store::user_home()
    }
    #[cfg(not(all(target_os = "macos", feature = "macos-app-store")))]
    {
        #[cfg(target_os = "windows")]
        let variable = "USERPROFILE";
        #[cfg(not(target_os = "windows"))]
        let variable = "HOME";
        std::env::var_os(variable)
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
    }
}

#[cfg(all(target_os = "macos", feature = "macos-app-store"))]
fn local_client_folder(client: &str) -> Result<PathBuf, String> {
    let home = user_home_directory().ok_or("The user home directory is unavailable")?;
    let folder = match client {
        "claude" => Some(home.join("Library/Application Support/Claude")),
        "codex" => codex_config_path().and_then(|p| p.parent().map(Path::to_path_buf)),
        "antigravity" => Some(home.join(".gemini")),
        "lm-studio" => Some(home.join(".lmstudio")),
        "jan" => Some(home.join("Library/Application Support/Jan")),
        _ => None,
    }
    .ok_or("The client's configuration folder is unavailable")?;
    Ok(folder)
}

#[cfg(all(target_os = "macos", feature = "macos-app-store"))]
fn annotate_folder_access(status: &mut Value, client: &str) {
    let key = match client {
        "claude" => "claudeInstalled",
        "codex" => "chatgptInstalled",
        "antigravity" => "antigravityInstalled",
        "lm-studio" => "lmStudioInstalled",
        "jan" => "janInstalled",
        _ => return,
    };
    if status[key] != true || status["state"] == "app-update-required" {
        return;
    }
    let Ok(folder) = local_client_folder(client) else {
        return;
    };
    let mut needed = !qc_native_platform::app_store::has_folder_access(&folder);
    if !needed && client == "jan" {
        if let Some(data) = jan_data_directory() {
            needed = !qc_native_platform::app_store::has_folder_access(&data);
        }
    }
    status["folderAccessRequired"] = Value::Bool(needed);
}

#[cfg(all(target_os = "macos", feature = "macos-app-store"))]
fn authorize_local_client_folder(client: &str) -> Result<(), String> {
    let folder = local_client_folder(client)?;
    qc_native_platform::app_store::authorize_folder(&folder)?;
    if client == "jan" {
        if let Some(data) = jan_data_directory().filter(|p| !p.starts_with(&folder)) {
            qc_native_platform::app_store::authorize_folder(&data)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn xdg_directory_from(value: Option<&Path>, home: &Path, fallback: &str) -> PathBuf {
    value
        .filter(|path| path.is_absolute())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(fallback))
}

#[cfg(target_os = "linux")]
fn xdg_config_home() -> Option<PathBuf> {
    let home = user_home_directory()?;
    Some(xdg_directory_from(
        std::env::var_os("XDG_CONFIG_HOME")
            .as_deref()
            .map(Path::new),
        &home,
        ".config",
    ))
}

#[cfg(target_os = "linux")]
fn xdg_data_home() -> Option<PathBuf> {
    let home = user_home_directory()?;
    Some(xdg_directory_from(
        std::env::var_os("XDG_DATA_HOME").as_deref().map(Path::new),
        &home,
        ".local/share",
    ))
}

#[cfg(target_os = "macos")]
fn xdg_data_home() -> Option<PathBuf> {
    user_home_directory().map(|home| home.join("Library").join("Application Support"))
}

fn codex_config_path_in(home: &Path, codex_home: Option<&Path>) -> PathBuf {
    let default_codex_home = home.join(".codex");
    let codex_directory = codex_home
        .filter(|path| path.is_absolute())
        .unwrap_or(&default_codex_home);
    codex_directory.join("config.toml")
}

fn codex_config_path() -> Option<PathBuf> {
    let home = user_home_directory()?;
    Some(codex_config_path_in(
        &home,
        std::env::var_os("CODEX_HOME").as_deref().map(Path::new),
    ))
}

fn antigravity_config_path() -> Option<PathBuf> {
    user_home_directory().map(|home| home.join(".gemini").join("config").join("mcp_config.json"))
}

fn antigravity_settings_path() -> Option<PathBuf> {
    user_home_directory().map(|home| home.join(".gemini").join("config").join("config.json"))
}

/// Where a **pre-rename** LM Studio keeps its MCP servers.
///
/// Still the right file for a 0.3.x install, and no longer the right file for
/// anything else: Bionic does not read it.
fn lm_studio_config_path() -> Option<PathBuf> {
    user_home_directory().map(|home| home.join(".lmstudio").join("mcp.json"))
}

/// Where **Bionic** keeps its MCP servers.
///
/// `~\.lmstudio\mcp.json` and `~\.lmstudio\ng-mcp.json` are not read by
/// Bionic, so there is no fallback to try and nothing to guess at.
fn bionic_config_path() -> Option<PathBuf> {
    user_home_directory().map(|home| {
        home.join(".lmstudio")
            .join("apps")
            .join("bionic")
            .join(".internal")
            .join("ng-mcp.json")
    })
}

/// Whether an installation reads Bionic's document rather than the pre-rename
/// one.
///
/// Decided from the numbering series the install already belongs to, which is
/// the same reading the update gate and the row's label are taken from - so
/// "which file does this install read" cannot disagree with "what is this
/// application called". Adding a second discriminator, such as which of the two
/// executable names the search matched, would be a way for those two answers to
/// drift.
///
/// An install whose version this app cannot read belongs to no series, and is
/// taken to be Bionic: that is what https://lmstudio.ai/download ships, and it
/// is what `lmStudioAppName` already calls such an install, so the document
/// family and the label stay one reading in that case too.
fn lm_studio_reads_bionic_document(version: Option<&str>) -> bool {
    version
        .and_then(|version| app_version_series(version, LM_STUDIO_SUPPORTED_VERSIONS))
        .is_none_or(|series| series.name == BIONIC_SERIES_NAME)
}

/// Which MCP document this machine's LM Studio installation reads.
struct LmStudioMcpDocument {
    path: PathBuf,
    /// Bionic's array-shaped `ng-mcp.json`, rather than the pre-rename
    /// `mcpServers` map.
    bionic: bool,
}

/// Resolve the document once, for every caller that touches it.
///
/// The status read, Pair, Unpair and the start-up refresh all come through
/// here, so the choice of file is made in one place from one input. A provider
/// that answered this question twice is how a pairing gets written to a file
/// the status read then does not inspect.
fn lm_studio_mcp_document(
    installation: Option<&DesktopInstallation>,
) -> Result<LmStudioMcpDocument, String> {
    let bionic =
        lm_studio_reads_bionic_document(installation.and_then(|value| value.version.as_deref()));
    let path = if bionic {
        bionic_config_path()
    } else {
        lm_studio_config_path()
    }
    .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    Ok(LmStudioMcpDocument { path, bionic })
}

#[cfg(target_os = "windows")]
fn jan_data_directory() -> Option<PathBuf> {
    let app_data = std::env::var_os("APPDATA").map(PathBuf::from)?;
    let settings = app_data.join("Jan").join("settings.json");
    if let Ok(contents) = read_client_document_text(settings) {
        if let Ok(document) = serde_json::from_str::<Value>(&contents) {
            if let Some(path) = document.get("data_folder").and_then(Value::as_str) {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    return Some(path);
                }
            }
        }
    }
    Some(app_data.join("Jan").join("data"))
}

#[cfg(target_os = "linux")]
fn jan_data_directory() -> Option<PathBuf> {
    let config_home = xdg_config_home()?;
    let data_home = xdg_data_home()?;
    Some(jan_data_directory_in(&config_home, &data_home))
}

#[cfg(target_os = "linux")]
fn jan_data_directory_in(config_home: &Path, data_home: &Path) -> PathBuf {
    let settings = config_home.join("Jan").join("settings.json");
    if let Ok(contents) = read_client_document_text(settings) {
        if let Ok(document) = serde_json::from_str::<Value>(&contents) {
            if let Some(path) = document.get("data_folder").and_then(Value::as_str) {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    return path;
                }
            }
        }
    }
    data_home.join("Jan").join("data")
}

#[cfg(target_os = "macos")]
fn jan_data_directory() -> Option<PathBuf> {
    let data_home = xdg_data_home()?;
    let settings = data_home.join("Jan").join("settings.json");
    if let Ok(contents) = read_client_document_text(settings) {
        if let Ok(document) = serde_json::from_str::<Value>(&contents) {
            if let Some(path) = document.get("data_folder").and_then(Value::as_str) {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    return Some(path);
                }
            }
        }
    }
    Some(data_home.join("Jan").join("data"))
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn jan_data_directory() -> Option<PathBuf> {
    None
}

fn jan_config_path() -> Option<PathBuf> {
    jan_data_directory().map(|directory| directory.join("mcp_config.json"))
}

fn legacy_gemini_settings_path() -> Option<PathBuf> {
    user_home_directory().map(|home| home.join(".gemini").join("settings.json"))
}

fn canonical_integration_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|directory| directory.join("QC Remote").join("integrations"))
            .ok_or_else(|| "Windows local application data is unavailable.".to_string())
    }
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    {
        let state = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&state)?;
        Ok(state.join("integrations"))
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "macos", feature = "macos-app-store")
    )))]
    {
        xdg_data_home()
            .map(|directory| directory.join("QC Remote").join("integrations"))
            .ok_or_else(|| "The user data directory is unavailable.".to_string())
    }
}

fn durable_antigravity_integration_directory() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|directory| directory.join(WINDOWS_APP_IDENTIFIER).join("integrations"))
            .ok_or_else(|| "Windows local application data is unavailable.".to_string())
    }
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    {
        let state = crate::broker_paths::app_local_data_dir();
        qc_native_platform::paths_macos::prepare_directory(&state)?;
        Ok(state.join("integrations"))
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "macos", feature = "macos-app-store")
    )))]
    {
        xdg_data_home()
            .map(|directory| directory.join(WINDOWS_APP_IDENTIFIER).join("integrations"))
            .ok_or_else(|| "The user data directory is unavailable.".to_string())
    }
}

/// The paired launcher is the installed QC Remote executable itself.
///
/// Releases through 0.1.142 pointed Antigravity at a versioned byte-for-byte
/// copy of it under `integrations`.  Antigravity refuses to start that
/// copy while accepting the installed executable, so the route never came up
/// and Reconnect - which only re-copied the same rejected file - could not
/// recover it.  One stable command is what every other native route already
/// uses and what `antigravity/mcp_config.example.json` has always documented,
/// so an in-place upgrade no longer leaves the paired command naming a file
/// that is about to be swept.  The provider-neutral bridge is still selected
/// with `--local-mcp-bridge`.
fn antigravity_bridge_path() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ConfiguredChatgptRoute {
    configured: bool,
    launcher_current: bool,
    approval_mode_current: bool,
    /// The app version the configured managed route was written by, read from
    /// its own client identity.  The route runs QC Remote's native bridge, so
    /// the separately installed `local-mcp-bridge.cjs` - which belongs to the
    /// manual Codex and VS Code node routes - can never speak for it.
    managed_version: Option<String>,
}

/// Does this argument list name `client_id` the way a managed route does?
///
/// A route is only one this app would write when it carries the identity in
/// both channels, and this is the channel that decides whether the app ever
/// sees the client. Every managed route used to name itself in `env` alone.
/// Antigravity keeps that environment in its own MCP server state without
/// handing it to the native STDIO child, and the ChatGPT/Codex host does the
/// same: a bridge whose parent was `codex.exe` ran with `--local-mcp-bridge`
/// and nothing else on its command line, announced itself anonymously, and
/// left its row amber while its tools drove the hardware. So a pairing written
/// before the argument existed has to read as this app's own route that needs
/// rewriting - which is what `Update` does - rather than as current.
fn arguments_name_client(arguments: &[&str], client_id: &str) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == CLIENT_ID_ARGUMENT && pair[1].trim() == client_id)
}

fn configured_chatgpt_route(
    config_path: &Path,
    expected_executable: &Path,
) -> Result<ConfiguredChatgptRoute, String> {
    if !config_path.is_file() {
        return Ok(ConfiguredChatgptRoute::default());
    }
    let contents = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read ChatGPT's MCP configuration: {error}"))?;
    let document = contents
        .parse::<DocumentMut>()
        .map_err(|error| format!("ChatGPT's MCP configuration is invalid: {error}"))?;
    let Some(server) = document
        .get("mcp_servers")
        .and_then(Item::as_table)
        .and_then(|servers| servers.get(CLAUDE_EXTENSION_NAME))
        .and_then(Item::as_table_like)
    else {
        return Ok(ConfiguredChatgptRoute::default());
    };
    #[cfg(any(windows, target_os = "macos"))]
    if server.contains_key("http_headers_helper") {
        if !codex_http_approval_policy_supported(server) {
            return Err("The custom Codex approval policy is invalid for the supported client. It was preserved; review it in Codex settings.".into());
        }
        let current = crate::local_http_helper::directory()
            .and_then(|directory| {
                crate::local_http_binding::StablePort::read(
                    &directory.join(crate::local_http_helper::PORT_FILE),
                )
            })
            .and_then(|port| codex_http_route_table(expected_executable, CODEX_CLIENT_ID, port))
            .is_ok_and(|expected| {
                ["url", "http_headers_helper"].into_iter().all(|key| {
                    server.get(key).and_then(Item::as_str)
                        == expected.get(key).and_then(Item::as_str)
                })
            });
        let identity = server
            .get("http_headers")
            .and_then(Item::as_table_like)
            .and_then(|headers| headers.get(LOCAL_MCP_CLIENT_HEADER))
            .and_then(Item::as_str);
        return Ok(ConfiguredChatgptRoute {
            configured: true,
            launcher_current: current && identity == Some(CODEX_CLIENT_ID),
            // Valid owner's restrictive policy is current, not an invitation
            // to force approve. This flag never authorizes an MCP operation.
            approval_mode_current: true,
            managed_version: identity
                .and_then(|id| id.strip_prefix("codex:"))
                .map(str::to_owned),
        });
    }
    let command_matches = server
        .get("command")
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        .is_some_and(|command| {
            command
                .trim_matches('"')
                .eq_ignore_ascii_case(expected_executable.to_string_lossy().as_ref())
        });
    let arguments = server
        .get("args")
        .and_then(Item::as_value)
        .and_then(|item| item.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let bridge_mode = arguments.contains(&"--local-mcp-bridge");
    let names_client = arguments_name_client(&arguments, CODEX_CLIENT_ID);
    let approval_mode_current = server
        .get("default_tools_approval_mode")
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        == Some("approve");
    // Codex accepts `env` as either a sub-table or an inline table, so read
    // whichever the file on disk happens to carry.
    let client_identity = server
        .get("env")
        .and_then(Item::as_table_like)
        .and_then(|environment| environment.get(LOCAL_MCP_CLIENT_ENV))
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        .unwrap_or_default();
    let managed_version = (command_matches && bridge_mode)
        .then(|| client_identity.strip_prefix("codex:"))
        .flatten()
        .filter(|version| !version.is_empty() && version_numbers(version).is_some())
        .map(str::to_owned);
    Ok(ConfiguredChatgptRoute {
        configured: true,
        launcher_current: command_matches
            && bridge_mode
            && names_client
            && client_identity == CODEX_CLIENT_ID,
        approval_mode_current,
        managed_version,
    })
}

#[cfg(any(windows, target_os = "macos"))]
fn codex_http_approval_policy_supported(server: &dyn toml_edit::TableLike) -> bool {
    // Exact tested Codex0.163.0-alpha.11.2 AppToolApproval schema. Missing
    // values are valid defaults; unknown/malformed values remain diagnostic.
    let valid = |mode: Option<&Item>| {
        mode.is_none_or(|mode| {
            matches!(
                mode.as_str(),
                Some("auto" | "prompt" | "writes" | "approve")
            )
        })
    };
    if !valid(server.get("default_tools_approval_mode")) {
        return false;
    }
    let Some(tools) = server.get("tools") else {
        return true;
    };
    tools.as_table_like().is_some_and(|tools| {
        tools.iter().all(|(_, tool)| {
            tool.as_table_like().is_some_and(|tool| {
                valid(tool.get("approval_mode"))
                    && tool
                        .get("enabled")
                        .is_none_or(|enabled| enabled.as_bool().is_some())
            })
        })
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ConfiguredAntigravityRoute {
    configured: bool,
    launcher_current: bool,
    managed_version: Option<String>,
    enabled: bool,
}

/// `configured_antigravity_route` reads one file and cannot know the answers
/// that live in two others.  It used to return `permission_current: false` and
/// `legacy_route_present: false` as placeholders for its caller to overwrite,
/// which left three tests asserting a constant this function always returns.
/// The two settings-file facts live here instead, beside the route they qualify.
#[derive(Debug, PartialEq, Eq)]
struct AntigravityIntegration {
    route: ConfiguredAntigravityRoute,
    permission_current: bool,
    legacy_route_present: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct ConfiguredJsonMcpRoute {
    configured: bool,
    launcher_current: bool,
    managed_version: Option<String>,
    enabled: bool,
}

fn parse_json_mcp_config(contents: &str, label: &str) -> Result<Value, String> {
    if contents.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(contents)
        .map_err(|error| format!("{label}'s MCP configuration is invalid: {error}"))
}

/// The unpaired answer, which three readers return from four places.
fn unconfigured_json_mcp_route() -> ConfiguredJsonMcpRoute {
    ConfiguredJsonMcpRoute {
        configured: false,
        launcher_current: false,
        managed_version: None,
        enabled: true,
    }
}

/// Read a configured entry's command, arguments and identity, wherever the
/// host keeps them.
///
/// Legacy LM Studio and Jan hold the three at the top of the entry; Bionic
/// holds them in a nested `connection` object. Reading them in one place is
/// what keeps `launcher_current` and `managed_version` meaning the same thing
/// in every family - the version-recovery rule in particular decides this
/// app's own pairing from a stranger's file and must not be restated per
/// format.
///
/// `connection` may be any `Value`: a non-object carries none of the three, so
/// the entry reads as configured but not current, which is the right answer for
/// an entry somebody has rewritten.
fn configured_json_route_from_connection(
    connection: &Value,
    enabled: bool,
    expected_executable: &Path,
    client_prefix: &str,
    current_client_id: &str,
) -> ConfiguredJsonMcpRoute {
    let command_matches = connection
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            command.eq_ignore_ascii_case(expected_executable.to_string_lossy().as_ref())
        });
    let arguments = connection
        .get("args")
        .and_then(Value::as_array)
        .map(|args| args.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let bridge_mode = arguments.contains(&"--local-mcp-bridge");
    let names_client = arguments_name_client(&arguments, current_client_id);
    let client_identity = connection
        .get("env")
        .and_then(Value::as_object)
        .and_then(|environment| environment.get(LOCAL_MCP_CLIENT_ENV))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let managed_version = command_matches
        .then(|| client_identity.strip_prefix(client_prefix))
        .flatten()
        .filter(|version| !version.is_empty() && version_numbers(version).is_some())
        .map(str::to_string);
    ConfiguredJsonMcpRoute {
        configured: true,
        launcher_current: command_matches
            && bridge_mode
            && names_client
            && client_identity == current_client_id,
        managed_version,
        enabled,
    }
}

fn configured_json_mcp_route(
    config_path: &Path,
    expected_executable: &Path,
    client_prefix: &str,
    current_client_id: &str,
    label: &str,
    uses_active_field: bool,
) -> Result<ConfiguredJsonMcpRoute, String> {
    if !config_path.is_file() {
        return Ok(unconfigured_json_mcp_route());
    }
    let contents = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read {label}'s MCP configuration: {error}"))?;
    let document = parse_json_mcp_config(&contents, label)?;
    let Some(server) = document
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(CLAUDE_EXTENSION_NAME))
        .filter(|server| server.is_object())
    else {
        return Ok(unconfigured_json_mcp_route());
    };
    let enabled = if uses_active_field {
        server.get("active").and_then(Value::as_bool) == Some(true)
    } else {
        server.get("disabled").and_then(Value::as_bool) != Some(true)
    };
    Ok(configured_json_route_from_connection(
        server,
        enabled,
        expected_executable,
        client_prefix,
        current_client_id,
    ))
}

/// Bionic's `servers` array, with the two shapes that are not one refused.
///
/// A root that is not an object and a `servers` that is not an array are both
/// errors rather than a chance to start again, because starting again means
/// writing over a document whose contents this app cannot account for. A
/// missing `servers` is not an error: a document that has never held a server
/// is the ordinary state of a Bionic that has been started and not configured.
fn bionic_servers<'a>(document: &'a Value, label: &str) -> Result<&'a [Value], String> {
    if !document.is_object() {
        return Err(format!(
            "{label}'s MCP configuration root is not an object."
        ));
    }
    match document.get("servers") {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(servers)) => Ok(servers),
        Some(_) => Err(format!("{label}'s MCP server list is not an array.")),
    }
}

/// Whether one entry of Bionic's array is this app's own.
///
/// **`name` is the identity, and `id` is not.** Bionic mints a UUID per server
/// it creates, so `id` is a value this app cannot know before it has read the
/// file and cannot assume it chose; keying on it would make a pairing
/// unrecognisable the moment Bionic regenerated it. The command is not the
/// identity either - it is an absolute path to this app's executable, which
/// moves with a reinstall or a per-user install, so a route keyed on it would
/// be appended afresh rather than updated. `name` is exactly what the legacy
/// `mcpServers` map keyed this app's entry by, so it is the same identity this
/// provider has always used, expressed in the container Bionic uses.
fn bionic_entry_is_managed(entry: &Value) -> bool {
    entry.get("name").and_then(Value::as_str) == Some(CLAUDE_EXTENSION_NAME)
}

/// The `id` an existing entry carries, which a rewrite keeps.
fn bionic_entry_id(entry: &Value) -> Option<&str> {
    entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|identifier| !identifier.is_empty())
}

/// This app's own entry in Bionic's array, if the document holds one.
fn bionic_managed_entry<'a>(document: &'a Value, label: &str) -> Result<Option<&'a Value>, String> {
    Ok(bionic_servers(document, label)?
        .iter()
        .find(|entry| bionic_entry_is_managed(entry)))
}

fn configured_bionic_route(
    config_path: &Path,
    expected_executable: &Path,
    client_prefix: &str,
    current_client_id: &str,
    label: &str,
) -> Result<ConfiguredJsonMcpRoute, String> {
    if !config_path.is_file() {
        return Ok(unconfigured_json_mcp_route());
    }
    let contents = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read {label}'s MCP configuration: {error}"))?;
    let document = parse_json_mcp_config(&contents, label)?;
    let Some(entry) = bionic_managed_entry(&document, label)? else {
        return Ok(unconfigured_json_mcp_route());
    };
    let connection = entry.get("connection").cloned().unwrap_or(Value::Null);
    let mut route = configured_json_route_from_connection(
        &connection,
        // Every entry this app writes carries `enabled: true`, so for its own
        // entry the flag is always present and a missing one is somebody's
        // edit rather than a default to guess at.
        entry.get("enabled").and_then(Value::as_bool) == Some(true),
        expected_executable,
        client_prefix,
        current_client_id,
    );
    // A route whose transport is not the one the bridge speaks is not a working
    // route, however well the rest of it reads.
    route.launcher_current = route.launcher_current
        && connection.get("type").and_then(Value::as_str) == Some(BIONIC_STDIO_CONNECTION_TYPE);
    Ok(route)
}

/// Write whichever of LM Studio's two documents this installation reads.
fn write_lm_studio_route(
    document: &LmStudioMcpDocument,
    executable_path: &Path,
    client_id: &str,
) -> Result<(), String> {
    if document.bionic {
        write_bionic_mcp_route(
            &document.path,
            executable_path,
            client_id,
            LM_STUDIO_CONFIG_OWNER,
        )
    } else {
        write_json_mcp_route(
            &document.path,
            executable_path,
            client_id,
            LM_STUDIO_CONFIG_OWNER,
            false,
        )
    }
}

/// Read whichever of LM Studio's two documents this installation reads.
fn configured_lm_studio_route(
    document: &LmStudioMcpDocument,
    expected_executable: &Path,
) -> Result<ConfiguredJsonMcpRoute, String> {
    if document.bionic {
        configured_bionic_route(
            &document.path,
            expected_executable,
            LM_STUDIO_CLIENT_PREFIX,
            LM_STUDIO_CLIENT_ID,
            LM_STUDIO_CONFIG_OWNER,
        )
    } else {
        configured_json_mcp_route(
            &document.path,
            expected_executable,
            LM_STUDIO_CLIENT_PREFIX,
            LM_STUDIO_CLIENT_ID,
            LM_STUDIO_CONFIG_OWNER,
            false,
        )
    }
}

fn parse_antigravity_config(contents: &str) -> Result<Value, String> {
    if contents.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(contents)
        .map_err(|error| format!("Antigravity's MCP configuration is invalid: {error}"))
}

fn configured_antigravity_route(
    config_path: &Path,
    expected_executable: &Path,
) -> Result<ConfiguredAntigravityRoute, String> {
    if !config_path.is_file() {
        return Ok(ConfiguredAntigravityRoute {
            configured: false,
            launcher_current: false,
            managed_version: None,
            enabled: true,
        });
    }
    let contents = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read Antigravity's MCP configuration: {error}"))?;
    let document = parse_antigravity_config(&contents)?;
    let server = document
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get(CLAUDE_EXTENSION_NAME))
        .and_then(Value::as_object);
    let Some(server) = server else {
        return Ok(ConfiguredAntigravityRoute {
            configured: false,
            launcher_current: false,
            managed_version: None,
            enabled: true,
        });
    };
    let command = server
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let command_matches =
        command.eq_ignore_ascii_case(expected_executable.to_string_lossy().as_ref());
    let arguments = server
        .get("args")
        .and_then(Value::as_array)
        .map(|args| args.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let bridge_mode = arguments.contains(&"--local-mcp-bridge");
    let names_client = arguments_name_client(&arguments, ANTIGRAVITY_CLIENT_ID);
    let client_identity = server
        .get("env")
        .and_then(Value::as_object)
        .and_then(|environment| environment.get(LOCAL_MCP_CLIENT_ENV))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let command_path = Path::new(command);
    // The command is the same file for every release now, so it cannot say
    // which one wrote the route - only the route's own client identity can.
    // Answering with this build's version instead would report every older
    // pairing as current and never offer Update.
    let managed_version = if command_matches {
        client_identity
            .strip_prefix("antigravity:")
            .filter(|version| !version.is_empty() && version_numbers(version).is_some())
            .map(str::to_string)
    } else {
        command_path
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|name| name.strip_prefix(ANTIGRAVITY_BRIDGE_PREFIX))
            .and_then(|name| name.strip_suffix(".exe"))
            .and_then(|version| {
                let matches_current_directory = expected_executable
                    .parent()
                    .zip(command_path.parent())
                    .is_some_and(|(left, right)| {
                        left.to_string_lossy()
                            .eq_ignore_ascii_case(&right.to_string_lossy())
                    });
                // `expected_executable` is the app executable now, so its
                // parent no longer names an integrations directory. A pairing
                // from 0.1.142 or earlier still points into one of the two
                // this app installed into, and recognising it is what makes
                // that row "Update available" rather than unrecognised.
                let matches_durable_directory = durable_antigravity_integration_directory()
                    .ok()
                    .as_deref()
                    .zip(command_path.parent())
                    .is_some_and(|(left, right)| {
                        left.to_string_lossy()
                            .eq_ignore_ascii_case(&right.to_string_lossy())
                    });
                let matches_canonical_directory = canonical_integration_directory()
                    .ok()
                    .as_deref()
                    .zip(command_path.parent())
                    .is_some_and(|(left, right)| {
                        left.to_string_lossy()
                            .eq_ignore_ascii_case(&right.to_string_lossy())
                    });
                (bridge_mode
                    && (matches_current_directory
                        || matches_durable_directory
                        || matches_canonical_directory)
                    && !version.is_empty()
                    && client_identity == format!("antigravity:{version}"))
                .then(|| version.to_string())
            })
    };
    let executable_exists = expected_executable.is_file();
    Ok(ConfiguredAntigravityRoute {
        configured: true,
        launcher_current: command_matches
            && executable_exists
            && bridge_mode
            && names_client
            && client_identity == ANTIGRAVITY_CLIENT_ID,
        managed_version,
        enabled: server.get("disabled").and_then(Value::as_bool) != Some(true),
    })
}

fn antigravity_permission_configured(settings_path: &Path) -> Result<bool, String> {
    if !settings_path.is_file() {
        return Ok(false);
    }
    let contents = read_client_document_text(settings_path)
        .map_err(|error| format!("Could not read Antigravity's settings: {error}"))?;
    let document = parse_antigravity_config(&contents)?;
    Ok(document
        .get("userSettings")
        .and_then(|value| value.get("globalPermissionGrants"))
        .and_then(|value| value.get("allow"))
        .and_then(Value::as_array)
        .is_some_and(|permissions| {
            permissions
                .iter()
                .any(|permission| permission.as_str() == Some(ANTIGRAVITY_MCP_PERMISSION))
        }))
}

/// One `<file>.qc-remote-backup` per configuration used to be rewritten on every
/// pair, update, and unpair.  Pair backed up the pristine file, the second pair
/// overwrote that backup with the managed generation, and unpair overwrote it a
/// third time - after which nothing QC Remote had written still held the user's
/// original.  Backups are timestamped and the oldest pruned instead, so the
/// original survives every later write and the history stays bounded.
const CONFIG_BACKUP_INFIX: &str = ".qc-remote-backup-";
/// How many backups a configuration keeps: the first one ever taken, plus the
/// most recent `CONFIG_BACKUP_HISTORY - 1`.
const CONFIG_BACKUP_HISTORY: usize = 8;

/// One lock for every read-modify-write of another vendor's configuration.
///
/// Pair, Update and Unpair run on the command pool, and the start-up route
/// refresh runs on its own thread, and each of them reads a vendor's file,
/// changes it and writes it back. Unserialised, a Remove that landed between
/// the refresh's read and its write was undone - the refresh wrote the route
/// straight back - so a removal the owner watched succeed could come back on
/// its own. Every writer and remover takes this lock across its whole read,
/// modify and write, and the refresh takes it across its read, its decision and
/// its rewrite.
static CONFIG_WRITE: Mutex<()> = Mutex::new(());

thread_local! {
    /// Whether this thread already holds [`CONFIG_WRITE`]. The refresh holds it
    /// across a rewrite that goes through the same writers Pair uses, and a
    /// `std` mutex taken twice on one thread deadlocks.
    static CONFIG_WRITE_HELD: Cell<bool> = const { Cell::new(false) };
}

/// Holds [`CONFIG_WRITE`] until dropped, or nothing when an outer guard on the
/// same thread already does.
struct ConfigWriteGuard(Option<MutexGuard<'static, ()>>);

impl Drop for ConfigWriteGuard {
    fn drop(&mut self) {
        if self.0.is_some() {
            CONFIG_WRITE_HELD.with(|held| held.set(false));
        }
    }
}

fn config_write_lock() -> ConfigWriteGuard {
    if CONFIG_WRITE_HELD.with(Cell::get) {
        return ConfigWriteGuard(None);
    }
    // A writer that panicked left the file either as it was or replaced in one
    // rename, never half written, so the lock it poisoned guards nothing broken.
    let guard = CONFIG_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    CONFIG_WRITE_HELD.with(|held| held.set(true));
    ConfigWriteGuard(Some(guard))
}

static CONFIG_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    /// Lets a test name the next staged file in advance, so it can put a
    /// directory in its place and fail the write deterministically.
    static PINNED_CONFIG_STAGE_SEQUENCE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The sibling file a writer stages its replacement document in.
///
/// The process id keeps two QC Remote processes apart, and the per-call
/// sequence number keeps two writers inside one process apart: with the
/// process id alone, one writer's `fs::write` could replace the other's staged
/// bytes, and the second rename then failed with "Could not install" over a
/// write that had in fact landed.
fn staged_config_path(parent: &Path, config_path: &Path, purpose: &str) -> PathBuf {
    let next = || CONFIG_STAGE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    let sequence = PINNED_CONFIG_STAGE_SEQUENCE
        .with(Cell::get)
        .unwrap_or_else(next);
    #[cfg(not(test))]
    let sequence = next();
    parent.join(format!(
        ".{}.{purpose}.{}.{sequence}.tmp",
        config_file_name(config_path),
        std::process::id()
    ))
}

fn config_file_name(config_path: &Path) -> &str {
    config_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("configuration")
}

/// Every timestamped backup of `config_path`, oldest first.
fn config_backups(config_path: &Path) -> Vec<(u128, PathBuf)> {
    let Some(parent) = config_path.parent() else {
        return Vec::new();
    };
    let prefix = format!("{}{CONFIG_BACKUP_INFIX}", config_file_name(config_path));
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut backups = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stamp = name.strip_prefix(&prefix)?.parse::<u128>().ok()?;
            entry
                .file_type()
                .ok()?
                .is_file()
                .then(|| (stamp, entry.path()))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|(stamp, _)| *stamp);
    backups
}

/// Exclusive per-invocation staging; close the handle before failure cleanup.
struct OwnedConfigStage {
    path: PathBuf,
    file: Option<fs::File>,
    owned: bool,
}

impl OwnedConfigStage {
    fn create_with(
        path: PathBuf,
        write: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
    ) -> std::io::Result<Self> {
        let file = qc_owner_only_file::create_new_owner_only(&path)?;
        let mut owned = Self {
            path,
            file: Some(file),
            owned: true,
        };
        let file = owned.file.as_mut().expect("owned staging handle");
        let result = write(file).and_then(|()| file.sync_all());
        owned.file.take();
        result?;
        Ok(owned)
    }

    fn write(path: PathBuf, bytes: &[u8]) -> std::io::Result<Self> {
        Self::create_with(path, |file| file.write_all(bytes))
    }

    fn install(self, destination: &Path) -> std::io::Result<()> {
        self.install_with(destination, || {})
    }

    fn install_with(
        mut self,
        destination: &Path,
        after_transfer: impl FnOnce(),
    ) -> std::io::Result<()> {
        fs::rename(&self.path, destination)?;
        self.owned = false;
        after_transfer();
        Ok(())
    }
}

impl Drop for OwnedConfigStage {
    fn drop(&mut self) {
        self.file.take();
        if !self.owned {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "QC Remote config staging cleanup {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

/// Copies the live configuration aside under a fresh timestamp. The live file
/// is never moved or deleted; only complete bytes enter backup history.
fn back_up_config_file(config_path: &Path, label: &str) -> Result<PathBuf, String> {
    let parent = config_path
        .parent()
        .ok_or_else(|| format!("{label}'s configuration path has no parent directory."))?;
    let prefix = format!("{}{CONFIG_BACKUP_INFIX}", config_file_name(config_path));
    let mut stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The system clock is before the Unix epoch.".to_string())?
        .as_millis();
    let stage = OwnedConfigStage::create_with(
        staged_config_path(parent, config_path, "qc-remote-backup"),
        |destination| {
            let bytes = read_client_document_bytes(config_path)?;
            destination.write_all(&bytes)
        },
    )
    .map_err(|error| format!("Could not back up {label}'s configuration: {error}"))?;
    let backup = loop {
        let candidate = parent.join(format!("{prefix}{stamp}"));
        // Publish only complete bytes, exclusively. A contender cannot be
        // overwritten, and incomplete copies never enter backup history.
        match fs::hard_link(&stage.path, &candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "Could not publish {label}'s configuration backup: {error}"
                ));
            }
        }
        // Two writes inside one millisecond must not share a name, and the
        // numeric suffix is what orders the history.
        stamp += 1;
    };
    drop(stage);
    // The oldest backup is the only copy of the configuration as it stood
    // before QC Remote ever wrote to it, so pruning skips it. Everything after
    // it is a generation QC Remote produced; only the newest few earn a place.
    for (_, stale) in config_backups(config_path)
        .iter()
        .skip(1)
        .rev()
        .skip(CONFIG_BACKUP_HISTORY - 1)
    {
        let _ = fs::remove_file(stale);
    }
    Ok(backup)
}

fn write_json_document(config_path: &Path, document: &Value, label: &str) -> Result<(), String> {
    let parent = config_path
        .parent()
        .ok_or_else(|| format!("{label} settings path has no parent directory."))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {label}'s settings directory: {error}"))?;
    let staged = staged_config_path(parent, config_path, "qc-remote");
    let serialized = serde_json::to_vec_pretty(document)
        .map_err(|error| format!("Could not serialize {label}'s settings: {error}"))?;
    let staged = OwnedConfigStage::write(staged, &serialized)
        .map_err(|error| format!("Could not stage {label}'s settings: {error}"))?;
    if config_path.is_file() {
        back_up_config_file(config_path, label)?;
    }
    // Renaming over the live file replaces it in one step, so a crash can only
    // leave the old contents or the new ones, never a missing file.
    if let Err(error) = staged.install(config_path) {
        return Err(format!("Could not install {label}'s settings: {error}"));
    }
    Ok(())
}

fn write_json_mcp_route(
    config_path: &Path,
    executable_path: &Path,
    client_id: &str,
    label: &str,
    uses_active_field: bool,
) -> Result<(), String> {
    let _config_write = config_write_lock();
    let mut document = if config_path.is_file() {
        let contents = read_client_document_text(config_path)
            .map_err(|error| format!("Could not read {label}'s MCP configuration: {error}"))?;
        parse_json_mcp_config(&contents, label)?
    } else {
        json!({})
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| format!("{label}'s MCP configuration root is not an object."))?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{label}'s MCP server settings are not an object."))?;
    servers.insert(
        CLAUDE_EXTENSION_NAME.to_string(),
        json_mcp_route_server(executable_path, client_id, uses_active_field),
    );
    write_json_document(config_path, &document, label)
}

/// The managed LM Studio / Jan server entry, exactly as this app writes it.
/// Shared with the start-up refresh for the reason [`codex_route_table`] is.
fn json_mcp_route_server(
    executable_path: &Path,
    client_id: &str,
    uses_active_field: bool,
) -> Value {
    // Both identity channels, for the reason the Antigravity and ChatGPT
    // routes carry both: a host that keeps `env` in its own server state
    // without passing it to the native STDIO child leaves the bridge
    // announcing itself anonymously, and this app then has nothing to credit
    // the heartbeat to. The argument reaches the child regardless.
    let mut server = json!({
        "command": executable_path.to_string_lossy(),
        "args": ["--local-mcp-bridge", CLIENT_ID_ARGUMENT, client_id],
        "env": { "QC_REMOTE_LOCAL_MCP_CLIENT": client_id }
    });
    if uses_active_field {
        server["active"] = Value::Bool(true);
    } else {
        server["disabled"] = Value::Bool(false);
    }
    server
}

/// The managed Bionic server entry, exactly as this app writes it.
///
/// The shape is the one Bionic itself produces for a server added through its
/// own UI: `id`, `name`, `enabled`, and a nested `connection` carrying `type`,
/// `command`, `args` and `env`.
///
/// Both identity channels are filled, for the reason every other provider's
/// route fills both: a host that keeps `env` in its own server state without
/// passing it to the native STDIO child leaves the bridge announcing itself
/// anonymously, and this app then has nothing to credit the heartbeat to. The
/// argument reaches the child regardless.
///
/// `enabled: true` is this app's to set here, and setting it is the one place
/// this document is *better* than the one it replaces: the pre-rename model
/// required the owner to find a switch in the host's own UI after pairing, and
/// pairing can now be complete with nothing left for him to do.
fn bionic_route_server(executable_path: &Path, client_id: &str, identifier: Option<&str>) -> Value {
    json!({
        "id": identifier.unwrap_or(BIONIC_MANAGED_SERVER_ID),
        "name": CLAUDE_EXTENSION_NAME,
        "enabled": true,
        "connection": {
            "type": BIONIC_STDIO_CONNECTION_TYPE,
            "command": executable_path.to_string_lossy(),
            "args": ["--local-mcp-bridge", CLIENT_ID_ARGUMENT, client_id],
            "env": { LOCAL_MCP_CLIENT_ENV: client_id }
        }
    })
}

/// Put this app's entry into Bionic's document, and change nothing else.
///
/// The root is an **array**, and the difference that matters is that an array
/// has no key to overwrite. So this merges: every entry that is not this app's
/// is left exactly as it was, in the position it was in, with every key it
/// carries - including keys this app does not understand, which it never reads
/// and therefore cannot judge. A user may have a hand-authored server entry
/// of their own, and losing it while pairing would be far worse than not
/// pairing at all.
///
/// This app's own entry is replaced rather than merged into, which is what the
/// `mcpServers` writer has always done, with one exception: an existing `id` is
/// carried forward. Bionic minted it and may key its own per-server state on
/// it, and this app has no reason to prefer its own default to a value the host
/// chose.
///
/// A document that already holds more than one entry under this app's name -
/// which no version of this writer produces, and which the duplicate guard
/// exists to prevent - is collapsed to the first of them. They are all this
/// app's by its own identity rule, so removing the extras loses nothing that
/// was not a duplicate.
fn write_bionic_mcp_route(
    config_path: &Path,
    executable_path: &Path,
    client_id: &str,
    label: &str,
) -> Result<(), String> {
    let _config_write = config_write_lock();
    let mut document = if config_path.is_file() {
        let contents = read_client_document_text(config_path)
            .map_err(|error| format!("Could not read {label}'s MCP configuration: {error}"))?;
        parse_json_mcp_config(&contents, label)?
    } else {
        json!({})
    };
    // Refused before anything is staged, so a document this app cannot account
    // for is left byte for byte as it was.
    bionic_servers(&document, label)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| format!("{label}'s MCP configuration root is not an object."))?;
    let servers = root
        .entry("servers")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("{label}'s MCP server list is not an array."))?;
    let managed = servers
        .iter()
        .enumerate()
        .filter(|(_, entry)| bionic_entry_is_managed(entry))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let identifier = managed
        .first()
        .and_then(|index| bionic_entry_id(&servers[*index]))
        .map(str::to_string);
    let entry = bionic_route_server(executable_path, client_id, identifier.as_deref());
    match managed.first() {
        Some(&first) => {
            servers[first] = entry;
            // Back to front, so each removal cannot move an index still to be
            // removed.
            for &duplicate in managed.iter().skip(1).rev() {
                servers.remove(duplicate);
            }
        }
        None => servers.push(entry),
    }
    write_json_document(config_path, &document, label)
}

/// Take this app's entry out of Bionic's document, and change nothing else.
///
/// An emptied `servers` array is left in place, for the reason
/// [`remove_json_mcp_route`] leaves an emptied `mcpServers` object: this app
/// cannot tell a container it created from one the user already had, and an
/// inert empty array is a far smaller imposition than deleting a key somebody
/// else wrote.
fn remove_bionic_mcp_route(config_path: &Path, label: &str) -> Result<bool, String> {
    let _config_write = config_write_lock();
    if !config_path.is_file() {
        return Ok(false);
    }
    let contents = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read {label}'s MCP configuration: {error}"))?;
    let mut document = parse_json_mcp_config(&contents, label)?;
    bionic_servers(&document, label)?;
    let Some(servers) = document.get_mut("servers").and_then(Value::as_array_mut) else {
        return Ok(false);
    };
    let before = servers.len();
    servers.retain(|entry| !bionic_entry_is_managed(entry));
    let removed = servers.len() != before;
    if removed {
        write_json_document(config_path, &document, label)?;
    }
    Ok(removed)
}

fn remove_json_mcp_route(config_path: &Path, label: &str) -> Result<bool, String> {
    let _config_write = config_write_lock();
    if !config_path.is_file() {
        return Ok(false);
    }
    let contents = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read {label}'s MCP configuration: {error}"))?;
    let mut document = parse_json_mcp_config(&contents, label)?;
    let removed = document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove(CLAUDE_EXTENSION_NAME))
        .is_some();
    // An emptied `mcpServers` is left in place. QC Remote cannot tell an object
    // it created from one the user already had, and an inert empty object is a
    // far smaller imposition than deleting a key they wrote themselves.
    if removed {
        write_json_document(config_path, &document, label)?;
    }
    Ok(removed)
}

/// The managed Antigravity server entry, exactly as this app writes it.
/// Shared with the start-up refresh for the reason [`codex_route_table`] is.
fn antigravity_route_server(executable_path: &Path, client_id: &str) -> Value {
    json!({
        "command": executable_path.to_string_lossy(),
        // Antigravity keeps the configured environment in its own
        // server state but does not pass it to a native STDIO child, so a
        // route identified only by `env` produced a working MCP server
        // whose heartbeat was anonymous - and a LOCAL row stuck on "Needs
        // attention". The argument carries the same identity through a
        // channel the host cannot drop, and `env` stays for the hosts that
        // do propagate it.
        "args": ["--local-mcp-bridge", CLIENT_ID_ARGUMENT, client_id],
        "env": {
            "QC_REMOTE_LOCAL_MCP_CLIENT": client_id
        }
    })
}

fn write_antigravity_config(
    config_path: &Path,
    executable_path: &Path,
    client_id: &str,
) -> Result<(), String> {
    let _config_write = config_write_lock();
    let mut document = if config_path.is_file() {
        let contents = read_client_document_text(config_path)
            .map_err(|error| format!("Could not read Antigravity's MCP configuration: {error}"))?;
        parse_antigravity_config(&contents)?
    } else {
        json!({})
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| "Antigravity's MCP configuration root is not an object.".to_string())?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "Antigravity's MCP server settings are not an object.".to_string())?;
    servers.insert(
        CLAUDE_EXTENSION_NAME.to_string(),
        antigravity_route_server(executable_path, client_id),
    );
    write_json_document(config_path, &document, "Antigravity")
}

fn cleanup_stale_antigravity_bridges(current: Option<&Path>) {
    for directory in [durable_antigravity_integration_directory().ok()]
        .into_iter()
        .flatten()
    {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let managed = path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.starts_with(ANTIGRAVITY_BRIDGE_PREFIX)
                        && name.to_ascii_lowercase().ends_with(".exe")
                });
            if managed && current != Some(path.as_path()) {
                // A previous version may still be serving Antigravity. Windows
                // keeps that image locked; leaving it until the next pairing is
                // safer than interrupting a live MCP conversation.
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn legacy_gemini_route_present(settings_path: &Path) -> Result<bool, String> {
    if !settings_path.is_file() {
        return Ok(false);
    }
    let contents = read_client_document_text(settings_path)
        .map_err(|error| format!("Could not read legacy Gemini settings: {error}"))?;
    let document = parse_antigravity_config(&contents)?;
    Ok(document
        .get("mcpServers")
        .and_then(Value::as_object)
        .is_some_and(|servers| servers.contains_key(CLAUDE_EXTENSION_NAME)))
}

fn remove_legacy_gemini_route(settings_path: &Path) -> Result<bool, String> {
    let _config_write = config_write_lock();
    if !settings_path.is_file() {
        return Ok(false);
    }
    let contents = read_client_document_text(settings_path)
        .map_err(|error| format!("Could not read legacy Gemini settings: {error}"))?;
    let mut document = parse_antigravity_config(&contents)?;
    let mut removed = false;
    if let Some(servers) = document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
    {
        removed = servers.remove(CLAUDE_EXTENSION_NAME).is_some();
    }
    if removed {
        write_json_document(settings_path, &document, "legacy Gemini")?;
    }
    Ok(removed)
}

fn write_antigravity_permission(settings_path: &Path) -> Result<(), String> {
    let _config_write = config_write_lock();
    let mut document = if settings_path.is_file() {
        let contents = read_client_document_text(settings_path)
            .map_err(|error| format!("Could not read Antigravity's settings: {error}"))?;
        parse_antigravity_config(&contents)?
    } else {
        json!({})
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| "Antigravity's settings root is not an object.".to_string())?;
    let user_settings = root
        .entry("userSettings")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "Antigravity's user settings are not an object.".to_string())?;
    let permission_grants = user_settings
        .entry("globalPermissionGrants")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "Antigravity's permission grants are not an object.".to_string())?;
    let allow = permission_grants
        .entry("allow")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "Antigravity's permission allowlist is not an array.".to_string())?;
    if !allow
        .iter()
        .any(|permission| permission.as_str() == Some(ANTIGRAVITY_MCP_PERMISSION))
    {
        allow.push(Value::String(ANTIGRAVITY_MCP_PERMISSION.to_string()));
        write_json_document(settings_path, &document, "Antigravity")?;
    }
    Ok(())
}

fn remove_antigravity_route(config_path: &Path) -> Result<bool, String> {
    let _config_write = config_write_lock();
    if config_path.is_file() {
        let contents = read_client_document_text(config_path)
            .map_err(|error| format!("Could not read Antigravity's MCP configuration: {error}"))?;
        let mut document = parse_antigravity_config(&contents)?;
        let mut removed = false;
        if let Some(servers) = document
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
        {
            removed = servers.remove(CLAUDE_EXTENSION_NAME).is_some();
        }
        if removed {
            write_json_document(config_path, &document, "Antigravity")?;
        }
        return Ok(removed);
    }
    Ok(false)
}

fn remove_antigravity_permission(settings_path: &Path) -> Result<bool, String> {
    let _config_write = config_write_lock();
    if !settings_path.is_file() {
        return Ok(false);
    }
    let contents = read_client_document_text(settings_path)
        .map_err(|error| format!("Could not read Antigravity's settings: {error}"))?;
    let mut document = parse_antigravity_config(&contents)?;
    let Some(allow) = document
        .get_mut("userSettings")
        .and_then(|value| value.get_mut("globalPermissionGrants"))
        .and_then(|value| value.get_mut("allow"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(false);
    };
    let original_length = allow.len();
    allow.retain(|permission| permission.as_str() != Some(ANTIGRAVITY_MCP_PERMISSION));
    let removed = allow.len() != original_length;
    if removed {
        write_json_document(settings_path, &document, "Antigravity")?;
    }
    Ok(removed)
}

#[allow(clippy::too_many_arguments)]
fn chatgpt_status_document(
    chatgpt_installed: bool,
    configured: bool,
    launcher_current: bool,
    approval_mode_current: bool,
    installed_version: Option<String>,
    status_error: Option<String>,
    restart_required: bool,
    evidence: LocalClientEvidence,
) -> Value {
    let available_version = env!("CARGO_PKG_VERSION");
    let state = if !chatgpt_installed {
        "chatgpt-not-installed"
    } else if status_error.is_some() {
        "status-error"
    } else if !configured {
        "not-installed"
    } else if !launcher_current {
        // A route QC Remote itself wrote is an update, not a foreign
        // configuration. Its version comes from the route's own client
        // identity, never from the separately installed node bridge, which
        // this route does not run - and a version equal to this build's is no
        // longer evidence that the route is current, because a pairing made
        // before the identity argument existed names this exact version in
        // `env` and nothing on its command line. Reporting that as
        // "different-configuration" would tell the user a configuration this
        // app wrote is somebody else's; it is theirs, and Update rewrites it.
        // Only an entry this app cannot recognise at all has no version here.
        match installed_version.as_deref() {
            Some(installed) => match compare_versions(available_version, installed) {
                Some(Ordering::Less) => "newer-installed",
                _ => "update-available",
            },
            None => "different-configuration",
        }
    } else if !approval_mode_current {
        "update-available"
    } else if let Some(installed) = installed_version.as_deref() {
        match compare_versions(available_version, installed) {
            Some(Ordering::Greater) => "update-available",
            Some(Ordering::Less) => "newer-installed",
            _ if installed == available_version => "current",
            _ => "update-available",
        }
    } else {
        "update-available"
    };
    json!({
        "chatgptInstalled": chatgpt_installed,
        "integrationInstalled": configured,
        "installedVersion": installed_version,
        "availableVersion": available_version,
        "state": state,
        "statusError": status_error,
        "restartRequired": restart_required,
        "connectionActive": evidence.connected(),
        "bridgeAttached": evidence.attached(),
        "connectionPending": evidence.settling(),
        "approvalModeCurrent": approval_mode_current,
    })
}

fn chatgpt_restart_required(config_path: &Path) -> bool {
    let Some(configured_at) = fs::metadata(config_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
    else {
        return false;
    };
    chatgpt_restart_required_for_processes(configured_at, chatgpt_process_start_times())
}

/// ChatGPT Desktop and Codex share the managed MCP entry.  A reload in either
/// host is sufficient to load the current bridge; using the oldest process
/// kept the UI stuck on a restart prompt whenever the other host remained
/// open.  Compare the newest host instead, so the status reflects the reload
/// the user actually completed.  The prompt itself is gone - the row now says
/// ChatGPT starts the rewritten connection in the next chat, which is what
/// Codex actually does - but this comparison still decides that wording.
fn chatgpt_restart_required_for_processes(
    configured_at: u128,
    process_start_times: impl IntoIterator<Item = u128>,
) -> bool {
    process_start_times
        .into_iter()
        .max()
        .is_some_and(|started_at| started_at < configured_at)
}

/// Whether to ask a host to reload its MCP servers.
///
/// The timestamp comparison above is a guess, and it was the last word. It
/// asks whether the host's process is older than the configuration file, which
/// answers the wrong question twice over: the ChatGPT MCP reload control does
/// not restart ChatGPT.exe, so a host that has reloaded still looks old, and
/// an app restart wiped the bridge record that used to override it - so six
/// bridges running the current route read as never-attached and the row
/// demanded a restart that would have changed nothing.
///
/// Evidence decides it now, and the timestamp is only the fallback for when
/// there is none:
///
/// - a bridge that has announced this app's own client id has demonstrably
///   read the configuration this app wrote, so there is nothing to reload;
/// - inside the start-up window nothing is known yet, and a host is not asked
///   to restart on the strength of a heartbeat that has not had time to
///   arrive;
/// - only a host that has stayed silent past that window falls back to the
///   process-age guess.
fn host_restart_required(process_restart_required: bool, evidence: LocalClientEvidence) -> bool {
    if evidence.picked_up_current_config() || evidence.settling() {
        return false;
    }
    process_restart_required
}

/// The managed ChatGPT/Codex server entry, exactly as this app writes it.
///
/// `write_codex_config` installs it, and the startup repair uses it, through
/// the rebuilder [`classify_route_refresh`] is given, as the desired
/// configuration for an entry carrying this app's managed identity. The
/// startup repair preserves all other host entries.
fn codex_route_table(executable_path: &Path, client_id: &str) -> Table {
    let mut server = Table::new();
    server.insert("command", value(executable_path.to_string_lossy().as_ref()));
    let mut args = Array::new();
    args.push("--local-mcp-bridge");
    // The identity is carried twice on purpose, and the argument is the copy
    // that survives. A host may keep the configured `env` in its own MCP
    // server state without handing it to the native STDIO child it spawns: a
    // bridge started that way runs as `qc-remote.exe --local-mcp-bridge` with
    // no identity on its command line at all, so its heartbeat is anonymous,
    // nothing is ever recorded under `codex:<version>`, and the row stays
    // amber while the tools themselves work. An argument cannot be dropped
    // that way.
    args.push(CLIENT_ID_ARGUMENT);
    args.push(client_id);
    server.insert("args", value(args));
    // This is the documented Codex/ChatGPT per-server equivalent of Always
    // allow. QC Remote remains the policy boundary through its own selected
    // Read-only, Performance, Modify, or Full access tier.
    server.insert("default_tools_approval_mode", value("approve"));
    server.insert("startup_timeout_sec", value(15));
    server.insert("tool_timeout_sec", value(300));
    // Kept as the fallback channel for the hosts that do propagate it, and as
    // the field every route reader has always keyed the paired version on.
    let mut environment = Table::new();
    environment.insert(LOCAL_MCP_CLIENT_ENV, value(client_id));
    server.insert("env", Item::Table(environment));
    server
}

#[cfg(any(windows, target_os = "macos"))]
fn codex_http_route_table(
    executable_path: &Path,
    client_id: &str,
    port: crate::local_http_binding::StablePort,
) -> Result<Table, String> {
    let helper = codex_http_helper_command(executable_path)?;
    let mut server = Table::new();
    server.insert("url", value(port.url()));
    server.insert("http_headers_helper", value(helper));
    server.insert("default_tools_approval_mode", value("approve"));
    server.insert("startup_timeout_sec", value(15));
    server.insert("tool_timeout_sec", value(300));
    let mut headers = Table::new();
    headers.insert(LOCAL_MCP_CLIENT_HEADER, value(client_id));
    server.insert("http_headers", Item::Table(headers));
    Ok(server)
}

#[cfg(windows)]
fn codex_http_helper_command(executable_path: &Path) -> Result<String, String> {
    use base64::Engine as _;
    let executable = executable_path
        .to_str()
        .filter(|_| executable_path.is_absolute())
        .ok_or("The HTTP credential helper must be an absolute Unicode executable path")?;
    // Codex runs this STRING through cmd.exe. Never put a profile path into
    // that command language: % expansion happens even inside double quotes.
    // Encode a data-decoding PowerShell command invoking the absolute native
    // helper. No path literal, profile, PATH lookup, token or resident proxy.
    let path = base64::engine::general_purpose::STANDARD.encode(
        executable
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    // Smart quotes are PowerShell delimiters too. Decode the path as DATA;
    // no path characters are ever interpolated into command syntax.
    let script = format!("$ErrorActionPreference='Stop'; $p=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{path}')); & $p --local-mcp-http-headers; exit $LASTEXITCODE");
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let shell = crate::system_tools::powershell_exe();
    let shell = shell
        .to_str()
        .filter(|path| !path.contains(['%', '"', '\r', '\n']))
        .ok_or("The Windows system PowerShell path cannot be represented safely")?;
    let helper =
        format!("\"{shell}\" -NoLogo -NoProfile -NonInteractive -EncodedCommand {encoded}");
    Ok(helper)
}

#[cfg(target_os = "macos")]
fn codex_http_helper_command(executable_path: &Path) -> Result<String, String> {
    let executable = executable_path
        .to_str()
        .filter(|_| executable_path.is_absolute())
        .filter(|path| !path.contains(['\n', '\r', '\0']))
        .ok_or("The HTTP credential helper must be an absolute Unicode executable path")?;
    // Codex executes the helper through the platform shell. Single-quote the
    // path as data; apostrophes are escaped by closing/reopening the quote.
    Ok(format!(
        "'{}' --local-mcp-http-headers",
        executable.replace('\'', "'\"'\"'")
    ))
}

fn write_codex_config(
    config_path: &Path,
    executable_path: &Path,
    client_id: &str,
) -> Result<(), String> {
    write_codex_config_with_route(config_path, codex_route_table(executable_path, client_id))
}

const CODEX_TRANSPORT_FIELDS: &[&str] = &[
    "command",
    "args",
    "env",
    "cwd",
    "url",
    "http_headers_helper",
    "http_headers",
    "env_http_headers",
    "bearer_token_env_var",
    "startup_timeout_sec",
    "tool_timeout_sec",
];

fn write_codex_config_with_route(config_path: &Path, mut route: Table) -> Result<(), String> {
    let _config_write = config_write_lock();
    let existing = if config_path.is_file() {
        read_client_document_text(config_path)
            .map_err(|error| format!("Could not read ChatGPT's MCP configuration: {error}"))?
    } else {
        String::new()
    };
    let mut document = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing
            .parse::<DocumentMut>()
            .map_err(|error| format!("ChatGPT's MCP configuration is invalid: {error}"))?
    };
    if document.get("mcp_servers").is_none() {
        document["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = document["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| "ChatGPT's MCP server configuration is not a table.".to_string())?;
    if let Some(present_item) = servers.get(CLAUDE_EXTENSION_NAME) {
        let present = present_item
            .as_table_like()
            .ok_or("The existing QC Remote entry is not a table; no configuration was changed")?;
        if route.contains_key("http_headers_helper") {
            let identity =
                route_client_identity(&toml_item_as_json(present_item)).map(str::to_owned);
            if !identity
                .as_deref()
                .and_then(|id| id.strip_prefix("codex:"))
                .is_some_and(|version| !version.is_empty() && version_numbers(version).is_some())
                || ["env_http_headers", "bearer_token_env_var"]
                    .iter()
                    .any(|key| present.contains_key(key))
                || present.get("http_headers").is_some_and(|headers| {
                    !headers.as_table_like().is_some_and(|headers| {
                        headers.len() == 1
                            && headers.get(LOCAL_MCP_CLIENT_HEADER).and_then(Item::as_str)
                                == identity.as_deref()
                    })
                })
            {
                return Err("The existing QC Remote entry has custom ownership or authentication. Review it explicitly before pairing shared HTTP; no configuration was changed.".into());
            }
        }
        // Replace transport ONLY. Preserve every policy/unknown non-transport
        // field, including enabled, per-tool restrictions and approval modes.
        // A future client policy must not silently become permissive here.
        for (key, item) in present
            .iter()
            .filter(|(key, _)| !CODEX_TRANSPORT_FIELDS.contains(key))
        {
            route.insert(key, item.clone());
        }
    }
    servers.insert(CLAUDE_EXTENSION_NAME, Item::Table(route));

    let parent = config_path
        .parent()
        .ok_or_else(|| "ChatGPT's MCP configuration path has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create ChatGPT's configuration directory: {error}"))?;
    let staged = staged_config_path(parent, config_path, "qc-remote");
    let staged = OwnedConfigStage::write(staged, document.to_string().as_bytes())
        .map_err(|error| format!("Could not stage ChatGPT's MCP configuration: {error}"))?;
    if config_path.is_file() {
        back_up_config_file(config_path, "ChatGPT")?;
    }
    if let Err(error) = staged.install(config_path) {
        return Err(format!(
            "Could not install ChatGPT's MCP configuration: {error}"
        ));
    }
    Ok(())
}

fn remove_codex_route(config_path: &Path) -> Result<bool, String> {
    let _config_write = config_write_lock();
    if !config_path.is_file() {
        return Ok(false);
    }
    let existing = read_client_document_text(config_path)
        .map_err(|error| format!("Could not read ChatGPT's MCP configuration: {error}"))?;
    let mut document = existing
        .parse::<DocumentMut>()
        .map_err(|error| format!("ChatGPT's MCP configuration is invalid: {error}"))?;
    let removed = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_mut)
        .is_some_and(|servers| servers.remove(CLAUDE_EXTENSION_NAME).is_some());
    if !removed {
        return Ok(false);
    }
    // An emptied `[mcp_servers]` stays, for the same reason the JSON writers
    // leave an emptied `mcpServers`: QC Remote cannot tell a table it created
    // from one the user wrote, so it never deletes the table itself.

    let parent = config_path
        .parent()
        .ok_or_else(|| "ChatGPT's MCP configuration path has no parent directory.".to_string())?;
    // The staging name is unique per call like its sibling writer's: two QC
    // Remote processes unpairing at once used to stage over each other.
    let staged = staged_config_path(parent, config_path, "qc-remote-remove");
    let staged = OwnedConfigStage::write(staged, document.to_string().as_bytes())
        .map_err(|error| format!("Could not stage ChatGPT's MCP configuration: {error}"))?;
    back_up_config_file(config_path, "ChatGPT")?;
    // No delete-then-rename: the live configuration is replaced in one step, so
    // a crash mid-unpair can never leave the user without a config.toml.
    if let Err(error) = staged.install(config_path) {
        return Err(format!(
            "Could not remove ChatGPT's QC Remote configuration: {error}"
        ));
    }
    Ok(true)
}

fn chatgpt_is_running() -> bool {
    !chatgpt_process_start_times().is_empty()
}

fn claude_user_data_roots_in(
    roaming_app_data: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(roaming_app_data) = roaming_app_data {
        roots.push(roaming_app_data.join("Claude"));
    }

    // The Microsoft Store build is an MSIX package. Its Electron userData
    // directory is virtualized beneath the package family rather than exposed
    // through this (unpackaged) process's APPDATA value.
    if let Some(local_app_data) = local_app_data {
        let packages = local_app_data.join("Packages");
        if let Ok(entries) = fs::read_dir(packages) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let name = name.to_ascii_lowercase();
                if !name.starts_with("claude_") && !name.starts_with("anthropic.claude_") {
                    continue;
                }
                let package = entry.path();
                for root in [
                    package.join("LocalCache").join("Roaming"),
                    package.join("RoamingState"),
                ] {
                    roots.push(root.join("Claude"));
                }
            }
        }
    }
    roots
}

fn claude_extension_registry_paths_in(user_data_roots: &[PathBuf]) -> Vec<PathBuf> {
    user_data_roots
        .iter()
        .map(|root| root.join("extensions-installations.json"))
        .collect()
}

fn claude_extension_manifest_paths_in(user_data_roots: &[PathBuf]) -> Vec<PathBuf> {
    user_data_roots
        .iter()
        .flat_map(|root| {
            fs::read_dir(root.join("Claude Extensions"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path().join("manifest.json"))
                .filter(|path| path.is_file())
        })
        .collect()
}

fn claude_user_data_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return user_home_directory()
            .map(|home| {
                vec![home
                    .join("Library")
                    .join("Application Support")
                    .join("Claude")]
            })
            .unwrap_or_default();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let roaming_app_data = std::env::var_os("APPDATA").map(PathBuf::from);
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        claude_user_data_roots_in(roaming_app_data.as_deref(), local_app_data.as_deref())
    }
}

fn installed_claude_extension_version(path: &Path) -> Result<Option<String>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let contents = read_client_document_bytes(path)
        .map_err(|error| format!("Could not read Claude Desktop's extension registry: {error}"))?;
    let document: Value = serde_json::from_slice(&contents)
        .map_err(|error| format!("Claude Desktop's extension registry is invalid: {error}"))?;
    let Some(extensions) = document.get("extensions").and_then(Value::as_object) else {
        return Err("Claude Desktop's extension registry has no extensions list.".into());
    };
    let Some(entry) = extensions.values().find(|entry| {
        entry
            .get("manifest")
            .and_then(|manifest| manifest.get("name"))
            .and_then(Value::as_str)
            == Some(CLAUDE_EXTENSION_NAME)
    }) else {
        return Ok(None);
    };
    let manifest = entry
        .get("manifest")
        .ok_or_else(|| "The installed QC Remote extension has no manifest.".to_string())?;
    entry
        .get("version")
        .and_then(Value::as_str)
        .or_else(|| manifest.get("version").and_then(Value::as_str))
        .map(str::to_owned)
        .map(Some)
        .ok_or_else(|| "The installed QC Remote extension has no version.".to_string())
}

fn installed_claude_extension_manifest_version(path: &Path) -> Result<Option<String>, String> {
    let contents = read_client_document_bytes(path)
        .map_err(|error| format!("Could not read Claude Desktop's extension manifest: {error}"))?;
    let manifest: Value = serde_json::from_slice(&contents)
        .map_err(|error| format!("Claude Desktop's extension manifest is invalid: {error}"))?;
    if manifest.get("name").and_then(Value::as_str) != Some(CLAUDE_EXTENSION_NAME) {
        return Ok(None);
    }
    manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .map(Some)
        .ok_or_else(|| "The installed QC Remote extension has no version.".to_string())
}

fn installed_claude_extension_version_from_paths(
    registry_paths: &[PathBuf],
    manifest_paths: &[PathBuf],
) -> Result<Option<String>, String> {
    let mut installed = Vec::new();
    let mut errors = Vec::new();
    for path in registry_paths {
        match installed_claude_extension_version(path) {
            Ok(Some(version)) => installed.push(version),
            Ok(None) => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    for path in manifest_paths {
        match installed_claude_extension_manifest_version(path) {
            Ok(Some(version)) => installed.push(version),
            Ok(None) => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    installed
        .sort_by(|left, right| compare_versions(left, right).unwrap_or_else(|| left.cmp(right)));
    if let Some(version) = installed.pop() {
        return Ok(Some(version));
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(errors.join("; "))
    }
}

fn version_numbers(version: &str) -> Option<Vec<u64>> {
    let core = version.split(['-', '+']).next()?;
    let parsed = core
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!parsed.is_empty()).then_some(parsed)
}

fn compare_versions(left: &str, right: &str) -> Option<Ordering> {
    let left = version_numbers(left)?;
    let right = version_numbers(right)?;
    let length = left.len().max(right.len());
    Some(
        (0..length)
            .map(|index| *left.get(index).unwrap_or(&0))
            .cmp((0..length).map(|index| *right.get(index).unwrap_or(&0))),
    )
}

/// The Claude payload used to carry no liveness at all, so the panel rendered
/// "Connected" whenever a file of the right version sat on disk.  These two
/// fields have the same meaning as every managed provider's: only a route that
/// is installed and current can report a live client, and `bridgeAttached`
/// stays true once a bridge has ever handshaked in this app session.
fn claude_status_document(
    claude_installed: bool,
    installed_version: Option<String>,
    evidence: LocalClientEvidence,
) -> Value {
    let available_version = env!("CARGO_PKG_VERSION");
    let state = if !claude_installed {
        "claude-not-installed"
    } else if let Some(installed) = installed_version.as_deref() {
        match compare_versions(available_version, installed) {
            Some(Ordering::Greater) => "update-available",
            Some(Ordering::Less) => "newer-installed",
            _ if installed == available_version => "current",
            _ => "different-version",
        }
    } else {
        "not-installed"
    };
    let extension_current = matches!(state, "current" | "newer-installed");
    json!({
        "claudeInstalled": claude_installed,
        "extensionInstalled": installed_version.is_some(),
        "installedVersion": installed_version,
        "availableVersion": available_version,
        "state": state,
        "connectionActive": extension_current && evidence.connected(),
        "bridgeAttached": extension_current && evidence.attached(),
        "connectionPending": extension_current && evidence.settling(),
    })
}

fn claude_desktop_extension_status_blocking(
    evidence: LocalClientEvidence,
) -> Result<Value, String> {
    let desktop = claude_desktop_installation();
    let user_data_roots = claude_user_data_roots();
    let registry_paths = claude_extension_registry_paths_in(&user_data_roots);
    let manifest_paths = claude_extension_manifest_paths_in(&user_data_roots);
    let (installed_version, status_error) =
        match installed_claude_extension_version_from_paths(&registry_paths, &manifest_paths) {
            Ok(version) => (version, None),
            Err(error) => (None, Some(error)),
        };
    let mut status = claude_status_document(
        desktop.is_some() || claude_uri_handler_registered(),
        installed_version,
        evidence,
    );
    if let Some(version) = desktop.and_then(|installation| installation.version) {
        status["appVersion"] = Value::String(version);
    }
    if let Some(error) = status_error {
        status["statusError"] = Value::String(error);
    }
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    annotate_folder_access(&mut status, "claude");
    Ok(status)
}

#[tauri::command]
pub async fn claude_desktop_extension_status() -> Result<Value, String> {
    let evidence = local_client_evidence(CLAUDE_CLIENT_ID);
    inspect_client_until(Instant::now() + CLIENT_INSPECTION_TIMEOUT, move || {
        claude_desktop_extension_status_blocking(evidence)
    })
    .await
}

fn install_claude_desktop_extension_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    {
        let home = user_home_directory().ok_or("The user home directory is unavailable")?;
        qc_native_platform::app_store::authorize_folder(
            &home.join("Library/Application Support/Claude"),
        )?;
    }
    let claude = claude_desktop_installation()
        .and_then(|installation| installation.executable)
        .or_else(claude_msix_path);
    if claude.is_none() && !claude_uri_handler_registered() {
        return Err("Claude Desktop is not installed for this account.".to_string());
    }
    let integration_directory = canonical_integration_directory()?;
    fs::create_dir_all(&integration_directory)
        .map_err(|error| format!("Could not prepare the Claude integration: {error}"))?;
    let package_path = integration_directory.join(CLAUDE_EXTENSION_FILE);
    fs::write(&package_path, claude_desktop_extension()?)
        .map_err(|error| format!("Could not prepare the Claude extension: {error}"))?;

    // Claude Desktop owns extension validation, confirmation, and replacement.
    // Passing the MCPB path invokes that supported install/update flow.
    if let Some(claude) = claude {
        spawn_desktop_command(desktop_document_command(&claude, &package_path))?;
    } else {
        #[cfg(windows)]
        {
            // Keep the shell fallback for future Claude packages that register
            // the protocol but expose neither a classic nor discoverable MSIX
            // binary.
            let mut command = Command::new(crate::system_tools::explorer_exe());
            command.arg(&package_path);
            spawn_desktop_command(command)?;
        }
        #[cfg(not(windows))]
        {
            return Err(
                "Claude Desktop extension installation is unavailable on this platform. Use the generic MCP configuration instead."
                    .to_string(),
            );
        }
    }

    Ok(json!({
        "launched": true,
        "availableVersion": env!("CARGO_PKG_VERSION"),
    }))
}

#[tauri::command]
pub async fn install_claude_desktop_extension() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(install_claude_desktop_extension_blocking)
        .await
        .map_err(|error| format!("Could not prepare the Claude Desktop integration: {error}"))?
}

#[cfg(not(windows))]
#[tauri::command]
pub fn open_claude_desktop_download() -> Result<(), String> {
    webbrowser::open("https://claude.ai/download")
        .map(|_| ())
        .map_err(|error| format!("Could not open the Claude Desktop download page: {error}"))
}

fn open_claude_desktop_blocking() -> Result<(), String> {
    if let Some(claude) = claude_desktop_installation()
        .and_then(|installation| installation.executable)
        .or_else(claude_msix_path)
    {
        return spawn_desktop_command(desktop_application_command(&claude))
            .map_err(|error| format!("Could not open Claude Desktop: {error}"));
    }
    if claude_uri_handler_registered() {
        #[cfg(windows)]
        {
            return Command::new(crate::system_tools::explorer_exe())
                .arg("claude://")
                .spawn()
                .map(|_| ())
                .map_err(|error| format!("Could not open Claude Desktop: {error}"));
        }
        #[cfg(not(windows))]
        {
            return Err(
                "Claude Desktop URI launching is unavailable on this platform.".to_string(),
            );
        }
    }
    Err("Claude Desktop is not installed for this user account.".to_string())
}

#[tauri::command]
pub async fn open_claude_desktop() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(open_claude_desktop_blocking)
        .await
        .map_err(|error| format!("Could not launch Claude Desktop: {error}"))?
}

// Same skill evidence policy as the other managed hosts; route health is independent.
fn apply_chatgpt_device_knowledge_skill_status(status: &mut Value, skill_status: &Value) {
    apply_device_knowledge_skill_status(status, skill_status);
}

fn chatgpt_local_integration_status_blocking(
    evidence: LocalClientEvidence,
) -> Result<Value, String> {
    let desktop = chatgpt_desktop_installation();
    let codex_cli_installed = codex_cli_executable().is_some();
    let config_path = codex_config_path();
    let executable_path = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    let skill_status = codex_device_knowledge_skill_path()
        .map(|path| device_knowledge_skill_status_at(&path))
        .unwrap_or_else(|| {
            json!({
                "installed": false,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "status-error",
                "statusError": "The user home directory is unavailable.",
            })
        });
    let (route, status_error) = match config_path.as_deref() {
        Some(config_path) => match configured_chatgpt_route(config_path, &executable_path) {
            Ok(route) => (route, None),
            Err(error) => (ConfiguredChatgptRoute::default(), Some(error)),
        },
        None => (
            ConfiguredChatgptRoute::default(),
            Some("The user home directory is unavailable.".to_string()),
        ),
    };
    let configuration_current = route.launcher_current && route.approval_mode_current;
    let mut status = chatgpt_status_document(
        desktop.is_some() || chatgpt_package_registered() || codex_cli_installed,
        route.configured,
        route.launcher_current,
        route.approval_mode_current,
        route
            .launcher_current
            .then(|| env!("CARGO_PKG_VERSION").to_string())
            .or_else(|| route.managed_version.clone()),
        status_error,
        configuration_current
            && host_restart_required(
                config_path.as_deref().is_some_and(chatgpt_restart_required),
                evidence,
            ),
        evidence,
    );
    if let Some(version) = desktop.and_then(|installation| installation.version) {
        status["appVersion"] = Value::String(version);
    }
    status["chatgptRunning"] = Value::Bool(chatgpt_is_running());
    status["codexCliInstalled"] = Value::Bool(codex_cli_installed);
    status["clientName"] = Value::String(if cfg!(target_os = "linux") {
        "Codex CLI".to_owned()
    } else {
        "ChatGPT".to_owned()
    });
    apply_chatgpt_device_knowledge_skill_status(&mut status, &skill_status);
    status["routeRefresh"] = route_refresh_report(CHATGPT_ROUTE);
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    annotate_folder_access(&mut status, "codex");
    Ok(status)
}

#[tauri::command]
pub async fn chatgpt_local_integration_status() -> Result<Value, String> {
    // The global native-bridge slot is written by every `/rpc` caller, so
    // Antigravity, LM Studio, Jan or an "other" host making one tool call used
    // to mark ChatGPT connected and suppress its restart prompt. Read once, so
    // liveness, attachment and the restart rule cannot disagree about which
    // instant they are describing. The prompt is no longer a row state; the
    // rule now only selects which sentence the paired row uses.
    let evidence = local_client_evidence(CODEX_CLIENT_ID);
    inspect_client_until(Instant::now() + CLIENT_INSPECTION_TIMEOUT, move || {
        chatgpt_local_integration_status_blocking(evidence)
    })
    .await
}

fn install_chatgpt_local_integration_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    authorize_local_client_folder("codex")?;
    if !chatgpt_desktop_installed() {
        #[cfg(target_os = "linux")]
        let message = "Codex CLI is not installed for this Linux user.";
        #[cfg(not(target_os = "linux"))]
        let message = "ChatGPT Desktop or Codex is not installed for this user account.";
        return Err(message.to_string());
    }
    let executable_path = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    let config_path =
        codex_config_path().ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let skill_path = codex_device_knowledge_skill_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let backup_root = device_knowledge_skill_backup_root(CODEX_SKILL_HOST)?;
    // Version 0.1.114 and earlier installed this PowerShell shim. The managed
    // route now launches QC Remote's native bridge directly and needs no Node.
    let legacy_runner = canonical_integration_directory()?.join(LEGACY_CHATGPT_RUNNER_FILE);
    if legacy_runner.is_file() {
        // It is no longer referenced. A locked legacy file must not turn a
        // successful native install into a misleading failure.
        let _ = fs::remove_file(&legacy_runner);
    }
    #[cfg(any(windows, target_os = "macos"))]
    let mut result = {
        let directory = crate::local_http_helper::directory()?;
        fs::create_dir_all(&directory)
            .map_err(|_| "Could not prepare the local HTTP pairing directory")?;
        let port_file = directory.join(crate::local_http_helper::PORT_FILE);
        let _pairing =
            crate::local_access::acquire_document_lease(&port_file, "pairing", Duration::ZERO)?;
        let port = crate::local_http_binding::StablePort::select_for_pairing(&port_file)?;
        // The already-running native host adopts the selected port. This
        // action never starts/enables a device or respawns a user-quit host.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Could not verify the native shared HTTP host")?;
        runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if crate::local_http_helper::verified_headers(&directory).await.is_ok() { return Ok::<(),String>(()); }
                if tokio::time::Instant::now() >= deadline {
                    return Err("The shared HTTP host is unavailable. Open QC Remote and enable Local access, then Pair again. The Codex configuration was not changed.".into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })?;
        let mut result = pair_codex_route_and_skill(
            &config_path,
            codex_http_route_table(&executable_path, CODEX_CLIENT_ID, port)?,
            &skill_path,
            &backup_root,
        )?;
        result["transport"] = json!("streamable_http");
        result
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut result = pair_chatgpt_route(&config_path, &executable_path, &skill_path, &backup_root)?;
    result["restartRequired"] = Value::Bool(chatgpt_restart_required(&config_path));
    Ok(result)
}

/// The MCP entry is written first and the skill installed second, and the
/// result describes each independently.  The other order installed the skill -
/// moving the user's own `qc-device-knowledge` aside - before the write that
/// could still fail, and returned an `Err` the caller could only report as an
/// outright failure, with the displaced skill nowhere in it.
#[cfg(any(not(any(windows, target_os = "macos")), test))]
fn pair_chatgpt_route(
    config_path: &Path,
    executable_path: &Path,
    skill_path: &Path,
    skill_backup_root: &Path,
) -> Result<Value, String> {
    pair_codex_route_and_skill(
        config_path,
        codex_route_table(executable_path, CODEX_CLIENT_ID),
        skill_path,
        skill_backup_root,
    )
}

fn pair_codex_route_and_skill(
    config_path: &Path,
    route: Table,
    skill_path: &Path,
    skill_backup_root: &Path,
) -> Result<Value, String> {
    write_codex_config_with_route(config_path, route)?;
    let skill = install_device_knowledge_skill_at(skill_path, skill_backup_root);
    let mut result = json!({
        "installed": true,
        "availableVersion": env!("CARGO_PKG_VERSION"),
    });
    merge_object(&mut result, skill.report());
    Ok(result)
}

/// Folds one flat JSON object into another.  Used to attach a sub-result - the
/// skill outcome, an Antigravity refresh - without restating its field names.
fn merge_object(target: &mut Value, source: Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        target.extend(source.clone());
    }
}

/// Publish a skill status onto a provider row built by
/// `local_json_integration_status`, and fold the skill's own state into the
/// row's.
///
/// `antigravity_status_document` does both inline because it builds its row
/// field by field. A provider on the generic document needs the same two
/// things, and they have to stay identical: the five `skill*` fields are what
/// the panel reads, and the fold is what makes a paired app with a missing or
/// stale skill offer **Update** instead of **Open**. Splitting the fold from
/// the fields would let a row report `skillState: "not-installed"` while its
/// own state still said `current`, which is a row that shows Open and never
/// installs the skill.
///
/// Only `current`, `newer-installed` and `update-available` fold. Any other row state is already
/// describing something worse than a skill being out of date - not installed,
/// a configuration this app does not own - and a skill cannot improve it.
fn apply_device_knowledge_skill_status(status: &mut Value, skill_status: &Value) {
    let row_state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if matches!(
        row_state.as_str(),
        "current" | "newer-installed" | "update-available"
    ) {
        let folded = match skill_status["state"].as_str() {
            Some("current" | "newer-installed") => row_state.as_str(),
            Some("not-installed" | "update-available") => "update-available",
            // Unreadable is not the same as different. A skill folder this app
            // could not inspect - locked, or its marker unreadable - used to
            // fall through to "different-configuration", which tells the owner
            // their skill conflicts with the managed one and offers an Update
            // that would try to back up the very directory that is already
            // failing to read. The row keeps its own state and reports the
            // error it actually has, in `skillStatusError`.
            Some("status-error") => row_state.as_str(),
            _ => "different-configuration",
        };
        status["state"] = Value::String(folded.to_string());
    }
    merge_object(
        status,
        json!({
            "skillInstalled": skill_status["installed"],
            "installedSkillVersion": skill_status.get("installedVersion").cloned().unwrap_or(Value::Null),
            "availableSkillVersion": skill_status["availableVersion"],
            "skillState": skill_status["state"],
            "skillStatusError": skill_status.get("statusError").cloned().unwrap_or(Value::Null),
        }),
    );
}

#[tauri::command]
pub async fn install_chatgpt_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(install_chatgpt_local_integration_blocking)
        .await
        .map_err(|error| {
            format!("Could not configure the local Codex/ChatGPT integration: {error}")
        })?
}

fn remove_chatgpt_local_integration_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    authorize_local_client_folder("codex")?;
    let config_path =
        codex_config_path().ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let skill_path = codex_device_knowledge_skill_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let mut removed = remove_codex_route(&config_path)?;
    // The JavaScript bridge is shared by manual clients. Only the obsolete
    // PowerShell shim from the pre-native managed route can be removed here.
    let path = canonical_integration_directory()?.join(LEGACY_CHATGPT_RUNNER_FILE);
    if path.is_file() {
        fs::remove_file(&path)
            .map_err(|error| format!("Could not remove {}: {error}", path.display()))?;
        removed = true;
    }
    let skill = remove_device_knowledge_skill_at(
        &skill_path,
        &device_knowledge_skill_backup_root(CODEX_SKILL_HOST)?,
    )?;
    Ok(json!({
        "removed": removed || skill.removed,
        "restartRequired": removed && chatgpt_is_running(),
        "skillRemoved": skill.removed,
        "skillRestoredPath": skill
            .restored_from
            .map(|path| path.to_string_lossy().into_owned()),
    }))
}

#[tauri::command]
pub async fn remove_chatgpt_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(remove_chatgpt_local_integration_blocking)
        .await
        .map_err(|error| format!("Could not remove the local Codex/ChatGPT integration: {error}"))?
}

fn open_chatgpt_desktop_blocking() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let codex = codex_cli_executable()
            .ok_or_else(|| "Codex CLI is not installed for this Linux user.".to_owned())?;
        launch_codex_cli_in_terminal(&codex)
    }
    #[cfg(target_os = "windows")]
    {
        let chatgpt = chatgpt_desktop_installation()
            .and_then(|installation| installation.executable)
            .or_else(chatgpt_msix_path);
        let Some(chatgpt) = chatgpt else {
            return Err(
                "ChatGPT Desktop or Codex is not installed for this user account.".to_string(),
            );
        };
        spawn_desktop_command(desktop_application_command(&chatgpt))
            .map_err(|error| format!("Could not open ChatGPT Desktop: {error}"))
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(chatgpt) =
            chatgpt_desktop_installation().and_then(|installation| installation.executable)
        {
            return spawn_desktop_command(desktop_application_command(&chatgpt))
                .map_err(|error| format!("Could not open ChatGPT Desktop: {error}"));
        }
        let codex = codex_cli_executable().ok_or_else(|| {
            "ChatGPT Desktop or Codex CLI is not installed for this user.".to_owned()
        })?;
        launch_codex_cli_in_terminal(&codex)
    }
}

#[cfg(target_os = "macos")]
fn launch_codex_cli_in_terminal(codex: &Path) -> Result<(), String> {
    let codex = codex
        .to_str()
        .ok_or("The Codex CLI path is not valid Unicode")?;
    if codex.chars().any(char::is_control) {
        return Err("The Codex CLI path contains unsupported control characters".into());
    }
    let shell_command = format!("'{}'", codex.replace('\'', "'\\''"));
    let applescript_command = shell_command.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("tell application \"Terminal\" to do script \"{applescript_command}\"");
    let mut command = Command::new("/usr/bin/osascript");
    command.arg("-e").arg(script);
    spawn_desktop_command(command)
        .map_err(|error| format!("Could not open Codex CLI in Terminal: {error}"))
}

#[cfg(target_os = "linux")]
fn launch_codex_cli_in_terminal(codex: &Path) -> Result<(), String> {
    for (terminal, separator) in [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xterm", "-e"),
    ] {
        let Some(program) = where_command(terminal).into_iter().next() else {
            continue;
        };
        let mut command = Command::new(program);
        command.arg(separator).arg(codex);
        return crate::broker_tray_control::spawn_owned(command)
            .map_err(|error| format!("Could not open Codex CLI in a terminal: {error}"));
    }
    Err(
        "Codex CLI is installed, but no terminal emulator is available; run `codex` in a terminal."
            .to_owned(),
    )
}

#[tauri::command]
pub async fn open_chatgpt_desktop() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(open_chatgpt_desktop_blocking)
        .await
        .map_err(|error| {
            if cfg!(any(target_os = "linux", target_os = "macos")) {
                format!("Could not launch Codex CLI: {error}")
            } else {
                format!("Could not launch ChatGPT Desktop: {error}")
            }
        })?
}

#[cfg(not(windows))]
#[tauri::command]
pub fn open_chatgpt_desktop_download() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    let url = "https://developers.openai.com/codex/cli";
    #[cfg(not(target_os = "linux"))]
    let url = "https://chatgpt.com/download/";
    webbrowser::open(url)
        .map(|_| ())
        .map_err(|error| format!("Could not open the client download page: {error}"))
}

#[tauri::command]
pub fn open_chatgpt_configuration() -> Result<(), String> {
    let config_path =
        codex_config_path().ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    if !config_path.is_file() {
        return Err("The shared ChatGPT and Codex MCP configuration does not exist.".to_string());
    }
    #[cfg(target_os = "windows")]
    {
        let result = Command::new(crate::system_tools::notepad_exe())
            .arg(&config_path)
            .spawn();
        result.map(|_| ()).map_err(|error| {
            format!("Could not open the shared ChatGPT and Codex MCP configuration: {error}")
        })
    }
    #[cfg(target_os = "linux")]
    {
        let mut command = Command::new("xdg-open");
        command.arg(&config_path);
        crate::broker_tray_control::spawn_owned(command).map_err(|error| {
            format!("Could not open the shared ChatGPT and Codex MCP configuration: {error}")
        })
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("/usr/bin/open");
        command.arg(&config_path);
        spawn_desktop_command(command).map_err(|error| {
            format!("Could not open the shared ChatGPT and Codex MCP configuration: {error}")
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    Err("opening the MCP configuration is unsupported on this platform".to_owned())
}

fn antigravity_status_document(
    antigravity_installed: bool,
    integration: &AntigravityIntegration,
    skill_status: &Value,
    status_error: Option<String>,
    connection_active: bool,
    evidence: LocalClientEvidence,
) -> Value {
    let AntigravityIntegration {
        route,
        permission_current,
        legacy_route_present,
    } = integration;
    let available_version = env!("CARGO_PKG_VERSION");
    let route_current = route.configured && route.launcher_current && route.enabled;
    let managed_current = route_current && *permission_current && !legacy_route_present;
    let managed_update = route.configured
        && route.managed_version.is_some()
        && route.enabled
        && *permission_current
        && !legacy_route_present;
    let mut state = if !antigravity_installed {
        "antigravity-not-installed"
    } else if status_error.is_some() {
        "status-error"
    } else if !route.configured {
        "not-installed"
    } else if managed_current {
        "current"
    } else if managed_update {
        "update-available"
    } else {
        "different-configuration"
    };
    if matches!(state, "current" | "update-available") {
        match skill_status["state"].as_str() {
            Some("current" | "newer-installed") => {}
            Some("not-installed" | "update-available") => state = "update-available",
            // See `apply_device_knowledge_skill_status`: unreadable is not the
            // same as different, and must not be reported as a conflict.
            Some("status-error") => {}
            _ => state = "different-configuration",
        }
    }
    json!({
        "antigravityInstalled": antigravity_installed,
        "integrationInstalled": route.configured,
        "installedVersion": route.managed_version.clone(),
        "availableVersion": available_version,
        "state": state,
        "statusError": status_error,
        "connectionActive": managed_current && connection_active,
        // The language-server probe can report a live connection that the
        // heartbeat record has not seen, so attachment takes either; the
        // start-up window is the heartbeat's alone, and it closes the moment
        // the probe answers.
        "bridgeAttached": managed_current && (connection_active || evidence.attached()),
        "connectionPending": managed_current && !connection_active && evidence.settling(),
        "skillInstalled": skill_status["installed"],
        "installedSkillVersion": skill_status.get("installedVersion").cloned().unwrap_or(Value::Null),
        "availableSkillVersion": skill_status["availableVersion"],
        "skillState": skill_status["state"],
        "skillStatusError": skill_status.get("statusError").cloned().unwrap_or(Value::Null),
    })
}

fn antigravity_local_integration_status_blocking(
    connection_active: bool,
    evidence: LocalClientEvidence,
) -> Result<Value, String> {
    let installation = antigravity_installation();
    let executable_path = antigravity_bridge_path()?;
    let config_path = antigravity_config_path();
    let settings_path = antigravity_settings_path();
    let legacy_settings_path = legacy_gemini_settings_path();
    let skill_status = antigravity_device_knowledge_skill_path()
        .map(|path| device_knowledge_skill_status_at(&path))
        .unwrap_or_else(|| {
            json!({
                "installed": false,
                "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                "state": "status-error",
                "statusError": "The user home directory is unavailable.",
            })
        });
    let unconfigured = ConfiguredAntigravityRoute {
        configured: false,
        launcher_current: false,
        managed_version: None,
        enabled: true,
    };
    let (route, mut status_error) = match config_path.as_deref() {
        Some(path) => match configured_antigravity_route(path, &executable_path) {
            Ok(route) => (route, None),
            Err(error) => (unconfigured, Some(error)),
        },
        None => (
            unconfigured,
            Some("The user home directory is unavailable.".to_string()),
        ),
    };
    let mut integration = AntigravityIntegration {
        route,
        permission_current: false,
        legacy_route_present: false,
    };
    if status_error.is_none() {
        match settings_path.as_deref() {
            Some(path) => match antigravity_permission_configured(path) {
                Ok(configured) => integration.permission_current = configured,
                Err(error) => status_error = Some(error),
            },
            None => status_error = Some("The user home directory is unavailable.".into()),
        }
    }
    if status_error.is_none() {
        match legacy_settings_path.as_deref() {
            Some(path) => match legacy_gemini_route_present(path) {
                Ok(present) => integration.legacy_route_present = present,
                Err(error) => status_error = Some(error),
            },
            None => status_error = Some("The user home directory is unavailable.".into()),
        }
    }
    let mut status = antigravity_status_document(
        installation.is_some(),
        &integration,
        &skill_status,
        status_error,
        connection_active,
        evidence,
    );
    if let Some(version) = installation.and_then(|value| value.version) {
        status["appVersion"] = Value::String(version);
    }
    status["routeRefresh"] = route_refresh_report(ANTIGRAVITY_ROUTE);
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    annotate_folder_access(&mut status, "antigravity");
    Ok(status)
}

#[tauri::command]
pub async fn antigravity_local_integration_status() -> Result<Value, String> {
    let deadline = Instant::now() + CLIENT_INSPECTION_TIMEOUT;
    let evidence = antigravity_bridge_evidence();
    let connection_active = antigravity_runtime_connected_until(deadline).await;
    inspect_client_until(deadline, move || {
        antigravity_local_integration_status_blocking(connection_active, evidence)
    })
    .await
}

fn open_antigravity_blocking() -> Result<(), String> {
    let executable = antigravity_installation()
        .and_then(|installation| installation.executable)
        .ok_or_else(|| "Google Antigravity is not installed on this computer.".to_string())?;
    #[cfg(target_os = "linux")]
    {
        let command = Command::new(executable);
        crate::broker_tray_control::spawn_owned(command)
            .map_err(|error| format!("Could not open Google Antigravity: {error}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let command = desktop_application_command(&executable);
        spawn_desktop_command(command)
            .map_err(|error| format!("Could not open Google Antigravity: {error}"))
    }
}

#[tauri::command]
pub async fn open_antigravity() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(open_antigravity_blocking)
        .await
        .map_err(|error| format!("Could not launch Google Antigravity: {error}"))?
}

#[cfg(not(windows))]
#[tauri::command]
pub fn open_antigravity_download() -> Result<(), String> {
    webbrowser::open(ANTIGRAVITY_DOWNLOAD_URL)
        .map(|_| ())
        .map_err(|error| format!("Could not open the Google Antigravity download page: {error}"))
}

fn install_antigravity_local_integration_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    authorize_local_client_folder("antigravity")?;
    if antigravity_installation().is_none() {
        return Err("Google Antigravity is not installed for this user account.".to_string());
    }
    let executable_path = antigravity_bridge_path()?;
    let config_path = antigravity_config_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let settings_path = antigravity_settings_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let legacy_settings_path = legacy_gemini_settings_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let skill_path = antigravity_device_knowledge_skill_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let backup_root = device_knowledge_skill_backup_root(ANTIGRAVITY_SKILL_HOST)?;
    // Same ordering rule as ChatGPT: every reversible configuration write lands
    // before the one step that displaces a directory the user owns.
    write_antigravity_config(&config_path, &executable_path, ANTIGRAVITY_CLIENT_ID)?;
    write_antigravity_permission(&settings_path)?;
    remove_legacy_gemini_route(&legacy_settings_path)?;
    // No copy is installed any more, so every managed bridge still sitting in
    // an integrations directory is stale and none of them is in use.
    cleanup_stale_antigravity_bridges(None);
    let skill = install_device_knowledge_skill_at(&skill_path, &backup_root);
    let mut result = json!({
        "installed": true,
        "launched": false,
        "availableVersion": env!("CARGO_PKG_VERSION"),
    });
    merge_object(&mut result, skill.report());
    Ok(result)
}

#[tauri::command]
pub async fn install_antigravity_local_integration() -> Result<Value, String> {
    ensure_and_refresh_antigravity_local_integration().await
}

async fn ensure_and_refresh_antigravity_local_integration() -> Result<Value, String> {
    let mut result =
        tauri::async_runtime::spawn_blocking(install_antigravity_local_integration_blocking)
            .await
            .map_err(|error| format!("Could not pair Google Antigravity: {error}"))??;
    let refresh = refresh_running_antigravity_mcp().await;
    if let (Some(result), Some(refresh)) = (result.as_object_mut(), refresh.as_object()) {
        result.extend(refresh.clone());
    }
    Ok(result)
}

fn remove_antigravity_local_integration_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    authorize_local_client_folder("antigravity")?;
    let config_path = antigravity_config_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let settings_path = antigravity_settings_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let legacy_settings_path = legacy_gemini_settings_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let skill_path = antigravity_device_knowledge_skill_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let removed_route = remove_antigravity_route(&config_path)?;
    let removed_permission = remove_antigravity_permission(&settings_path)?;
    let removed_legacy_route = remove_legacy_gemini_route(&legacy_settings_path)?;
    cleanup_stale_antigravity_bridges(None);
    let skill = remove_device_knowledge_skill_at(
        &skill_path,
        &device_knowledge_skill_backup_root(ANTIGRAVITY_SKILL_HOST)?,
    )?;
    Ok(json!({
        "removed": removed_route || removed_permission || removed_legacy_route || skill.removed,
        "skillRemoved": skill.removed,
        "skillRestoredPath": skill
            .restored_from
            .map(|path| path.to_string_lossy().into_owned()),
    }))
}

#[tauri::command]
pub async fn remove_antigravity_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(remove_antigravity_local_integration_blocking)
        .await
        .map_err(|error| format!("Could not unpair Google Antigravity: {error}"))?
}

/// One numbering series of a host application, and what this app needs from it.
///
/// A minimum version is only meaningful inside the series it was set for.
/// When a product renumbers - as LM Studio did when it became Bionic and
/// restarted at 1.x after a 0.3.x line - a single threshold silently starts
/// describing versions nobody ever checked it against. Declaring the series
/// makes the ambiguity impossible to write down by accident: a threshold always
/// arrives beside the numbering line it belongs to.
///
/// Rows are ordered newest series first, and an installed version belongs to
/// the first row whose `series_begins_at` it reaches.
struct AppVersionSeries {
    /// What this numbering line is called - which is the product that ships it,
    /// because here a rename is what produced the renumbering.
    ///
    /// **This string is user-visible.** It reaches the status document as
    /// `appVersionSeries`, and the LM Studio row is headed by it: `Bionic` for
    /// the 1.x line, `LM Studio` for the 0.3.x line below it. That is
    /// deliberate - it makes the name the row shows and the threshold the row
    /// was judged against a single reading, so the two cannot disagree - and it
    /// means renaming a series renames a row. Name a series after its product.
    name: &'static str,
    /// The lowest version number this series uses. The oldest row carries
    /// `"0"`, which every parsable version reaches, so no version is homeless.
    series_begins_at: &'static str,
    /// The lowest version of *this series* QC Remote can pair with. Equal to
    /// `series_begins_at` when the whole series qualifies.
    minimum: &'static str,
}

/// Which numbering series an installed version belongs to: the first row,
/// newest first, whose floor it reaches.
///
/// `None` means this table has no opinion about the version - it is below
/// every declared series, or it is not a version number this app can read at
/// all. Matching demands a *decided* comparison for exactly that reason: an
/// unreadable version compares as `None` against every floor, and silently
/// filing it under the newest series would be an invention, not a reading.
fn app_version_series<'a>(
    version: &str,
    series: &'a [AppVersionSeries],
) -> Option<&'a AppVersionSeries> {
    series.iter().find(|row| {
        matches!(
            compare_versions(version, row.series_begins_at),
            Some(Ordering::Greater | Ordering::Equal)
        )
    })
}

/// Whether `version` satisfies the series it belongs to.
///
/// Unknown is treated as supported throughout - an unparsable version, or a
/// version below every declared series - because the alternative is telling the
/// owner to update an application that may be perfectly current, and that
/// accusation costs more than the check is worth.
///
/// `compare_versions` zero-extends the shorter side, which is load-bearing
/// here: `executable_version` reads VS_FIXEDFILEINFO and therefore always
/// yields four components, so a three-component threshold is only ever compared
/// against `a.b.c.d`. `0.3.17` versus a legacy `0.3.17.0` is Equal, not Less.
fn app_version_supported(version: &str, series: &[AppVersionSeries]) -> bool {
    app_version_series(version, series)
        .is_none_or(|row| compare_versions(version, row.minimum) != Some(Ordering::Less))
}

fn local_json_integration_status(
    installed: bool,
    installation: Option<DesktopInstallation>,
    route: Result<ConfiguredJsonMcpRoute, String>,
    installed_key: &str,
    client_id: &str,
    supported_versions: &[AppVersionSeries],
) -> Value {
    let (route, status_error) = match route {
        Ok(route) => (route, None),
        Err(error) => (
            ConfiguredJsonMcpRoute {
                configured: false,
                launcher_current: false,
                managed_version: None,
                enabled: true,
            },
            Some(error),
        ),
    };
    let installed_version = installation
        .as_ref()
        .and_then(|value| value.version.as_deref());
    // Which numbering series the gate judged this install in. "App update
    // required" used to be unfalsifiable from outside: the panel showed the
    // verdict and nothing said which threshold produced it, so a threshold left
    // behind by a rename could accuse a current install and look like a fact.
    let version_series = installed_version
        .and_then(|version| app_version_series(version, supported_versions))
        .map(|row| row.name);
    let app_supported =
        installed_version.is_none_or(|version| app_version_supported(version, supported_versions));
    let state = if !installed {
        "app-not-installed"
    } else if !app_supported {
        "app-update-required"
    } else if status_error.is_some() {
        "status-error"
    } else if !route.configured {
        "not-installed"
    } else if route.launcher_current && route.enabled {
        "current"
    } else if route.managed_version.is_some() && route.enabled {
        "update-available"
    } else {
        "different-configuration"
    };
    let evidence = local_client_evidence(client_id);
    let current = state == "current";
    let mut status = json!({
        "integrationInstalled": route.configured,
        "installedVersion": route.managed_version,
        "availableVersion": env!("CARGO_PKG_VERSION"),
        "state": state,
        "statusError": status_error,
        "appVersionSeries": version_series,
        "connectionActive": current && evidence.connected(),
        "bridgeAttached": current && evidence.attached(),
        "connectionPending": current && evidence.settling(),
    });
    status[installed_key] = Value::Bool(installed);
    if let Some(version) = installation.and_then(|value| value.version) {
        status["appVersion"] = Value::String(version);
    }
    status
}

fn lm_studio_status_blocking() -> Result<Value, String> {
    let installation = lm_studio_installation();
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    // The document this install actually reads. A route in the other file is
    // not a pairing - it is a file nothing opens - so it is deliberately not
    // consulted here: reporting it as paired is what made pressing Pair report
    // success and change nothing observable.
    let document = lm_studio_mcp_document(installation.as_ref())?;
    let route = configured_lm_studio_route(&document, &executable);
    let mut status = local_json_integration_status(
        installation.is_some(),
        installation,
        route,
        "lmStudioInstalled",
        LM_STUDIO_CLIENT_ID,
        LM_STUDIO_SUPPORTED_VERSIONS,
    );
    status["routeRefresh"] = route_refresh_report(LM_STUDIO_ROUTE);
    apply_device_knowledge_skill_status(
        &mut status,
        &bionic_device_knowledge_skill_path()
            .map(|path| device_knowledge_skill_status_at(&path))
            .unwrap_or_else(|| {
                json!({
                    "installed": false,
                    "availableVersion": DEVICE_KNOWLEDGE_SKILL_VERSION,
                    "state": "status-error",
                    "statusError": "The user home directory is unavailable.",
                })
            }),
    );
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    annotate_folder_access(&mut status, "lm-studio");
    Ok(status)
}

#[tauri::command]
pub async fn lm_studio_local_integration_status() -> Result<Value, String> {
    inspect_client_until(
        Instant::now() + CLIENT_INSPECTION_TIMEOUT,
        lm_studio_status_blocking,
    )
    .await
}

fn install_lm_studio_local_integration_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    authorize_local_client_folder("lm-studio")?;
    let Some(installation) = lm_studio_installation() else {
        return Err("LM Studio is not installed for this user account.".to_string());
    };
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    let document = lm_studio_mcp_document(Some(&installation))?;
    let skill_path = bionic_device_knowledge_skill_path()
        .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
    let backup_root = device_knowledge_skill_backup_root(LM_STUDIO_SKILL_HOST)?;
    // Same ordering rule as ChatGPT and Antigravity: the reversible
    // configuration write lands before the one step that displaces a directory
    // the user owns.
    write_lm_studio_route(&document, &executable, LM_STUDIO_CLIENT_ID)?;
    let skill = install_device_knowledge_skill_at(&skill_path, &backup_root);
    let mut result = json!({ "installed": true, "availableVersion": env!("CARGO_PKG_VERSION") });
    merge_object(&mut result, skill.report());
    Ok(result)
}

#[tauri::command]
pub async fn install_lm_studio_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(install_lm_studio_local_integration_blocking)
        .await
        .map_err(|error| format!("Could not pair LM Studio: {error}"))?
}

#[tauri::command]
pub async fn remove_lm_studio_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
        authorize_local_client_folder("lm-studio")?;
        // Both documents, because unpairing is "take this app's entry out",
        // and this app's entry can be in either: the rename moved the file a
        // pairing is written to, so a machine upgraded from LM Studio to
        // Bionic carries one entry this build wrote and one an older build
        // wrote. Removing only the one the current install reads would leave
        // the other behind for ever, and the removal touches nothing but this
        // app's own entry in each.
        let bionic = bionic_config_path()
            .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
        let legacy = lm_studio_config_path()
            .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
        let removed_bionic = remove_bionic_mcp_route(&bionic, LM_STUDIO_CONFIG_OWNER)?;
        let removed_legacy = remove_json_mcp_route(&legacy, LM_STUDIO_CONFIG_OWNER)?;
        // The skill goes back the way it came: whatever this app displaced is
        // restored, and a directory it did not install is left alone.
        let skill_path = bionic_device_knowledge_skill_path()
            .ok_or_else(|| "The user home directory is unavailable.".to_string())?;
        let skill = remove_device_knowledge_skill_at(
            &skill_path,
            &device_knowledge_skill_backup_root(LM_STUDIO_SKILL_HOST)?,
        )?;
        Ok(json!({
            "removed": removed_bionic || removed_legacy || skill.removed,
            "skillRemoved": skill.removed,
            "skillRestoredPath": skill
                .restored_from
                .map(|path| path.to_string_lossy().into_owned()),
        }))
    })
    .await
    .map_err(|error| format!("Could not unpair LM Studio: {error}"))?
}

#[tauri::command]
pub async fn open_lm_studio() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let executable = lm_studio_installation()
            .and_then(|installation| installation.executable)
            .ok_or_else(|| "LM Studio is not installed on this computer.".to_string())?;
        #[cfg(target_os = "linux")]
        {
            let command = Command::new(executable);
            crate::broker_tray_control::spawn_owned(command)
                .map_err(|error| format!("Could not open LM Studio: {error}"))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let command = desktop_application_command(&executable);
            spawn_desktop_command(command)
                .map_err(|error| format!("Could not open LM Studio: {error}"))
        }
    })
    .await
    .map_err(|error| format!("Could not launch LM Studio: {error}"))?
}

#[cfg(not(windows))]
#[tauri::command]
pub fn open_lm_studio_download() -> Result<(), String> {
    webbrowser::open(LM_STUDIO_DOWNLOAD_URL)
        .map(|_| ())
        .map_err(|error| format!("Could not open the LM Studio download page: {error}"))
}

fn jan_status_blocking() -> Result<Value, String> {
    let installation = jan_installation();
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    let config =
        jan_config_path().ok_or_else(|| "Jan's data directory is unavailable.".to_string())?;
    let route = configured_json_mcp_route(&config, &executable, "jan:", JAN_CLIENT_ID, "Jan", true);
    let mut status = local_json_integration_status(
        installation.is_some(),
        installation,
        route,
        "janInstalled",
        JAN_CLIENT_ID,
        JAN_SUPPORTED_VERSIONS,
    );
    status["routeRefresh"] = route_refresh_report(JAN_ROUTE);
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    annotate_folder_access(&mut status, "jan");
    Ok(status)
}

#[tauri::command]
pub async fn jan_local_integration_status() -> Result<Value, String> {
    inspect_client_until(
        Instant::now() + CLIENT_INSPECTION_TIMEOUT,
        jan_status_blocking,
    )
    .await
}

fn install_jan_local_integration_blocking() -> Result<Value, String> {
    #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
    authorize_local_client_folder("jan")?;
    if jan_installation().is_none() {
        return Err(if cfg!(target_os = "linux") {
            "Jan pairing is unavailable on Linux; use the generic MCP configuration instead."
                .to_string()
        } else {
            "Jan is not installed for this user account.".to_string()
        });
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    let config =
        jan_config_path().ok_or_else(|| "Jan's data directory is unavailable.".to_string())?;
    write_json_mcp_route(&config, &executable, JAN_CLIENT_ID, "Jan", true)?;
    Ok(json!({ "installed": true, "availableVersion": env!("CARGO_PKG_VERSION") }))
}

#[tauri::command]
pub async fn install_jan_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(install_jan_local_integration_blocking)
        .await
        .map_err(|error| format!("Could not pair Jan: {error}"))?
}

#[tauri::command]
pub async fn remove_jan_local_integration() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        #[cfg(all(target_os = "macos", feature = "macos-app-store"))]
        authorize_local_client_folder("jan")?;
        let config =
            jan_config_path().ok_or_else(|| "Jan's data directory is unavailable.".to_string())?;
        Ok(json!({ "removed": remove_json_mcp_route(&config, "Jan")? }))
    })
    .await
    .map_err(|error| format!("Could not unpair Jan: {error}"))?
}

#[tauri::command]
pub async fn open_jan() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        #[cfg(target_os = "linux")]
        {
            Err("Jan is not available as a native MCP host on Linux.".to_owned())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let executable = jan_installation()
                .and_then(|installation| installation.executable)
                .ok_or_else(|| "Jan is not installed for this user account.".to_string())?;
            let command = desktop_application_command(&executable);
            spawn_desktop_command(command).map_err(|error| format!("Could not open Jan: {error}"))
        }
    })
    .await
    .map_err(|error| format!("Could not launch Jan: {error}"))?
}

#[cfg(not(windows))]
#[tauri::command]
pub fn open_jan_download() -> Result<(), String> {
    webbrowser::open(JAN_DOWNLOAD_URL)
        .map(|_| ())
        .map_err(|error| format!("Could not open the Jan download page: {error}"))
}

#[tauri::command]
pub fn other_mcp_stdio_setup() -> Result<Value, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    // The pasted block names itself the same way every managed route does, so
    // a hand-configured host is observable on the same terms: `env` for the
    // clients that pass it through, and the argument for the ones that keep it
    // to themselves. Without the argument this row could only ever report
    // "STDIO available", however well the client was working.
    let configuration = json!({
        "mcpServers": {
            "qc-remote": {
                "command": executable.to_string_lossy(),
                "args": ["--local-mcp-bridge", CLIENT_ID_ARGUMENT, OTHER_MCP_CLIENT_ID],
                "env": { "QC_REMOTE_LOCAL_MCP_CLIENT": OTHER_MCP_CLIENT_ID }
            }
        }
    });
    let evidence = local_client_evidence(OTHER_MCP_CLIENT_ID);
    Ok(json!({
        "configuration": serde_json::to_string_pretty(&configuration)
            .map_err(|error| format!("Could not create the STDIO configuration: {error}"))?,
        "availableVersion": env!("CARGO_PKG_VERSION"),
        "connectionActive": evidence.connected(),
        "bridgeAttached": evidence.attached(),
        "connectionPending": evidence.settling(),
    }))
}

/// Every release stamps its own version into the client id each managed route
/// names itself by, so shipping 0.1.146 makes a route written by 0.1.145 stale
/// the moment the app starts - on every paired provider at once. The panel then
/// showed "Update available" on each row and waited for the owner to press
/// Update, which is the app asking him to do a job it can do itself: it wrote
/// those routes, it can read them back, and it knows exactly what the current
/// one should say.
///
/// So it repairs its own routes, once per app start, in the background, under
/// these rules:
///
/// - only a provider that is already paired, because the entry is on disk;
/// - ownership is the managed identity plus the command name: the client id in
///   the entry's `env` must carry this provider's prefix and a version, and
///   the command's file name must equal this executable's file name
///   (case-insensitive; the directory is not compared). An entry that fails
///   either test is left exactly as it is, and a configuration that cannot be
///   read is not touched;
/// - an entry that passes is repaired by replacing the whole entry with the one
///   this build writes. Nothing inside the entry is protected: arguments or
///   environment keys somebody added are dropped, and a route switched off in
///   the entry is switched back on. Other servers in the host's configuration
///   are preserved, and the writer copies the file aside (a bounded history of
///   timestamped backups) before it replaces it;
/// - the repair runs when the entry names an older build, and also when it
///   names this build but differs from what this build would write, for
///   instance because it points at another path of the same executable name.
///   So a copy of the app started from a portable, development or second
///   install path repoints the managed routes at itself;
/// - an entry naming a newer build than this one is never downgraded, however
///   else it differs; the explicit Update control keeps that decision.
///
/// Absence of a running bridge is deliberately not a trigger. No host has one
/// after an app restart, so it proves nothing; this is keyed on the route's
/// content, which is a fact on disk.
///
/// Claude Desktop is not here, and that is deliberate: its pairing is a `.mcpb`
/// extension install that spawns Claude's own installer UI, which is a visible,
/// heavyweight action and must stay something the owner starts. The pasted
/// STDIO snippet for "another client" is not here either - that configuration
/// is the user's own file, not ours to find or to touch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteRefreshDecision {
    /// No `qc-remote` entry in this host's configuration. Nothing to freshen,
    /// and pairing is the owner's decision to make.
    NotPaired,
    /// The entry names this build and is exactly the entry this build would
    /// write.
    Current,
    /// The entry has no valid managed identity, or its command now names a
    /// different executable, so ownership cannot be established safely and the
    /// entry is left exactly as it is.
    Customised,
    /// The configuration could not be read or parsed, so this app cannot prove
    /// the entry is its own and does not touch the file.
    Unreadable,
    /// Ours, and naming a build newer than this one - the owner is running an
    /// older app than the one that paired. Rewriting would quietly downgrade
    /// the identity, so the explicit Update control keeps that decision. The
    /// version is judged before the rest of the entry, so this holds whatever
    /// else differs in it.
    PairedWithNewerBuild { paired: String },
    /// Ours, and either naming an older build, or naming this build but
    /// differing from the entry this build would write (another executable
    /// path, edited arguments or environment, a disabled flag). The whole
    /// entry is replaced. This is the only decision that rewrites.
    Stale { paired: String },
}

impl RouteRefreshDecision {
    /// The word a status document reports, and the only place these states are
    /// spelled out for a reader.
    fn outcome(&self) -> &'static str {
        match self {
            Self::NotPaired => "not-paired",
            Self::Current => "current",
            Self::Customised => "customised",
            Self::Unreadable => "unreadable",
            Self::PairedWithNewerBuild { .. } => "paired-with-newer-build",
            Self::Stale { .. } => "rewritten",
        }
    }

    /// The version the configured entry names itself by, where it has one.
    fn paired_version(&self) -> Option<&str> {
        match self {
            Self::PairedWithNewerBuild { paired } | Self::Stale { paired } => Some(paired),
            _ => None,
        }
    }
}

/// One managed server entry projected to plain JSON.
///
/// The ChatGPT route is TOML and the other three are JSON. Values emitted by
/// the route builders convert exactly, allowing one repair decision to cover
/// all four formats. Unsupported TOML value kinds convert to `null`.
fn toml_value_as_json(item: &toml_edit::Value) -> Value {
    match item {
        toml_edit::Value::String(text) => Value::String(text.value().clone()),
        toml_edit::Value::Integer(number) => json!(number.value()),
        toml_edit::Value::Boolean(flag) => Value::Bool(*flag.value()),
        toml_edit::Value::Array(array) => {
            Value::Array(array.iter().map(toml_value_as_json).collect())
        }
        toml_edit::Value::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, entry)| (key.to_string(), toml_value_as_json(entry)))
                .collect(),
        ),
        _ => Value::Null,
    }
}

fn toml_item_as_json(item: &Item) -> Value {
    match item {
        Item::Value(entry) => toml_value_as_json(entry),
        Item::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, entry)| (key.to_string(), toml_item_as_json(entry)))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// The client id a configured entry names itself by.
///
/// The environment value is the durable marker written by this app. Some
/// clients do not pass it through to the bridge process, so the argument
/// channel is kept for runtime identification but is not required here.
fn route_client_identity(server: &Value) -> Option<&str> {
    if server.get("http_headers_helper").is_some() {
        return server
            .get("http_headers")
            .and_then(|headers| headers.get(LOCAL_MCP_CLIENT_HEADER))
            .and_then(Value::as_str);
    }
    // Bionic nests the whole connection - transport, command, arguments and
    // environment - one level below the entry, so the identity is looked for
    // there as well. The map-shaped hosts carry no `connection` key at all, so
    // this cannot change what is read for any of them.
    [
        server.get("env"),
        server
            .get("connection")
            .and_then(|connection| connection.get("env")),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_object)
    .find_map(|environment| environment.get(LOCAL_MCP_CLIENT_ENV))
    .and_then(Value::as_str)
}

/// The command name is the second ownership check for an automatic repair.
/// The managed identity says QC Remote paired this entry; the executable name
/// makes sure a hand-edited entry that now launches another product is left
/// alone. Comparing only the basename allows the normal install path to move.
fn route_command_name(server: &Value) -> Option<&str> {
    let command = server
        .get("command")
        .or_else(|| {
            server
                .get("connection")
                .and_then(|value| value.get("command"))
        })?
        .as_str()?;
    command.rsplit(['\\', '/']).next()
}

/// Decide what to do about one configured entry, from the entry alone.
///
/// Pure, and taking the entry, this build's version and a rebuilder as
/// arguments, so every branch can be asserted without a host, a configuration
/// file or a clock.
fn classify_route_refresh(
    present: Option<&Value>,
    current_version: &str,
    client_prefix: &str,
    expected_for: &dyn Fn(&str) -> Value,
) -> RouteRefreshDecision {
    let Some(present) = present else {
        return RouteRefreshDecision::NotPaired;
    };
    // Direct HTTP pairing is explicit. Do not turn an existing HTTP route
    // back into stdio (or migrate a legacy route) as a startup side effect.
    if present.get("http_headers_helper").is_some() {
        return RouteRefreshDecision::Customised;
    }
    let Some(identity) = route_client_identity(present) else {
        return RouteRefreshDecision::Customised;
    };
    let Some(paired) = identity
        .strip_prefix(client_prefix)
        .filter(|version| !version.is_empty() && version_numbers(version).is_some())
    else {
        return RouteRefreshDecision::Customised;
    };
    // The identity marks an entry QC Remote previously installed. Require its
    // command to still name our executable before replacing the entry, but do
    // not treat edits inside that entry as a veto: startup repair is meant to
    // fix stale paths and restore the managed route. Other server entries in
    // the host configuration remain the writer's responsibility to preserve.
    let expected = expected_for(identity);
    if !route_command_name(present)
        .zip(route_command_name(&expected))
        .is_some_and(|(present, expected)| present.eq_ignore_ascii_case(expected))
    {
        return RouteRefreshDecision::Customised;
    }
    match compare_versions(current_version, paired) {
        Some(Ordering::Greater) => RouteRefreshDecision::Stale {
            paired: paired.to_string(),
        },
        Some(Ordering::Less) => RouteRefreshDecision::PairedWithNewerBuild {
            paired: paired.to_string(),
        },
        _ if expected != *present => RouteRefreshDecision::Stale {
            paired: paired.to_string(),
        },
        _ => RouteRefreshDecision::Current,
    }
}

/// Which file carries a provider's route, and how to read and rewrite it.
///
/// The four providers differ only in their file format and their identity, so
/// the pass carries that difference as data and runs one routine over it. A
/// target names explicit paths, which is what lets the whole pass be exercised
/// against temporary files.
struct ManagedRouteTarget {
    /// The key a status document reports this provider's outcome under.
    provider: &'static str,
    /// The provider's name as the writers' failure sentences spell it, which
    /// is also what a reported failure names.
    label: &'static str,
    config_path: PathBuf,
    executable_path: PathBuf,
    client_id: &'static str,
    client_prefix: &'static str,
    format: ManagedRouteFormat,
}

enum ManagedRouteFormat {
    /// Codex's `config.toml`.
    Toml,
    /// Antigravity's `mcp_config.json`.
    Antigravity,
    /// Pre-rename LM Studio's and Jan's `mcpServers` documents, which differ
    /// from each other only in whether the enabled flag is `active` or
    /// `disabled`.
    Json { uses_active_field: bool },
    /// Bionic's `ng-mcp.json`, whose `servers` is an **array**.
    ///
    /// The repair rule - the managed identity in the entry plus this app's
    /// executable name, then replace the whole entry - is unchanged by that,
    /// because the rule is about one entry rather than the document around it.
    /// What the array does change is that the entry has no key to be found
    /// under: it is located by the identity `bionic_entry_is_managed`
    /// declares, and everything beside it is untouched whatever the pass
    /// decides.
    Bionic,
}

const CHATGPT_ROUTE: &str = "chatgpt";
const ANTIGRAVITY_ROUTE: &str = "antigravity";
const LM_STUDIO_ROUTE: &str = "lmStudio";
const JAN_ROUTE: &str = "jan";

/// Every route this app rewrites unattended, resolved to the file that carries
/// it. Claude Desktop and the pasted STDIO snippet are absent by design; see
/// [`RouteRefreshDecision`].
fn managed_route_targets() -> Result<Vec<ManagedRouteTarget>, String> {
    // Every managed route runs this executable. Antigravity's has since
    // 0.1.143, and `antigravity_bridge_path` resolves to the same file. It is
    // the running copy, so whichever copy of the app is started becomes the
    // command the repaired routes name.
    let executable_path = std::env::current_exe()
        .map_err(|error| format!("Could not resolve the QC Remote executable: {error}"))?;
    let profile_unavailable = || "The user home directory is unavailable.".to_string();
    // The same selector Pair, Unpair and the status read come through, so the
    // pass freshens the file the row is reporting on. Resolved through the
    // other one, a perfectly good Bionic route would never be seen at all.
    let lm_studio = lm_studio_mcp_document(lm_studio_installation().as_ref())?;
    Ok(vec![
        ManagedRouteTarget {
            provider: CHATGPT_ROUTE,
            label: "ChatGPT",
            config_path: codex_config_path().ok_or_else(profile_unavailable)?,
            executable_path: executable_path.clone(),
            client_id: CODEX_CLIENT_ID,
            client_prefix: "codex:",
            format: ManagedRouteFormat::Toml,
        },
        ManagedRouteTarget {
            provider: ANTIGRAVITY_ROUTE,
            label: "Antigravity",
            config_path: antigravity_config_path().ok_or_else(profile_unavailable)?,
            executable_path: executable_path.clone(),
            client_id: ANTIGRAVITY_CLIENT_ID,
            client_prefix: "antigravity:",
            format: ManagedRouteFormat::Antigravity,
        },
        ManagedRouteTarget {
            provider: LM_STUDIO_ROUTE,
            label: LM_STUDIO_CONFIG_OWNER,
            config_path: lm_studio.path,
            executable_path: executable_path.clone(),
            client_id: LM_STUDIO_CLIENT_ID,
            client_prefix: LM_STUDIO_CLIENT_PREFIX,
            format: if lm_studio.bionic {
                ManagedRouteFormat::Bionic
            } else {
                ManagedRouteFormat::Json {
                    uses_active_field: false,
                }
            },
        },
        ManagedRouteTarget {
            provider: JAN_ROUTE,
            label: "Jan",
            config_path: jan_config_path().ok_or_else(|| {
                "The roaming application data directory is unavailable.".to_string()
            })?,
            executable_path,
            client_id: JAN_CLIENT_ID,
            client_prefix: "jan:",
            format: ManagedRouteFormat::Json {
                uses_active_field: true,
            },
        },
    ])
}

impl ManagedRouteTarget {
    /// The managed entry as the host's configuration carries it, projected to
    /// JSON. `Ok(None)` is an unpaired provider - including one whose
    /// configuration file does not exist at all, which is the ordinary state of
    /// a host that has never been paired.
    fn configured_route(&self) -> Result<Option<Value>, String> {
        if !self.config_path.is_file() {
            return Ok(None);
        }
        let contents = read_client_document_text(&self.config_path).map_err(|error| {
            format!("Could not read {}'s MCP configuration: {error}", self.label)
        })?;
        Ok(match self.format {
            ManagedRouteFormat::Toml => {
                let document = contents.parse::<DocumentMut>().map_err(|error| {
                    format!("{}'s MCP configuration is invalid: {error}", self.label)
                })?;
                document
                    .get("mcp_servers")
                    .and_then(Item::as_table)
                    .and_then(|servers| servers.get(CLAUDE_EXTENSION_NAME))
                    .map(toml_item_as_json)
            }
            ManagedRouteFormat::Antigravity | ManagedRouteFormat::Json { .. } => {
                parse_json_mcp_config(&contents, self.label)?
                    .get("mcpServers")
                    .and_then(Value::as_object)
                    .and_then(|servers| servers.get(CLAUDE_EXTENSION_NAME))
                    .cloned()
            }
            ManagedRouteFormat::Bionic => {
                bionic_managed_entry(&parse_json_mcp_config(&contents, self.label)?, self.label)?
                    .cloned()
            }
        })
    }

    /// What this app's own writer produces for `client_id`, in the same shape
    /// `configured_route` returns - so the comparison is like for like.
    ///
    /// `present` is the entry on disk, because for one format the writer's
    /// output depends on it: Bionic's entry carries an `id` the host minted,
    /// and [`write_bionic_mcp_route`] keeps it. Rebuilding with a different id
    /// would make this app's own freshly written route differ from what it
    /// would write, and it would be rewritten at every start. Every other
    /// format ignores it, as their writers do.
    fn expected_route(&self, client_id: &str, present: Option<&Value>) -> Value {
        match self.format {
            ManagedRouteFormat::Toml => {
                let mut route = toml_item_as_json(&Item::Table(codex_route_table(
                    &self.executable_path,
                    client_id,
                )));
                if let Some(policies) = present.and_then(Value::as_object) {
                    for (key, policy) in policies
                        .iter()
                        .filter(|(key, _)| !CODEX_TRANSPORT_FIELDS.contains(&key.as_str()))
                    {
                        route[key] = policy.clone();
                    }
                }
                route
            }
            ManagedRouteFormat::Antigravity => {
                antigravity_route_server(&self.executable_path, client_id)
            }
            ManagedRouteFormat::Json { uses_active_field } => {
                json_mcp_route_server(&self.executable_path, client_id, uses_active_field)
            }
            ManagedRouteFormat::Bionic => bionic_route_server(
                &self.executable_path,
                client_id,
                present.and_then(bionic_entry_id),
            ),
        }
    }

    /// Rewrites the managed entry through the same writer Pair and Update call.
    ///
    /// A failure here leaves the previous route working: each of these writers
    /// renders the whole document to a staged sibling file and renames it over
    /// the live one, so a refused write never reaches the live path and a crash
    /// mid-write can only leave the old contents or the new ones.
    fn rewrite(&self) -> Result<(), String> {
        match self.format {
            ManagedRouteFormat::Toml => {
                write_codex_config(&self.config_path, &self.executable_path, self.client_id)
            }
            ManagedRouteFormat::Antigravity => {
                write_antigravity_config(&self.config_path, &self.executable_path, self.client_id)
            }
            ManagedRouteFormat::Json { uses_active_field } => write_json_mcp_route(
                &self.config_path,
                &self.executable_path,
                self.client_id,
                self.label,
                uses_active_field,
            ),
            ManagedRouteFormat::Bionic => write_bionic_mcp_route(
                &self.config_path,
                &self.executable_path,
                self.client_id,
                self.label,
            ),
        }
    }
}

/// What the pass did about one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteRefreshOutcome {
    decision: RouteRefreshDecision,
    /// Set when a rewrite was attempted and did not land, or when the
    /// configuration could not be read. The previous route is still on disk and
    /// still working either way.
    failure: Option<String>,
}

impl RouteRefreshOutcome {
    fn report(&self) -> Value {
        json!({
            // "failed" is reserved for a rewrite that was attempted and
            // refused, because that is the state whose remedy differs: the
            // route is this app's own and needed repair, and something
            // stopped the write. A configuration this app could not read keeps
            // its own word - it never reached the point of attempting
            // anything - and both carry the reason in `error`.
            "outcome": match (&self.decision, &self.failure) {
                (RouteRefreshDecision::Stale { .. }, Some(_)) => "failed",
                _ => self.decision.outcome(),
            },
            "pairedVersion": self.decision.paired_version(),
            "availableVersion": env!("CARGO_PKG_VERSION"),
            "error": self.failure,
        })
    }
}

/// Reads one provider's route and replaces it when it carries this app's
/// managed identity and command name (see [`RouteRefreshDecision`]) and is
/// either at least one release behind or different from what this build would
/// write.
fn refresh_managed_route(target: &ManagedRouteTarget) -> RouteRefreshOutcome {
    // Held from the read through the rewrite: an Unpair that lands in between
    // must find the route already freshened, never have its removal written
    // back over.
    let _config_write = config_write_lock();
    let present = match target.configured_route() {
        Ok(present) => present,
        Err(error) => {
            return RouteRefreshOutcome {
                decision: RouteRefreshDecision::Unreadable,
                failure: Some(error),
            };
        }
    };
    let decision = classify_route_refresh(
        present.as_ref(),
        env!("CARGO_PKG_VERSION"),
        target.client_prefix,
        &|client_id| target.expected_route(client_id, present.as_ref()),
    );
    let failure = match decision {
        RouteRefreshDecision::Stale { .. } => target.rewrite().err(),
        _ => None,
    };
    RouteRefreshOutcome { decision, failure }
}

/// The pass's record, and the guard that keeps it to one run per app start.
#[derive(Default)]
struct RouteRefreshState {
    started: bool,
    outcomes: BTreeMap<&'static str, Value>,
    /// The last failure reported for each provider, which is what makes the
    /// stderr line one per reason rather than one per attempt.
    reported_failure: BTreeMap<&'static str, String>,
}

fn route_refresh_state() -> &'static Mutex<RouteRefreshState> {
    static STATE: OnceLock<Mutex<RouteRefreshState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RouteRefreshState::default()))
}

/// What the start-up pass decided about this provider, for its status document.
/// `null` until the pass has recorded an outcome for it.
fn route_refresh_report(provider: &str) -> Value {
    route_refresh_state()
        .lock()
        .ok()
        .and_then(|state| state.outcomes.get(provider).cloned())
        .unwrap_or(Value::Null)
}

/// Records one provider's outcome and reports a failure at most once per
/// distinct reason.
///
/// The reason is the rate limiter's key, so only a writer's static sentence and
/// the operating system's own error may appear in it - never a path carrying a
/// process id, never a timestamp, never an attempt count. See
/// [`should_report_failure`].
fn record_route_refresh(target: &ManagedRouteTarget, outcome: &RouteRefreshOutcome) -> bool {
    let provider = target.provider;
    let mut announce = None;
    if let Ok(mut state) = route_refresh_state().lock() {
        state.outcomes.insert(provider, outcome.report());
        match outcome.failure.as_deref() {
            Some(failure) => {
                let reported = state.reported_failure.get(provider).map(String::as_str);
                if should_report_failure(reported, failure) {
                    state.reported_failure.insert(provider, failure.to_string());
                    announce = Some(failure.to_string());
                }
            }
            // A provider that is working again drops its remembered reason, so
            // the same failure later is announced afresh rather than swallowed.
            None => {
                state.reported_failure.remove(provider);
            }
        }
    }
    match announce {
        Some(failure) => {
            eprintln!(
                "QC Remote could not bring its own {} connection up to date: {failure}",
                target.label
            );
            true
        }
        None => false,
    }
}

/// Reads and freshens every managed route, in order, on the calling thread.
fn refresh_managed_routes_blocking() -> Vec<(&'static str, RouteRefreshOutcome)> {
    let targets = match managed_route_targets() {
        Ok(targets) => targets,
        Err(error) => {
            eprintln!("QC Remote could not check its own connections: {error}");
            return Vec::new();
        }
    };
    targets
        .iter()
        .map(|target| {
            let outcome = refresh_managed_route(target);
            // The answer - whether this outcome earned a line on stderr - is
            // what the once-per-reason rule is asserted on; the pass itself
            // has nothing further to do with it.
            record_route_refresh(target, &outcome);
            (target.provider, outcome)
        })
        .collect()
}

/// Repairs every recognizable QC Remote route to this build, once per app start.
///
/// Off the start-up path entirely: the work runs on its own detached thread,
/// owned by the shared original-thread family. A slow file does not block
/// the async UI runtime. Pair/Unpair share CONFIG_WRITE and may still wait
/// behind filesystem I/O; no hard OS-I/O cancellation guarantee is implied.
///
/// One run per app start, enforced here rather than by the caller, so a second
/// call returns immediately. The pass itself is sequential, so there is never
/// more than one attempt in flight for a provider, and it is not a retry loop:
/// a provider that fails is reported and left until the next app start or an
/// explicit Update.
pub fn refresh_managed_routes_on_start() {
    match route_refresh_state().lock() {
        Ok(mut state) if !state.started => state.started = true,
        _ => return,
    }
    // One original is retained process-wide, including filesystem stalls and
    // native TLS after the body returns. No GUI shutdown hard-deadline claim.
    static REFRESH_BUDGET: qc_native_platform::thread_owner::Budget =
        qc_native_platform::thread_owner::Budget::new(1);
    let original = qc_native_platform::thread_owner::Owned::spawn(
        "qc-route-refresh",
        &REFRESH_BUDGET,
        (),
        || {
            let outcomes = refresh_managed_routes_blocking();
            // Antigravity is the one host that can be told, and it is worth
            // telling. ChatGPT/Codex spawns its MCP servers per chat and does
            // not respawn on a configuration change, and LM Studio and Jan
            // pick the file up on their own schedule, so for those
            // there is nothing to call. Here a running Antigravity would
            // otherwise keep serving the bridge the old entry named for the
            // rest of its session: its heartbeat would keep arriving under the
            // old client id and the row this rewrite was meant to clear would
            // stay exactly as it was. The call is gated on having actually
            // rewritten that route, so a start-up with a current route costs
            // nothing; it is best effort and reports into its own value rather
            // than failing the pass; and it asks for one server by name, so no
            // other MCP server in that host is disturbed.
            let rewrote_antigravity = outcomes.iter().any(|(provider, outcome)| {
                *provider == ANTIGRAVITY_ROUTE
                    && outcome.failure.is_none()
                    && matches!(outcome.decision, RouteRefreshDecision::Stale { .. })
            });
            if rewrote_antigravity {
                let refresh = tauri::async_runtime::block_on(refresh_running_antigravity_mcp());
                if let Ok(mut state) = route_refresh_state().lock() {
                    if let Some(report) = state.outcomes.get_mut(ANTIGRAVITY_ROUTE) {
                        merge_object(report, refresh);
                    }
                }
            }
        },
    );
    match original {
        Ok(original) => drop(original), // retained by the shared original reaper
        Err(error) => {
            // Explicit Update remains available after failed startup admission.
            eprintln!("QC Remote could not check its own connections: {error}");
        }
    }
}

#[tauri::command]
pub fn save_local_client_package() -> Result<Value, String> {
    let file_name = format!("QC-Remote-Local-Clients-{}.zip", env!("CARGO_PKG_VERSION"));
    save_package(
        "QC Remote Local Clients",
        "zip",
        file_name,
        local_client_archive()?,
    )
}

#[cfg(test)]
#[path = "client_packages_tests.rs"]
mod tests;
