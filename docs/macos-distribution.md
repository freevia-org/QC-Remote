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
  an explicitly provisioned shared-container design and a deliberate migration
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
