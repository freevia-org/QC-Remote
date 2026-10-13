# iOS shared protocol bridge

`quadcortex-ios-ffi` exposes a small C ABI over the shared Rust QC protocol and
session implementation. It is intended for Swift code in the iPad host app and
does not own USB. A platform driver remains responsible for opening the QC HID
interface and moving complete HID reports.

The interface covers session creation, report framing and reduction, the
initial reset-comms handshake, guarded gateway read and write planning,
correlated response projection, basic shared command generation, and status
projection. Gateway write plans carry encoded messages, pacing, verification
policy, a session-bound dispatch ticket, and the serialized verification
context. The ABI also exposes the shared delivery journal's prepare, admit,
begin, finish, stop, and operation completion transitions, the shared
post-write verification lifecycle, and retained preset/setlist workflow
preparation and advancement. The Swift adapter runs those workflow actions on
the DriverKit queue while keeping report ingestion responsive. The iPad app
must not advertise a release-ready full USB mode pending Apple approval and
hardware validation.

The Xcode app target builds this crate for its active Apple SDK into DerivedData
using `apps/ios/tools/build-ios-rust-ffi.sh` and force-links the static library.
`QCIosProtocolSession.swift` wraps the session, ingress, handshake, startup
batch, gateway write planning, report framing, and delivery journal APIs for
native Swift callers. Access to each session is serialized.
The Swift adapter begins with an 8 KiB JSON buffer and retries a short result
using the required size; Rust retains that result until the retry.

This bridge is not evidence that the DriverKit extension builds or that Apple
has granted the required USB and driver communication entitlements. Those remain
separate release requirements documented in `../../driverkit/README.md`.
