# iPad USB DriverKit prototype

`App.xcodeproj` now contains a DriverKit target and embeds its DEXT in the iOS
app target. The extension source claims the QC device, locates its HID
interface, and exposes report read/write and diagnostics through a user client.
It prefers the measured Quad Cortex control function at interface 5; its
fallback requires an interrupt-IN endpoint sized for the complete QC report.
The Capacitor plugin now opens the user client, reads reports on a dedicated
queue, feeds complete frames to the shared Rust session, sends the reset-comms
handshake and startup batches, and publishes decoded state frames to native
listeners. When the DEXT is available, the iPad gateway service uses local Rust
planning for correlated reads and guarded writes, with per-report delivery
receipts. Gateway writes with a shared verification plan wait for fresh QC
state and return verified when it matches, or `accepted_unverified` when the
confirmation window expires. Raw report writes and realtime operations remain
unverified. MIDI channel is configurable and persisted for planner-routed USB
MIDI actions. Preset and setlist mutations use the shared guarded multi-stage
workflow; uncertain writes quarantine further writes until reconnect and sync. The project has
not been built with Xcode or run against hardware.

## What “HID profile” means here

There is no app-installable iOS configuration profile that adds raw access to a
vendor HID interface. `Info.plist` is the DriverKit extension's matching
personality: it tells iPadOS which USB device can start this driver. The two
personalities below already match the Quad Cortex and QC Mini by vendor and
product ID. They do not install a driver or grant USB access by themselves.

Apple DTS confirms that a custom USB HID DEXT on iPadOS must match the device,
then use USBDriverKit to inspect and claim the HID interface. Matching only the
interface can start beside Apple's HID driver, leaving the custom extension
unable to open it. This device-level match is why the personality must not be
changed to an interface-only HID match. See [Apple DTS guidance on custom HID
over USB on iPad](https://developer.apple.com/forums/thread/789712).

This route is available on supported iPads, not iPhones. Apple documents
USBDriverKit on iPadOS for M-series devices. See [USBDriverKit platform
support](https://developer.apple.com/documentation/usbdriverkit).

## Device match

The Android USB host profile lists QC VID `5418` (`0x152A`) and product IDs
`34826` (`0x880A`, Quad Cortex) and `35119` (`0x892F`, QC Mini). The personalities
in `Info.plist` match at `IOUSBHostDevice` level. Apple DTS specifically advises
matching custom USB HID drivers at the device level so the driver can own the
device, then inspecting the configuration and claiming its HID interface. The
driver must select the interface by descriptors and use its interrupt endpoint;
it must not assume Android's interface number is universal.

Apple says DriverKit doesn't support USB devices that manipulate audio. The QC
is a USB audio interface as well as exposing control HID, so this is a material
compatibility constraint, not just an untested side effect. A prototype may
claim only the control path, but the QC's full-device match could interfere
with Apple's audio/MIDI drivers and Apple may reject this device/function
combination. Do not ship or advertise full iPad control until Apple confirms
the entitlement and the DEXT is validated with QC audio, MIDI, and HID
simultaneously. See [Apple's DriverKit guidance](https://developer.apple.com/documentation/driverkit/creating-a-driver-using-the-driverkit-sdk).
iPhone MIDI remains on Core MIDI and does not use this DEXT.

## Signing

- `QCRemoteUSBDriver.entitlements` requests DriverKit plus the two QC USB
  products. Apple's developer portal must grant the USB transport entitlement
  to the signing team and the provisioning profile must contain the granted
  values.
- The App target now points `CODE_SIGN_ENTITLEMENTS` at
  `QCIosApp.entitlements`, which requests the iPadOS Communicates with Drivers
  entitlement for the host app. The developer team still needs Apple's
  entitlement grant and a matching provisioning profile.
- The extension bundle identifier should be
  `org.freevia.qcremote.driver`; the app remains `org.freevia.qcremote`.
- The app user enables the DEXT under Settings > Privacy & Security > Drivers.

The current project points the driver target at `Info.plist` and
`QCRemoteUSBDriver.entitlements`, uses the bundle identifier above, and embeds
the `.dext`. The app target uses `QCIosApp.entitlements`. Confirm these settings
against Xcode's DriverKit template and ensure they survive `cap sync`; the
Capacitor CLI owns and rewrites its generated project files.

## App-to-driver contract

The driver user client needs a bounded report API:

1. Open and close the QC HID session.
2. Query the selected interface, endpoint addresses, packet size, and report
   layout for connection diagnostics.
3. Submit one complete HID output report with a completion result.
4. Deliver completed interrupt-IN reports asynchronously to the app, including
   disconnect and terminal transfer errors.
5. Abort the outstanding interrupt-IN transfer before closing the user client;
   interrupt reads have no timeout, so teardown must explicitly cancel them.

The iOS C ABI and Swift adapter now use the shared `TransportRuntime`,
`SessionRuntime`, and state decoder. iPad gateway reads use shared read
planning and response correlation. Guarded writes use the shared write planner,
report delivery journal, and shared post-write state verifier; the verifier
returns `accepted_unverified` if its confirmation window expires without a
matching QC state update. Direct backup export uses the shared Rust chunk
assembler, limits the saved JSON to 32 MiB, writes it under
`Documents/Quad Cortex Backups`, and reconnects/resynchronizes the protocol
session before resolving the app request. The app exposes Documents through
Files. `fullUsbDriverBundled`
reports whether the DEXT is embedded in the app,
and `fullUsbReady` probes whether the app can open its user client on the
current supported iPad. Runtime availability does not replace Apple's
compatibility approval or physical device validation.

## External prerequisites

Apple requires a DriverKit entitlement request for the hardware vendor ID and
driver function, then provisions the approved entitlements to the developer
team. The device maker owns VID `0x152A`; approval for a third-party controller
must be confirmed with Apple before distribution. The extension also needs a
real Mac/Xcode build and validation on an M-series iPadOS 16+ device with a QC
or QC Mini connected. A Windows build cannot validate DEXT source, signing, or
hardware matching.
