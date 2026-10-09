The vocabulary the rest of the guide uses, and a map of which module owns which
job. Skim it once; every later section links back here.

## Three things that move: reports, frames, messages

A message to or from the unit passes through three shapes. Each is a separate
layer in the library.

| Shape | What it is | Where it lives |
| --- | --- | --- |
| **Report** | One fixed-size HID report: 129 bytes, a report-ID byte then a 128-byte body. A message too large for one report is split across several. | [`protocol::framing`](crate::protocol::framing) |
| **Frame** (envelope) | One logical message as reassembled from reports: the payload followed by an 8-byte trailer holding the message type, an *encrypted* flag, a *compressed* flag and two further bytes. | [`framing::Frame`](crate::protocol::framing::Frame) |
| **Message** | A message *type* (a small integer, listed in [`message_registry::MESSAGE_TYPES`](crate::protocol::message_registry::MESSAGE_TYPES)) and a protobuf *payload*. | [`commands::OutboundMessage`](crate::commands::OutboundMessage), [`proto`](crate::protocol::proto) |

The report layout, as `framing` encodes and parses it:

```text
byte   0      report ID: 0x02 host -> device, 0x01 device -> host
bytes  1..=2  header, little-endian u16
                bits 0..=13  number of data bytes in this report (at most 126)
                bit  14      FIRST: first report of a logical message
                bit  15      LAST:  last report of a logical message
bytes  3..    data, then zero padding up to byte 128

logical message = payload || trailer
trailer (8 bytes) = message type (u32, little-endian)
                  | encrypted flag (1 byte) | compressed flag (1 byte)
                  | two device bytes
```

Three details that surprise people:

- The trailer's message type is four bytes on the wire. The declared types all
  fit in one byte today, but the codec keeps the full 32-bit value
  ([`Frame::message_type`](crate::protocol::framing::Frame::message_type)), and
  APIs that return a `u16` fail with
  [`FrameError::MessageTypeOutOfRange`](crate::protocol::framing::FrameError::MessageTypeOutOfRange)
  rather than truncating.
- The two *device bytes* are preserved as received and never interpreted. This
  crate writes zero in both. Their meaning is **not established here**; host
  software is *believed* to ignore them, which has not been verified. See
  [`Frame::device_bytes`](crate::protocol::framing::Frame::device_bytes) and
  [`inbound_telemetry`](crate::inbound_telemetry), which tallies what the device
  puts there.
- A payload whose *compressed* flag is set is a gzip stream. The inflation limit
  is per message type, so a small control message cannot expand to catalogue
  size ([`compression`](crate::protocol::compression)).

## Request ids and correlation

Most requests carry a `u64` **request id**, and many replies echo it. Two facts
matter:

- The id is protobuf field 2 in almost every message, but field 1 in a fixed set
  of types, listed in
  [`wire::REQUEST_ID_FIELD_ONE_TYPES`](crate::protocol::wire::REQUEST_ID_FIELD_ONE_TYPES).
  [`wire::request_id`](crate::protocol::wire::request_id) reads the right one.
- Not every reply carries an id. Version replies, for example, legitimately echo
  none, so a caller falls back to matching on message type alone.
  [`ResponseExpectation`](crate::device::correlation::ResponseExpectation) holds
  that rule: the type must match, and if you asked for an id, the reply must
  carry exactly that id.

The unit also *pushes* state without being asked (a scene change, a recalled
preset). Pushes are not correlated with anything; they are folded into state
(see [section 4](crate::guide::read_state)).

## Sessions, epochs and tickets

A **session** is one handshake with the unit, identified by an opaque session id
your host supplies fresh for every physical open. The library binds work to the
session it was started in so that a late callback from an old session cannot
touch a new one:

- [`SessionEpoch`](crate::session::SessionEpoch): session id plus a revision
  that advances on each in-session rebuild.
- [`SessionFence`](crate::device::owner::SessionFence): the native connection
  number plus the epoch.
- Tickets ([`ObservationTicket`](crate::device::owner::ObservationTicket),
  [`OperationTicket`](crate::device::owner::OperationTicket),
  [`LifecycleTicket`](crate::session::LifecycleTicket),
  [`DeliveryTicket`](crate::device::delivery::DeliveryTicket)) are issued under a
  fence, and the owner rejects any ticket whose fence is no longer current.

If you only decode captures or build frames you never meet these. They are what
makes a long-running host safe against reconnects.

## The layer map

```text
        your application: threads, UI, files, the monotonic clock
  +---------------------------------------------------------------------+
  | HOST ADAPTER: opens USB/HID, writes and reads 129-byte reports.     |
  | Not in this crate; see "Transports".                                |
  +----------------^---------------------------------+------------------+
        reports out |                                 | reports in
  ==================|=================================|================== quadcortex
                    |                                 v
  device::owner::SessionRuntime   one value per device, behind your lock
   |  plan_operation, prepare_dispatch, prepare_delivery  (tickets, journal)
   |  push_native_report, observe_lifecycle, observe_state
   |
   +-- transport::TransportRuntime   framing, handshake pacing, keepalive
   +-- session::SessionDriver        startup: reset, version, build, boot, seed
   +-- device::state_runtime::DeviceStateRuntime
   |        +-- state::StateDecoder  messages -> StateUpdate, GatewaySnapshot
   +-- suspension gate, backup entry, delivery journal

  device::request   plan_gateway_read / plan_gateway_write / plan_preset_mutation
                    guards, verification predicates, deadlines
  commands          intent -> OutboundMessage { message_type, payload }
  protocol          framing, wire (request ids), responses, message_registry,
                    compression, proto (generated), profile (constants)
  backup            BackupRuntime: one LocalBackup transfer
```

You can stop at any layer. Each one works without the ones above it:

| You want to | Stop at | Section |
| --- | --- | --- |
| Frame and parse bytes, decode a capture | `protocol` | [3](crate::guide::offline_quick_start) |
| Build the message for an action | `commands` | [3](crate::guide::offline_quick_start), [5](crate::guide::send_commands) |
| Keep a model of the unit's state | `state`, `device::state_runtime` | [4](crate::guide::read_state) |
| Drive a connection with correct pacing | `transport`, `session` | [6](crate::guide::sessions) |
| Validate writes against observed state | `device::request` | [5](crate::guide::send_commands) |
| Run a long-lived host | `device::owner` | [6](crate::guide::sessions), [8](crate::guide::transports) |
| Export a device backup | `backup` | [7](crate::guide::backups) |

## Where each domain starts

| Domain module | Start with | What it answers |
| --- | --- | --- |
| [`protocol`](crate::protocol) | [`FrameAssembler`](crate::protocol::framing::FrameAssembler), [`decode_any`](crate::protocol::message_registry::decode_any) | What are these bytes? |
| [`commands`](crate::commands) | [`DeviceCommand`](crate::commands::DeviceCommand), [`DeviceOperation`](crate::commands::DeviceOperation) | What message performs this action? |
| [`state`](crate::state) | [`StateDecoder`](crate::state::StateDecoder) | What did the unit just tell me about itself? |
| [`device`](crate::device) | [`DeviceStateRuntime`](crate::device::state_runtime::DeviceStateRuntime), [`plan_gateway_write`](crate::device::request::plan_gateway_write), [`SessionRuntime`](crate::device::owner::SessionRuntime) | Is this write still safe, and did it land? |
| [`session`](crate::session) | [`SessionDriver`](crate::session::SessionDriver) | What do I send next at startup? |
| [`transport`](crate::transport) | [`TransportRuntime`](crate::transport::TransportRuntime) | Is it time to retry, keep alive, or give up? |
| [`backup`](crate::backup) | [`BackupRuntime`](crate::backup::BackupRuntime) | Is the backup stream complete, stalled, or failed? |

> The `session`, `transport` and `backup` domains re-export items that are
> defined under [`device`](crate::device) (`device::session`,
> `device::transport`, `device::backup`). They are the same items at two paths.
