# QC Remote for iOS

QC Remote targets two direct USB modes on Apple mobile devices:

- **iPhone: USB-MIDI control.** Core MIDI discovers the QC's USB MIDI endpoint and sends documented preset, scene, footswitch, expression pedal, mode, tap-tempo, tuner, Gig View, Looper X, and duplicate-PC setting commands. The MIDI input channel is configurable and remembered on the device. The connection is one-way; it does not provide live state, full preset editing, backups, or CorOS updates. The iPhone interface displays this limitation before connection.
- **iPad: direct USB DriverKit prototype.** Supported M-series iPads on iPadOS 16 or later can discover the QC DriverKit service, ingest reports into the shared protocol session, issue correlated reads, and dispatch guarded gateway writes through the shared delivery journal. Writes resolve as verified when fresh QC state matches, or `accepted_unverified` when the confirmation window expires. Apple also documents that DriverKit does not support USB devices that manipulate audio; because QC combines control HID with USB audio, full direct control remains an experimental prototype pending Apple's compatibility and entitlement approval plus physical QC validation.

An iOS configuration profile cannot add a HID driver or grant an app raw vendor-HID access. The supported iPad path is a signed DriverKit extension. iPhone does not support this custom USBDriverKit path, so iPhone uses the limited MIDI interface.

The native iOS bridge owns Keychain credentials, app lifecycle, display behavior, Core MIDI access, and iOS packaging. iPhone direct USB-MIDI is implemented and clearly disclosed as MIDI-only. The iPad service routes gateway reads and writes to the local DriverKit protocol session when the driver is available, with shared Rust planning, correlation, report delivery accounting, and pushed-state verification. Save-as, rename, copy, move, delete, setlist creation/deletion/duplication, and direct device backup export use the shared guarded Rust workflows over DriverKit. Backups are written as JSON under `Documents/Quad Cortex Backups` and appear in Files; after export, the USB protocol session reconnects and synchronizes before the request completes. An uncertain backup transfer quarantines further writes until reconnect. A failed or uncertain persistent mutation also quarantines further writes until reconnect and synchronization. Capability reporting probes whether the bundled driver user client is actually available to the app. Apple entitlement and compatibility approval, an Xcode build, and physical QC testing remain release requirements.

## Build

```sh
npm install
npm run ios:add --workspace @qc-remote/ios
npm run ios:sync --workspace @qc-remote/ios
npm run ios:open --workspace @qc-remote/ios
```

Building and running the native app requires macOS, Xcode, Rust via rustup, and the Rust target for the selected SDK (`aarch64-apple-ios` for devices, `aarch64-apple-ios-sim` for Apple Silicon simulators, or `x86_64-apple-ios` for Intel simulators). The `iOS build` GitHub Actions workflow compiles the web app, native app, Rust bridge, and DriverKit target without signing, then uploads the unsigned `.app` as a workflow artifact. An installable iPad build still needs Apple-approved DriverKit entitlements and provisioning. From Xcode, select an iPhone simulator or a connected device and run the `App` scheme.

### Paired-host adapter validation

`npm run test:mcp-adapter --workspace @qc-remote/core` verifies every generated app method against the MCP projection, including access policy, confirmations, result decoding and captured preset guards. It also runs through `npm run contracts:check`. The adapter uses direct, on-demand cloud reads; it adds no idle polling. Device identity requires both the dedicated cloud read tool and a host admitting `device.identity`.

The opt-in XCTest suite in `tests/UI` checks saved sign-in after cold launches, read-only model browsing and reversible UP/DOWN navigation. Set `TEST_RUNNER_QC_EXPECTED_PRESET` and `TEST_RUNNER_QC_EXPECTED_PREVIOUS_PRESET` only against a known, clean live preset. It refreshes host state explicitly and never saves a preset or changes persistent settings.
