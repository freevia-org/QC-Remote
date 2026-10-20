# Broker demo target integration

This is an incremental foundation, not a published demo MCP endpoint. The
registry and palette are implemented; production provider registration and
reverse dispatch are still required. Until then `Snapshot.served_source` is
unset and installed broker behavior is unchanged.

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

The registry's serialized identity is internal. It does not change the public
MCP status schema. Existing `ConnectionState.source` uses `virtual` for a demo
and `device` for hardware; `demo: false` alone does not establish hardware.

## Provider requirements for the next increment

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

The native target tests cover physical-first selection, return to demo with a
new generation, stale expected identity, provider withdrawal/replacement,
duplicate notifications, queued effect refusal and in-flight result rejection
without replay. The tray tests cover purple eligibility and physical/stale
precedence. Hardware and live callback integration remain unvalidated here.

## Shared TypeScript provider

`createDemoBrokerProvider` accepts the already-opened, immutable virtual
transport, a registration identity and a synchronous current-selection check.
It maps generated gateway RPC names back to the existing transport methods,
clones arguments at admission, serializes work with at most eight pending
requests, rejects non-increasing sequence numbers, and checks selection before
and after each call. Native adapters still own authentication, deadlines,
registration exclusivity and final effect admission. Disposing it rejects queued
work; a pending operation is never retried on a replacement target.

Optional `brokerTarget` metadata is defined on the shared client status and
connection types; production status projection remains to be connected. Remote
demo clients retain the host transport route (`source: device`), rather than
routing requests to their own phone-local demo.
