# Broker demo target integration

The desktop editor exposes its already-opened demo through an authenticated
reverse broker connection. MCP and the editor share one virtual store and its
transient runtime. Demo is selected only by an explicit user action; startup,
failed Connect, broker release, Quit and USB loss show **Disconnected**, not
Demo. An active, explicitly selected provider takes precedence over background
physical availability so MCP and the window serve the same target. This integration has
fixture coverage; live desktop UI/MCP and physical hot-plug acceptance are
separate release gates. Installed applications are unchanged by this branch.

A headless broker without a live editor provider does not invent demo availability.

## Explicit selection and recovery

Selecting Demo explicitly may start the shared broker through the supported
election path. Inventory polling and provider re-registration never start it,
clear a recorded Quit, or kill a broker that another client adopted. After a
Quit, select Demo again to start the broker and recover the saved Demo store;
no manual broker restart is required.

Selection intent is separate from provider availability within one broker epoch:

| Observation or action | Shared target |
| --- | --- |
| Explicit Demo registration acknowledged | That live Demo session |
| Failed inventory read, including a missing optional generation on an older host | Unchanged; unknown is not loss |
| Provider connection lost | Unavailable Demo intent; never physical fallback |
| Positively different broker instance | Demo ends; window follows the new broker, and MCP may address hardware |
| Same editor recovers its retained lease | Fresh Demo session; old requests remain fenced |
| Explicit Demo recovery after a lost acknowledgement/token | Revision-checked replacement, not an unconditional reset |
| Explicit physical selection or Stop Demo | Clear acknowledged first, then physical reads/work |

The editor reuses its existing inventory cadence to recover a failed provider.
While unavailable, its controls cannot address hardware and ordinary MCP calls
are rejected. Failed, queued or uncertain commands are never replayed after
recovery. The saved Demo document is retained.

**Demo does not survive a broker restart.** This is a deliberate runtime-mode
policy: a new broker has no Demo selection and defaults to its physical target.
MCP can therefore address the real unit before the window observes the restart.
On a positively different instance, compared with the last Demo registration
acknowledgement, the window ends Demo, clears that Demo's hold/disconnect latch,
and follows the new broker. It displays: "The broker restarted; Demo ended; the
unit is available to your AI client." The notice survives routine status
refreshes and clears after a successful explicit Demo selection. No marker is
persisted, and this observation never starts a broker or opens Demo.

This restart exception does not apply to a failed/Unknown inventory read,
confirmed temporary absence, or a lost provider on the same broker instance.
Those keep the existing Demo intent unavailable until its retained same-epoch
registration is acknowledged; they never authorize a physical fallback.

Broker-issued selection tokens stay in native memory, outside worker lifetimes;
they are not emitted to webview events, status, logs or cloud metadata. Revision
numbers are not secrets or GUI identity. Dedicated provider connections use the
existing trusted local bearer model. A holder of that bearer can deliberately
invoke selection/recovery operations; a role label or `physicalOnly` flag is not
proof of GUI identity. Ordinary MCP/device calls never invoke those operations.
An explicit token-loss recovery reads the current revision and compares it on
commit; old clear/disconnect events cannot erase a replacement selection.

Selection leases are scoped to a broker epoch. A verified replacement broker
cannot accept an old token. Missing or unreadable metadata alone does not prove
the old broker ended and never authorizes clearing its intent. Explicit Demo
starts/attaches safely; a blocked or older incompatible broker fails closed with
an actionable error instead of substituting hardware.

## Admission contract

`qc_device_broker::target::TargetRegistry` is the source of broker target
selection. Adapters publish usable physical and live demo provider **session**
IDs through `update`; duplicate availability events do not advance generation.
The registered Demo provider represents the editor's explicit selection and
stays selected when background physical availability changes. Selecting a
physical target withdraws the provider before physical work. Subsequent USB
loss leaves no target, rather than restoring Demo. Closing that provider must
publish its absence; reopening it must
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
and may call only the internal `broker.demoSelection`, `broker.demoClearSelection`,
`broker.demoRegister`, `broker.demoNext`,
`broker.demoAdmit`, `broker.demoReply` and `broker.demoWithdraw` methods.
These are not MCP tools or remotely exposed methods.

Registration receives a fresh broker-issued provider session id.
Desktop registration opts into `explicitSelection: true`; the broker validates
the boolean and gives that provider selection precedence. Omitted/false keeps
legacy physical-first routing, including older editors' background providers.
An older broker rejects the new field; the desktop then reports registration
failure instead of displaying a Demo target that MCP cannot share. `Next` is a
bounded local long poll (20 seconds when quiet), not cloud polling. At most
eight calls may remain pending, and each keeps its original 20-second deadline.
Admit revalidates the delivered target and sequence; repeated checks are
idempotent. Stale admission/reply is acknowledged as false without terminating
the live provider. Connection death retires pending work and live availability,
but retains explicit selection intent as unavailable. Non-opt-in legacy
providers keep their previous physical-first routing.

Hosts negotiate `expectedBrokerTarget` and `physicalTargetOrigin` before sending
the corresponding envelope guard or `physicalOnly` constraint. A physical-only
app request never falls back to demo. Rejected capabilities must not cause a
mutation to be replayed without its guard.
`physicalOnly` is a routing constraint, not consent to override explicit Demo:
device work is refused while that intent is held. Passive status/inventory
remain available, and an explicit physical choice clears the selection first.

## Physical selection and lifecycle acknowledgments

When no Demo provider is selected, MCP selects the usable default QC,
otherwise the single known ready QC. It
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
remain fail-closed. Shutdown remains `Unavailable`. Physical contention or
canonical ambiguity cannot override an explicitly selected Demo provider;
without that provider they still refuse unverified physical readiness.
No publication cache grants authority to execute a device request.

Native and shared-store fixtures cover selection generations, stale leases,
provider ownership, queued effect refusal, result metadata (including arrays),
worker shutdown and old local-host refusal before RPC effects. The UI lifecycle
fixtures cover startup/disposal races and replacement identities. The tray
fixtures cover purple eligibility and physical/stale precedence. These tests
do not replace live GUI/MCP, broker restart or hardware hot-plug acceptance.

## Shared TypeScript provider

An editor that adopts an existing Demo hold also records the broker instance
that reported it. A positive observation of a different instance ends that
hold under the restart policy; a failed poll or an absent instance never does.
If a new explicit Demo start fails before registration was attempted, the
editor restores its previous hold state. Native workers report that proof only
after ending, and mark registration attempted before sending its first request.
A timeout, lost acknowledgement, older worker without this proof, or a failed
provider after registration keeps Demo unavailable until an explicit choice.
None of these failure paths selects or replays a command on physical hardware.

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
the existing local inventory events; no cloud timer is added. Leaving Demo
withdraws its registration but preserves its saved store. Explicitly selecting
it again gets a new registration, never a stale physical session's fallback.

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

