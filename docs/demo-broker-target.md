# Broker demo target integration

The desktop editor exposes its already-opened demo through an authenticated
reverse broker connection. MCP and the editor share one virtual store and its
transient runtime. A usable physical QC takes precedence; unplugging returns
to that same demo with a fresh target generation. This draft integration has
fixture coverage; live desktop UI/MCP and physical hot-plug acceptance are
separate release gates. Installed applications are unchanged by this branch.

A headless broker without a live editor provider does not invent demo availability.

## Admission contract

`qc_device_broker::target::TargetRegistry` is the source of broker target
selection. Adapters publish usable physical and live demo provider **session**
IDs through `update`; duplicate availability events do not advance generation.
Physical wins whenever usable, independently of an editor deliberately viewing
the demo. Losing physical returns to the same demo provider with a new broker
generation. Closing that provider must publish its absence; reopening it must
use a new session ID, even if the persisted demo document is unchanged.

Admit a request exactly once with `admit(expected_identity)`. Resolve and retain
that session's transport/callback, rather than the UI's switchable gateway.
Check the lease before every effect and after every async boundary, and discard
results when it is superseded. `with_current` supplies a synchronous before/after
check; it is not a USB write lock or an asynchronous cancellation mechanism.
Never retry a superseded or uncertain write on a newly selected target.
Provider transport adapters must coordinate final effect admission with native
selection changes and retain their existing write identity/confirmation guards.

Status and relay readiness expose additive `brokerTarget` and `targetReady`
metadata while USB diagnostics continue to describe physical hardware.
`ConnectionState.source` describes the transport route: `virtual` for a
client-local demo and `device` for a host connection, including a remote demo.
It alone does not establish whether the host serves physical hardware.

Target session IDs are opaque and contain no hardware serials or USB paths.

## Native provider protocol

A dedicated authenticated `demoProvider` connection owns registration. It is
not a participant, observer or control connection, does not renew USB demand,
and may call only the internal `broker.demoRegister`, `broker.demoNext`,
`broker.demoAdmit`, `broker.demoReply` and `broker.demoWithdraw` methods.
These are not MCP tools or remotely exposed methods.

Registration receives a fresh broker-issued provider session id. `Next` is a
bounded local long poll (20 seconds when quiet), not cloud polling. At most
eight calls may remain pending, and each keeps its original 20-second deadline.
Admit revalidates the delivered target and sequence; repeated checks are
idempotent. Stale admission/reply is acknowledged as false without terminating
the live provider. Connection death withdraws it and retires pending work.

Hosts negotiate `expectedBrokerTarget` and `physicalTargetOrigin` before sending
the corresponding envelope guard or `physicalOnly` constraint. A physical-only
app request never falls back to demo. Rejected capabilities must not cause a
mutation to be replayed without its guard.

## Physical selection and lifecycle acknowledgments

MCP selects the usable default QC, otherwise the single known ready QC. It
binds the participant, native method route and target identity to that same
controller before queueing. An explicit noncanonical MCP selection is refused;
the physical-only app keeps its own selected controller. Multiple unresolved
physical candidates never cause demo fallback. This is not a general
multi-target MCP selection API.

Reset/reconnect may legitimately change the native epoch. A successful result
can carry `brokerTargetTransition` only with evidence from the bound native
controller and unchanged private backend selection key, plus fresh readiness.
Disconnect additionally requires a verified close, and its new target may only
be absent or demo, never another physical QC. The verification strings are
`same_native_controller` and `closed_native_controller`, respectively. An
unverified transition stays ambiguous and is never replayed. Such proof is not
independent hardware-serial attestation and grants no permission for later writes.

## Provider and desktop bridge contract

- Use the exact `VirtualDeviceStore` transport already opened by the editor.
  Do not read its saved file into a second simulated device.
- Authenticate registration and callbacks using the existing native broker
  session and bind ownership to that connection. Clear registration on death.
- Bound requests and replies, refuse unsupported RPCs, and reject duplicate,
  foreign, expired and unsolicited replies. No fallback to hardware on errors.
- Reply without taking the original request's broker/Tauri gateway locks.
- Feed effective target availability to MCP resources, status, host health and
  relay readiness; keep raw USB diagnostics truthful and physical-only.
- Update the tray from actual broker selection. An editor's local demo tab
  cannot turn the broker purple while the broker serves physical hardware.
- Use lifecycle events and existing local status machinery; add no cloud timer.
- Without a live demo runtime, headless operation must report unavailable
  until an explicitly owned shared demo runtime exists.

The shared purple LED (`Demo`, RGBA 166/112/232/255) uses the same renderer on
Windows, Linux and macOS. The macOS exporter and Swift asset loader include it.
Stale broker state remains red, and a usable physical reading takes precedence.

## Validation

`system.status` distinguishes API liveness from device availability. An
unguarded cache-only status request still reports the broker API and transport
capabilities when readiness is contended. Unavailable cached USB diagnostics
and unverified target identity/readiness are omitted, not synthesized. Explicit
expected-target guards and device effects still fail closed. The status path
uses try-only cached reads and never waits for the retained device tree.
The in-process `DeviceReadiness` probe likewise returns `Unknown` on short
readiness/inventory lock contention, rather than reporting a disconnect. A
publication adapter may skip that sample; ordinary availability checks must
remain fail-closed. Shutdown and canonical ambiguity remain `Unavailable`.
No publication cache grants authority to execute a device request.

Native and shared-store fixtures cover selection generations, stale leases,
provider ownership, queued effect refusal, result metadata (including arrays),
worker shutdown and old local-host refusal before RPC effects. The UI lifecycle
fixtures cover startup/disposal races and replacement identities. The tray
fixtures cover purple eligibility and physical/stale precedence. These tests
do not replace live GUI/MCP, broker restart or hardware hot-plug acceptance.

## Shared TypeScript provider

`createDemoBrokerProvider` accepts the already-opened, immutable virtual
transport, a registration identity and a synchronous current-selection check.
It maps generated gateway RPC names back to the existing transport methods,
clones arguments at admission, serializes work with at most eight pending
requests, rejects non-increasing sequence numbers, and checks selection before
and after each call. Native adapters still own authentication, deadlines,
registration exclusivity and final effect admission. Disposing it rejects queued
work; a pending operation is never retried on a replacement target.

Effect admission runs inside the original virtual store mutation queue, so a
physical selection that supersedes a queued MCP request prevents its mutation.
Per-invocation admission does not interfere with intentional editor edits.

The desktop native worker uses bounded original-thread ownership and exact
socket shutdown. Broker instance observations trigger re-registration through
the existing local inventory events; no cloud timer is added. Returning to an
already-served demo does not re-register it just because the UI changed views.

The Tauri relay, local MCP backend and local HTTP bridge preserve a captured
`expectedBrokerTarget` separately from generated RPC parameters. Unsupported
older hosts are refused before receiving a guarded action. Physical editor
requests use a separately negotiated physical-only origin. Guarded reads cannot
use an unguarded status shortcut, and array response metadata stays in the
native envelope. Uncertain writes are never retried on a replacement target.

Broker tests also cover ownership, bounded saturation, actual
controller routing, serial-free identities and narrow lifecycle proof. The tray
tests cover native icon pixels and a heartbeat while retained device state is
busy. The existing tray tick uses short try-readiness facts, not a blocking
graphics/device-tree read or a new cloud timer.

