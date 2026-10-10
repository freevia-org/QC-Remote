/** Per-method request deadlines for the cloud relay.
 *
 * The cloud worker is the outermost of three deadline layers. It fronts the
 * native host's own verification budgets, and a single hand-picked number here
 * silently truncated every long local workflow: the caller was told "the device
 * did not respond in time" while the host was still legitimately verifying, and
 * for a persistent write that false failure invites the retry which produces a
 * second copy of the preset.
 *
 * So every budget in this file is derived from the local budget it fronts.
 * `INNER_BUDGETS_MS` mirrors `packages/rust/quadcortex/src/profile.rs`.
 * Approval-gated RPCs are derived from generated QC action classifications, and
 * their owner-confirmation wait comes from the shared relay profile.
 * The public contracts:check value-level drift tests fail
 * if any value here drifts from that source, if a method's class stops
 * matching the policy the native host actually selects for it, or if an outer
 * budget ever comes out shorter than the inner budget it fronts.
 */
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };
import { SHARED_QC_HOST_APPROVAL_RPCS } from "../../../packages/typescript/qc-core/src/generated-actions.ts";

/** Verification budgets owned by the native host, mirrored from
 * `packages/rust/quadcortex/src/profile.rs`. Names match that file exactly so
 * the drift check is a name-for-name comparison. */
export const INNER_BUDGETS_MS = {
  READY_WAIT_TIMEOUT_MS: 35000,
  PRESET_SYNC_TIMEOUT_MS: 15000,
  COMMAND_CONFIRMATION_TIMEOUT_MS: 3000,
  PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS: 45000,
  PERSISTENT_FILE_VERIFICATION_TIMEOUT_MS: 120000,
  DUPLICATE_SETLIST_VERIFICATION_MAXIMUM_TIMEOUT_MS: 600000,
  BACKUP_TOTAL_TIMEOUT_MS: 180000
} as const;

/** One persistent File mutation whose outcome is proved by a fresh catalog
 * generation under `CatalogVerificationPolicy::persistent_file_mutation`. */
export const PERSISTENT_FILE_CATALOG_RPCS = [
  "device.savePresetAs",
  "device.renameCurrentPreset",
  "device.copyPreset",
  "device.deletePreset",
  "device.movePreset",
  "device.deleteSetlist"
] as const;

/** Catalog mutations verified under `CatalogVerificationPolicy::default`. */
export const CATALOG_DEFAULT_RPCS = ["device.createSetlist"] as const;

/** Verified under `CatalogVerificationPolicy::duplicate_setlist`, whose window
 * grows with the source setlist's occupied slots up to a documented maximum. */
export const DUPLICATE_SETLIST_RPCS = ["device.duplicateSetlist"] as const;

/** Streamed off the device under the host's own total backup budget. */
export const DEVICE_BACKUP_RPCS = ["device.createBackup"] as const;

/**
 * Older paired hosts can still display their 120-second approval prompt. Keep
 * the compatibility allowance in the cloud deadline until all installed hosts
 * have upgraded; current hosts do not spend this time because confirmation is
 * handled by the initiating AI client before dispatch.
 */
export const HOST_APPROVAL_REQUIRED_RPCS = new Set<string>(
  SHARED_QC_HOST_APPROVAL_RPCS
);
export const HOST_APPROVAL_TIMEOUT_MS: number = relayProfile.hostApprovalTimeoutMs;
/** API 31 is the first host release that guarantees no paired-host prompt. */
export const HOST_APPROVAL_REMOVED_GATEWAY_API_VERSION = 31;

/** What `recover_session_after_timeout` can spend before a second verification
 * pass begins: an in-place rebuild wait, a post-reset ready wait, and the
 * snapshot confirmation the catalog verifier takes before retrying. */
const SESSION_RECOVERY_ALLOWANCE_MS =
  2 * INNER_BUDGETS_MS.READY_WAIT_TIMEOUT_MS + INNER_BUDGETS_MS.COMMAND_CONFIRMATION_TIMEOUT_MS;

/** The widest single window an ordinary read or live-state write can occupy is
 * one preset synchronization, and any of them may recover the session once. */
const BASELINE_INNER_BUDGET_MS = INNER_BUDGETS_MS.PRESET_SYNC_TIMEOUT_MS + SESSION_RECOVERY_ALLOWANCE_MS;

/** Slack for the two WebSocket hops between this worker and the native host, so
 * a result produced exactly at the host's own deadline still arrives inside
 * ours. One device confirmation window, not an invented number. */
const TRANSPORT_MARGIN_MS = INNER_BUDGETS_MS.COMMAND_CONFIRMATION_TIMEOUT_MS;

const includes = (rpcs: readonly string[], rpc: string) => rpcs.includes(rpc);

/** The longest the native host can legitimately spend on `rpc` before it owes an
 * answer. */
export function innerBudgetMs(rpc: string): number {
  let verificationPassMs: number;
  if (includes(PERSISTENT_FILE_CATALOG_RPCS, rpc)) {
    verificationPassMs = INNER_BUDGETS_MS.PERSISTENT_FILE_VERIFICATION_TIMEOUT_MS;
  } else if (includes(CATALOG_DEFAULT_RPCS, rpc)) {
    verificationPassMs = INNER_BUDGETS_MS.PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS;
  } else if (includes(DUPLICATE_SETLIST_RPCS, rpc)) {
    verificationPassMs = INNER_BUDGETS_MS.DUPLICATE_SETLIST_VERIFICATION_MAXIMUM_TIMEOUT_MS;
  } else if (includes(DEVICE_BACKUP_RPCS, rpc)) {
    // A backup streams under one bounded budget and is never re-verified
    // against a fresh catalog, so it costs one pass and no recovery.
    return Math.max(INNER_BUDGETS_MS.BACKUP_TOTAL_TIMEOUT_MS, BASELINE_INNER_BUDGET_MS);
  } else {
    return BASELINE_INNER_BUDGET_MS;
  }
  // `verify_preset_listing_after_mutation` recovers the session and verifies a
  // second time rather than replaying the write, so a catalog mutation's honest
  // bound is two passes plus that recovery.
  return Math.max(2 * verificationPassMs + SESSION_RECOVERY_ALLOWANCE_MS, BASELINE_INNER_BUDGET_MS);
}

/** How long this worker allows for `rpc` before it may stop waiting. */
export function commandDeadlineMs(rpc: string, gatewayApiVersion?: number): number {
  const legacyHostApprovalAllowanceMs = HOST_APPROVAL_REQUIRED_RPCS.has(rpc)
    && (gatewayApiVersion === undefined || gatewayApiVersion < HOST_APPROVAL_REMOVED_GATEWAY_API_VERSION)
    ? HOST_APPROVAL_TIMEOUT_MS : 0;
  return legacyHostApprovalAllowanceMs + innerBudgetMs(rpc) + TRANSPORT_MARGIN_MS;
}

/** What the caller is told when the deadline passes.
 *
 * The old message — "The device did not respond in time" — was false whenever
 * the host was still verifying, and on a write it reads as "nothing happened",
 * which invites the retry that leaves a second copy of the preset behind. */
export function deadlineExceededMessage(rpc: string, gatewayApiVersion?: number): string {
  return `QC Remote stopped waiting for ${rpc} after ${Math.round(commandDeadlineMs(rpc, gatewayApiVersion) / 1000)}s. This is the relay's own deadline, not a reply from the Quad Cortex: the device host may still be working, and if this was a write its outcome is unknown. Read the device's current state before deciding anything, and do not repeat the request on the assumption that it did not happen.`;
}
