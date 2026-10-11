# Desktop Quad Cortex HID adapter

`quadcortex-hid` is the one desktop HID engine used by the shared device broker.
`quadcortex-windows` retains its original public API as a compatibility facade.
The `quadcortex` library continues to own protocol, framing, session, admission,
delivery, state and verification decisions. There is no second Linux session
engine or broker.

The native boundary contains only handle opening, concurrent report I/O and the
OS-specific completion policy:

- Windows retains its `windows-native` hidapi backend, one handle with independent
  RX/TX lanes, and the measured `ERROR_GEN_FAILURE` (31) status-stage exception.
- Linux uses hidapi's `linux-native-basic-udev` backend: udev/sysfs discovery and
  hidraw I/O, without a link-time libudev dependency. Its measured Linux
  control-status `EPIPE` exception is limited to VID/PID `152a:880a`, interface
  5, and that interface's exact 32-byte report descriptor. Every other Linux
  profile still requires a complete-length write; short writes and other errors
  fail closed. Windows status codes are not reused on Linux.

Both backends pin hidapi **exactly to 2.6.7**. Their private full-duplex wrappers
have separate safety audits in `src/backend/`; re-audit both before upgrading.
The upstream source corresponding to that release is
[`8729463`](https://github.com/ruabmbua/hidapi-rs/blob/8729463f8a96b6d4671b7ef114b5ead65ccd7469/src/linux_native.rs).
The Linux methods used here access only the immutable file descriptor and their
own buffers, never the backend's `Cell`/`RefCell` fields. The shared adapter uses
one RX thread and one TX thread and exposes no other methods on that handle.

Linux writes retain the report-ID byte, including zero for an unnumbered report,
as required by the [kernel hidraw API](https://www.kernel.org/doc/html/latest/hid/hidraw.html).
Inbound normalization continues through the shared transport runtime. A native
Linux hardware probe observed one full 129-byte type-52 `ResetCommsBuffers`
report followed by `EPIPE`, then a type-52 reply echoing the request's fresh
session UUID; that reply omitted the optional request ID. This demonstrates the
specific Linux `SET_REPORT` status-stage behavior for the descriptor above. The
shared protocol layer still requires the exact fresh session UUID before the
handshake completes. This single exchange does not establish full startup,
reconnect, or general Linux USB conformance.

Linux device selection uses vendor/product and an advertised serial, so a
different unit cannot inherit a reused `/dev/hidrawN` path. If no serial is
advertised, the node's inode and change time fence that identity instead;
such devices require explicit reselection after unplug/replug. Identity is
checked again after opening and before any session command.

Linux's advisory `flock` on the hidraw inode is held beside the HID handle in the
same `Arc`. A failed open releases it, and a reconnect cannot replace a retained
writer until the writer has actually released its handle. Other applications
that ignore `flock` can still open hidraw; close competing QC applications.

The RX poll is bounded to 200 ms. The shared queue stays bounded and abort drops
unsent fragments. Linux failure teardown also bounds the writer join to two
seconds. A write already inside the kernel cannot be safely cancelled by this
API: its thread and ownership lock remain alive until the syscall returns or the
process exits. A detached writer never gives its handle to a new session. Tests
exercise this failure path and release their simulated blocked writer before
finishing.

Linux flight-recorder persistence is disabled by default. An operator may set
`QC_FLIGHT_RECORDER_PATH` to an explicit diagnostics path; the recorder remains
bounded and payload-free. Broker authentication descriptors use the broker's
separate protected runtime storage.
