# macOS distribution

QC Remote has separate direct-download and Mac App Store distribution paths.
The current public macOS release is a developer preview; do not label a signed
but unnotarized candidate as a notarized production release.

## Direct download

Use the verified ARM64 and Intel CI assets from the same release commit. Check
SHA256SUMS before extracting. The app embeds QC Device Broker.app; include a
standalone copy in the final disk image so the broker can run without the editor.
Do not include personal settings, credentials, logs, provisioning private keys,
or local acceptance captures in public packages.

Install a Developer ID Application identity/private key for the publishing team.
Apple Distribution is for the App Store and cannot replace Developer ID here.
Xcode can also upload a Developer ID archive for notarization using an already
configured Apple account (`xcodebuild -exportArchive`, destination `upload`),
then export it only after Apple approves it with `-exportNotarizedApp`. Keep
separate archive upload records for each architecture. Successful upload does
not mean notarization approval.

Configure notarization authentication securely; do not put passwords or API keys
in the repository, command history, release metadata, or chat.

```sh
python3 scripts/package-macos-direct.py \
  --app '/path/to/QC Remote.app' \
  --broker '/path/to/QC Device Broker.app' \
  --identity 'Developer ID Application: Publisher (TEAM_ID)' \
  --notary-profile 'existing-notarytool-profile' \
  --source-commit FULL_VERIFIED_SOURCE_SHA \
  --output /new/release/output
```

The script signs nested code inside-out with hardened runtime and timestamps,
verifies both bundles, creates/signs the disk image, submits it to Apple's
notary service, and requires Accepted status before stapling and validating it.
It checks the image and Gatekeeper assessment and produces checksums and a
provenance manifest. It never publishes or changes installed applications.
Omitting --notary-profile prepares a clearly named signed-unnotarized candidate
only; this is not a completed public release.

Before publishing, smoke-test a quarantined, notarized installation on supported
macOS hardware: editor, standalone broker, USB ownership, broker lifecycle,
remote pairing/reconnect and local MCP integration. Test Intel natively if that
architecture is offered. Preserve the unsigned preview's known limitations;
signing is not evidence that hardware or deadline behavior has been fixed.
Publish only final, verified artifacts and their checksums to the owned release
and update the download page to describe the exact architecture and version.

## Mac App Store engineering gates

The existing direct-download bundle is not an App Store-ready sandboxed app.
Adding an entitlement plist alone is not sufficient. The following require
implementation and acceptance before creating a submission candidate:

- Register the exact app identifier org.freevia.qcremote and provision it for
  Mac App Store Connect under the publishing team. A profile for another app
  cannot be reused. Use an Apple Distribution application signature and a Mac
  Installer Distribution signature for the uploaded package.
- Enable App Sandbox and verify USB access plus outbound and loopback network
  capabilities actually required by the broker and editor.
- Resolve editor/standalone-broker sandbox identities and shared state. Current
  broker descriptors, credentials, settings, and workspace paths assume the
  direct-download user's Application Support directory. A Store build needs
  a signed and entitled shared-container design and a deliberate migration
  policy that preserves pairing, access levels, and the credential boundary.
- Replace direct LaunchAgent/background registration with a supported sandbox
  and login-item design. Test broker-only launch, editor close, explicit Quit,
  restarts, upgrades, and rollback without duplicate USB ownership.
- Local AI pairing writes managed entries and skills into other applications'
  profile directories. Define and implement an App Store-compatible user-driven
  flow; do not request broad filesystem exceptions merely to make submission
  validation pass, or silently remove this functionality.
- Audit document/workspace import/export and external process launches in the
  sandbox. Preserve explicit user selection and fail with useful diagnostics.
- Prepare the app record, screenshots from the actual candidate, support and
  privacy URLs, review notes (including hardware access/demo instructions),
  pricing/territories, and substantiated privacy, age, and export answers.
  Do not invent legal or privacy attestations.

Apple review approval and public availability are separate from successful
upload or local signature verification. Do not claim publication until the
specific product is actually available through its declared channel.

References:
- https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution
- https://developer.apple.com/help/account/certificates/create-developer-id-certificates/
- https://v2.tauri.app/distribute/app-store/

### App Store port in progress

The opt-in `macos-app-store` Cargo feature now resolves the GUI/broker state
directory through Foundation's App Group container API. Its macOS-only group is
`8C782Q826S.org.freevia.qcremote`; all participating executables must be signed
by that team and carry the matching application-groups entitlement. State lives
in a private `state` leaf. A missing group container is a startup error rather
than a silent fallback to separate app-private broker state. The direct build
retains its existing storage path and does not enable this feature.

The Store runtime and packaging are implemented behind this feature, with a
separately signed development candidate. Initial signed sandbox probes verify
shared-container writes, denied writes outside the container, and native MCP
initialization/tool discovery. Full GUI, login lifecycle, LOCAL pairing,
USB/network/file acceptance and review metadata remain gates to submission.
The installed direct-download app and its credentials are not migrated by this
work. Unit tests and these focused probes alone are not full E2E acceptance.

### Store candidate build and signing

Run `scripts/build-macos-appstore.sh` on the target Mac after installing the
normal build prerequisites. It builds both native binaries with the Store
feature, builds the helpers, and bundles the Store resources. The independent
menu is nested in `Contents/Helpers`; the login launcher is nested in
`Contents/Library/LoginItems`, following Apple's code-content locations. Tauri merges the
platform configuration automatically. The Store configuration replaces the
resource map with an empty array and uses explicit `macOS.files` destinations
for helpers and legal resources, preventing retained direct-helper inputs. Both packaging scripts
reject stale or wrong-flavor binary markers. The Store configuration sets macOS
13 minimum and a separate monotonically increasing `bundleVersion` (currently
197); increase it for each subsequent App Store upload. Helpers use that same
build number.

`package-macos-appstore.py` signs the editor and independent broker menu with
App Sandbox and the shared group. Broker child executables receive only sandbox
inheritance; the short-lived login launcher receives its own sandbox. It opens
the separate menu app and exits successfully, so unregistering the login item
does not terminate the long-lived broker. No LaunchAgents files are written in
the Store path.

Use `--development --identity 'Developer ID Application: ...'` to produce a
locally executable sandbox test candidate. Production signing uses
`--identity 'Apple Distribution: ...' --profile /path/to/store.provisionprofile
--installer-identity '3rd Party Mac Developer Installer: ...'`. The production
profile is embedded only in submission mode. A Store production profile is not
a local development execution entitlement; validate runtime behavior using the
development candidate before uploading the production package.

LOCAL Pair/Unpair requests an exact host configuration folder through the
standard macOS folder picker. Security-scoped bookmarks restore that access
without showing a dialog from polling or automatic status reads. Claude's Store
extension delegates STDIO to the installed, signed native bridge; its Node
process never reads protected App Group state. Generated packages and displaced
skill backups use the shared private state directory. Runtime acceptance of
these paths, not compilation alone, is required before App Store submission.

The Store broker verifies local TCP ownership without inspecting another
process's file descriptors, which App Sandbox prohibits. A private App Group
Unix socket supplies a kernel-authenticated peer PID and asks that process to
confirm its own established TCP connection's exact loopback port pair. The
client checks the expected PID and process creation time before sending the
existing broker credential on the original TCP connection. Missing or failed
proofs fail closed; the proof channel carries no credential. The default direct
build retains its existing listener ownership check.

### Shared Codex HTTP sessions

Explicit Codex Pair now uses one native HTTP audience in the existing host;
logical sessions no longer require a persistent STDIO process per chat. Existing
STDIO routes are preserved until explicit Pair. The credential helper is the
native editor executable with `--local-mcp-http-headers`, returns bounded header
JSON, and exits without starting a host. Its shell command quotes the absolute
path as data. Direct builds use private Application Support files; Store builds
use the existing Foundation App Group path. No Keychain storage is introduced.
The independent broker's Unix connection witness remains unchanged.

BSD admission keeps `SO_REUSEPORT` disabled. `SO_REUSEADDR` permits restarting
across retired TCP tuples, while a live listener on the exact loopback address
still refuses a second listener, including contenders using reuse flags.
Windows retains its own `SO_EXCLUSIVEADDRUSE` implementation. Neither platform
silently reassigns the saved port or kills a conflicting process.

Validation on macOS: 14 focused HTTP tests and 122 pairing tests pass in both
direct and Store feature builds; both Store native binaries compile. The shared
MCP crate's 33 server tests and remaining integration suites pass. An isolated
installed Codex 0.163.0-alpha.11.1 test uses the complete production route builder
and a native mock helper under shell-metacharacter/Unicode paths. It verifies
both headers, a completed mock write whose response is dropped, stale-session
reinitialization without repeating that write, and a positive new write only
after explicit status. All owned children retire; no model inference or physical
hardware is involved. This does not qualify the installed/signed Store app's
runtime lifecycle or authorize changing a live Codex profile.

The plaintext HTTP cached-header port-takeover limitation in USER-GUIDE still
applies on macOS. A helper proof protects new credential issuance, not every
later connection made with cached credentials. Review that limitation before
activating this transport; source adaptation does not migrate a live pairing.
