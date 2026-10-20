import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import { requestInitializeClient, requestClientMetadata, observedClientName, mcpPresenceFrame, updateMcpPresence, forgetMcpGrant, publicMcpPresence, rememberedMcpPresence, nextPresenceExpiry, type McpObservation, type StoredMcpPresence, type McpClientPresence } from "./mcp-presence.ts";
import OAuthProvider, { getOAuthApi, type OAuthProviderOptions } from "@cloudflare/workers-oauth-provider";
import { createMcpHandler } from "agents/mcp/server";
import { McpServer } from "@modelcontextprotocol/server";
import { z } from "zod";
import { SHARED_QC_ACTIONS, SHARED_QC_ASSISTANT_GUIDE, SHARED_QC_MCP_INSTRUCTIONS } from "../../../packages/typescript/qc-core/src/generated-actions";
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };
import { QC_COLORS, QC_TYPOGRAPHY } from "@qc-remote/theme/server";
import { completePairingSignIn, handleGoogleAuthorization, type GoogleOAuthEnv } from "./access-auth.ts";
import { ownerSessionKey, ownerSubject, PRIVATE_OWNER_KEY, relayEnvironment, type RelayEnvironment } from "./owner-key.ts";
import { accessTokenGrantId, connectorGrantUser, listConnectorGrants, retireGrantsForEmptiedAccount } from "./connector-grants.ts";
import { isMcpSetupPageRequest } from "./mcp-setup-request.ts";
import { commandDeadlineMs, deadlineExceededMessage, innerBudgetMs } from "./device-deadlines.ts";
import { LateResultLog } from "./late-results.ts";
import { selectDeviceHost } from "./device-selection.ts";
import { decodeDeviceMessage } from "./device-messages.ts";
import { assertBrokerTarget, deviceReadiness, parseBrokerTarget, sameBrokerTarget, verifiedTargetTransition, type BrokerTarget } from "./device-readiness.ts";
import { deviceInvocationError, deviceInvocationToolError, deviceResourceRead, deviceToolCall } from "./device-errors.ts";
import { sanitizePublicResult } from "./public-result.ts";
import { applyWorkerInputs } from "./model-query.ts";
import { cancelPairing, completeHostPairingSession, createHostPairingSession, hostPairingStatus, hostPairingUnavailable, openHostPairingSession, showPairingConfirmation, startPairingSignIn, type HostPairingSession } from "./host-pairing.ts";
import { cancelHostPairingState, claimHostPairingState, completeHostPairingState, dispatchDeviceSessionAlarm, initializeHostPairingState, pollHostPairingState, readHostPairingState } from "./host-pairing-state.ts";
import { adminGate } from "./admin-auth.ts";
import { fencedProviderEnv, accountDeletionDeadline, accountDeletionDenial, admitGrant, admitMcpRequest, admitProviderWrite, assertAccountWritable, beginAccountDeletion, closeCompensation, fenceProviderKv, markProviderDelete, publicDeletionStatus, scheduleAccountDeletionRetry, settleProviderWrite, sweepProviderWrites, PROVIDER_WRITES_SWEEP_KEY, scheduleProviderWritesRetention, enrollProviderWritesRetention, type ProviderKv, queueRefusedDirectoryEntry, readAccountDeletion, requestAccountDeletion, runAccountDeletion, runAccountDeletionAlarm, settleGrant, ACCOUNT_DELETION_PENDING_MESSAGE, ACCOUNT_DELETION_WEB_PATH, STALE_GENERATION_MESSAGE, type AccountDeletionOutcome, type PublicDeletionStatus } from "./account-deletion.ts";
import { confirmAccountDeletion, continueAccountDeletion, showAccountDeletionConfirmation, showAccountDeletionStart, showAccountDeletionStatus, startAccountDeletionSignIn, type AccountDeletionWebDeps } from "./account-deletion-web.ts";
import { clientFailure, htmlPageHeaders } from "./http-security.ts";
import { enforceRateLimit } from "./rate-limit.ts";
import { OAUTH_TOKEN_LIFETIMES } from "./oauth-lifetimes.ts";
import { QC_SCOPES } from "./scopes.ts";
import { NO_PAIRED_HOST } from "./device-selection.ts";
import { adaptForHost, isVersionSensitive, legacyModelGuardFailure, projectAction, projectAdminCommand, reportedGatewayApiVersion } from "./action-projection.ts";
import { sharedTool } from "./shared-tool-shape.ts";
import { adoptReplacement, admitPairingAttempt, resolvePairingAttempt, PAIRING_REFUSAL_MESSAGES, PAIRING_REFUSAL_RESPONSES, type PairingAdmission, type PairingResolution, DEVICE_PAIR_REPLACE_PATH, pairingPrevious, REPLACEMENT_ACK_PARAM, replacementAckFrame, completeDevicePairing, drainDirectoryCleanup, DEVICE_SWEEP_KEY, earliestDeadline, liveDeviceCount, redeemPairingOffer, refreshDeviceDirectory, sweepExpiredDevices, touchDeviceLastUse, type DirectoryKv, type AccountWriteDenial, type ConnectAdmission, type PendingReplacement, type RetentionTransaction } from "./device-retention.ts";
import { backfillDeviceSessionIds, DEVICE_SESSION_LATE_RESULTS_KEY, initializeDeviceSessionState, inspectDeviceSessionIds, inspectDeviceSessionStorage, requestLegacyRetentionBackfill, type DeviceSessionInventory, type DeviceSessionRetentionBackfillResult } from "./device-session-inventory.ts";

type SecretEnv = {
  QCRC_ADMIN_TOKEN?: string;
  OAUTH_PROVIDER: GoogleOAuthEnv["OAUTH_PROVIDER"];
  GOOGLE_CLIENT_ID: string;
  GOOGLE_CLIENT_SECRET: string;
  COOKIE_ENCRYPTION_KEY: string;
  QCRC_VERSION_METADATA: WorkerVersionMetadata;
};

// Durable Object, KV, and ordinary variable bindings are generated directly
// from wrangler.jsonc. Only dashboard-managed secrets and the OAuth provider's
// injected grant binding remain declared by hand.
export type Env = Omit<Cloudflare.Env, "QCRC_ENVIRONMENT"> & SecretEnv & {
  QCRC_ENVIRONMENT: RelayEnvironment;
};

type Json = Record<string, unknown>;
/** `connectionId` is this socket's identity, and unlike the socket object it
 * survives hibernation, so a deadline recorded against it can still be
 * recognised by a later incarnation of this session. */
/** `gatewayApiVersion` is what the host reported for this connection, learned
 * once per socket; a reconnect starts a new attachment and so asks again. */
/** `credentialHash` is the hash of the credential this socket was admitted
 * with: every message and dispatch re-checks that the record still
 * holds it, so a socket accepted while its record was being revoked cannot run
 * anything. Sockets accepted before this field existed carry none and rely on
 * revocation closing them by device id. */
type DeviceAttachment = { deviceId: string; deviceName: string; ready: boolean; usbConnected?: boolean; brokerTarget?: BrokerTarget; connectionId?: string; gatewayApiVersion?: number; mcpPresence?: boolean; credentialExpiresAt?: number | null; credentialHash?: string };
type PairingOffer = { codeHash: string; expiresAt: number; redeemed: boolean };
type PairingMetadata = { email: string; device: string; pairedAt: number };
/** `lastUsedAt` is the last authenticated use, written at most once a day;
 * retention is measured from it (device-retention.ts). */
/** `replaces` is a re-pair's pending predecessor (device-retention.ts). */
type DeviceRecord = { credentialHash: string; name: string; pairedAt: number; expiresAt?: number | null; lastUsedAt?: number; identityProvider: "google"; pairing?: PairingMetadata; replaces?: PendingReplacement; replacedDeviceId?: string };
type PendingCommand = { socket: WebSocket; method: string; expectedBrokerTarget?: BrokerTarget; resolve: (value: Json) => void; reject: (reason: Error) => void; timeout: ReturnType<typeof setTimeout> };
type PairingDirectory = { owner: string; expiresAt: number; pairing?: PairingMetadata };
type DeviceDirectory = { owner: string };
/** `remaining` is the account's device count after the removal, which only the
 * Durable Object can see and only the request handler can act on. */
type RevocationOutcome = { revoked: boolean; remaining: number };
type PairOfferResult = { pairingCode: string; codeHash: string; expiresAt: number; protocol: string };
/** `previousDeviceReplacementPending` is true when the previous device id and
 * credential were proven: that record is deleted when the new credential first
 * connects, not now, so the host keeps the old credential until then. It is
 * false when a previous tuple was sent but not proven, and absent when none was. */
type PairReceipt = { deviceId: string; deviceCredential: string; protocol: string; identityProvider: "google"; pairing?: PairingMetadata; previousDeviceReplacementPending?: boolean };
/** What a re-pairing host may send to have its previous record replaced once
 * the new credential connects. */
type PreviousCredential = { deviceId: string; credential: string };
type DailyUsage = { day: string; commands: number; lastCommandAt?: number };

const OWNER = PRIVATE_OWNER_KEY;
const OFFER_TTL_MS = 10 * 60_000;
const MCP_PRESENCE_KEY = "mcp-presence-v1";
/** Fallback only for legacy records without an explicit expiry. New records
 * use null (persistent), and valid legacy records migrate on authentication. */
const CREDENTIAL_TTL_MS = 90 * 24 * 60 * 60_000;
const MAX_JSON_BODY_BYTES = 64_000;
/** The `origin` an `invoke` frame carries when it serves an MCP resource
 * read, set here at the MCP ingress, where a resource read and a tool call
 * are still told apart. A host then serves the read only from a device
 * connection and session already open, never bringing one up. It can only
 * narrow a call; a host that predates it ignores it. Mirrors
 * `RESOURCE_READ_ORIGIN` in packages/rust/qc-relay-protocol/src/lib.rs. */
const RESOURCE_READ_ORIGIN = "resourceRead";
type InvokeOrigin = typeof RESOURCE_READ_ORIGIN;
/** Refusals `DeviceSession.pair` and `admitPairing` write for the person
 * pairing; any other failure there is internal and is reported only by
 * correlation id. */
const PUBLIC_PAIRING_FAILURES = ["A device name is required", "Pairing offer is invalid or expired",
  PAIRING_REFUSAL_MESSAGES["offer-closed"], PAIRING_REFUSAL_MESSAGES["open-attempts"], PAIRING_REFUSAL_MESSAGES.reservations] as const;
/** A short, deliberately normalized description of the authenticated browser.
 * Raw user agents are never persisted or returned to the desktop app. */
function authenticatedDeviceLabel(request: Request): string {
  const agent = request.headers.get("user-agent") ?? "";
  const platform = /Android/i.test(agent) ? "Android" : /iPhone|iPad|iPod/i.test(agent) ? "iPhone" : /Windows/i.test(agent) ? "Windows" : /Macintosh|Mac OS X/i.test(agent) ? "Mac" : "Web";
  const browser = /EdgA?|EdgiOS/i.test(agent) ? "Edge" : /Firefox|FxiOS/i.test(agent) ? "Firefox" : /Chrome|CriOS/i.test(agent) ? "Chrome" : /Safari/i.test(agent) ? "Safari" : "browser";
  return `${platform} ${browser}`;
}

function actionCatalog(): Json {
  return {
    instructions: SHARED_QC_MCP_INSTRUCTIONS,
    actions: SHARED_QC_ACTIONS.map(({ name, rpc, classification, access, description, inputSchema }) => ({ name, rpc, classification, access, description, inputSchema }))
  };
}

/** The direct `set_tempo` tool advertises exactly the shared contract's schema
 * and description (shared-tool-shape.ts), so it cannot drift from
 * `execute_qc_action` or the local MCP servers. */
const SHARED_SET_TEMPO = sharedTool("set_tempo");

function hostPairingSecret(request: Request): string | null {
  const token = bearer(request);
  return token && /^[A-Za-z0-9_-]{32,128}$/.test(token) ? token : null;
}

function json(value: Json, status = 200): Response {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" } });
}

function error(message: string, status: number): Response {
  return json({ error: message }, status);
}

function randomToken(bytes = 32): string {
  const value = crypto.getRandomValues(new Uint8Array(bytes));
  return Array.from(value, (part) => part.toString(16).padStart(2, "0")).join("");
}

async function hash(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (part) => part.toString(16).padStart(2, "0")).join("");
}

async function constantTimeEqual(left: string, right: string): Promise<boolean> {
  const encoder = new TextEncoder();
  const [leftHash, rightHash] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(left)),
    crypto.subtle.digest("SHA-256", encoder.encode(right))
  ]);
  return crypto.subtle.timingSafeEqual(leftHash, rightHash);
}

function directoryKv(env: Env): DirectoryKv {
  return env.QCRC_OAUTH as unknown as DirectoryKv;
}

/** Redeeming a high-entropy, single-use, short-lived pairing code is the
 * host's authorization. Requiring an owner bearer here would force the secret
 * into an app and defeat the QR pairing model. `replace` selects the two-phase
 * re-pair route, the only one that reads the previous* fields. */
async function pairDevice(env: Env, payload: Json | null, replace: boolean): Promise<Response> {
  if (!payload || typeof payload.pairingCode !== "string" || typeof payload.deviceName !== "string") return error("pairingCode and deviceName are required", 400);
  const request = pairingPrevious(payload, replace ? "replace" : "pair");
  if (!request.ok) return error(request.error, 400);
  const previous = request.previous;
  const pairingHash = await hash(payload.pairingCode);
  const directory = await env.QCRC_OAUTH.get<PairingDirectory>(`qcrc:pair:${pairingHash}`, "json");
  if (!directory || directory.expiresAt < Date.now()) return error("Pairing offer is invalid or expired", 400);
  try {
    const deviceId = crypto.randomUUID();
    // One id per route attempt, so a failed call is resolved against exactly
    // this attempt's committed state (device-retention.ts `completeDevicePairing`).
    const operationId = crypto.randomUUID();
    const receipt = await completeDevicePairing(directoryKv(env), { owner: directory.owner, pairingHash, deviceId },
      () => rpcSession(env, directory.owner).pair(payload.pairingCode as string, payload.deviceName as string, directory.pairing, deviceId, previous, operationId),
      { admit: () => rpcSession(env, directory.owner).admitPairing(operationId, deviceId, payload.pairingCode as string), resolve: () => rpcSession(env, directory.owner).resolvePairing(operationId, deviceId, payload.pairingCode as string) });
    return json(receipt);
  }
  // Only the refusals the Durable Object writes for a person are repeated;
  // RPC drops the error class, so they are matched by text. The capacity
  // refusals also carry a stable code and status (PAIRING_REFUSAL_RESPONSES).
  catch (reason) { return clientFailure(reason, { status: 400, fallback: "Pairing failed. Open REMOTE in QC Remote and start again.", context: replace ? "device-pair-replace" : "device-pair", publicMessages: PUBLIC_PAIRING_FAILURES, publicCodes: PAIRING_REFUSAL_RESPONSES }); }
}

function bearer(request: Request): string | null {
  const match = request.headers.get("authorization")?.match(/^Bearer\s+(.+)$/i);
  return match?.[1]?.trim() || null;
}

async function body(request: Request): Promise<Json | null> {
  try {
    const declaredLength = request.headers.get("content-length");
    if (declaredLength && (!/^\d+$/.test(declaredLength) || Number(declaredLength) > MAX_JSON_BODY_BYTES)) return null;
    if (!request.body) return null;
    const reader = request.body.getReader();
    const chunks: Uint8Array[] = [];
    let length = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > MAX_JSON_BODY_BYTES) {
        await reader.cancel("JSON request body exceeds the QC Remote limit");
        return null;
      }
      chunks.push(value);
    }
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }
    const parsed: unknown = JSON.parse(new TextDecoder().decode(bytes));
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed) ? parsed as Json : null;
  } catch {
    return null;
  }
}

function session(env: Env, owner = OWNER): DurableObjectStub<DeviceSession> {
  return env.DEVICE_SESSION.getByName(owner);
}

function rpcSession(env: Env, owner = OWNER): {
  observeMcpClient(observation: McpObservation, generation?: number): Promise<void>;
  forgetMcpGrant(grantId: string): Promise<void>;
  mcpClientActivity(): Promise<McpClientPresence[]>;
  createOffer(): Promise<PairOfferResult>;
  pair(pairingCode: string, deviceName: string, pairing: PairingMetadata | undefined, deviceId: string, previous?: PreviousCredential, operationId?: string): Promise<PairReceipt>;
  admitPairing(operationId: string, deviceId: string, pairingCode: string): Promise<PairingAdmission>;
  resolvePairing(operationId: string, deviceId: string, pairingCode: string): Promise<PairingResolution>;
  offerCurrent(codeHash: string): Promise<boolean>;
  requestAccountDeletion(subject: string): Promise<AccountDeletionOutcome>;
  continueAccountDeletion(operationId: string): Promise<AccountDeletionOutcome | null>;
  accountDeletionStatus(): Promise<PublicDeletionStatus>;
  admitGrant(admissionId: string): Promise<{ admitted: true; generation: number } | { admitted: false }>;
  settleGrant(admissionId: string, generation: number): Promise<boolean>;
  admitMcpRequest(generation: unknown, observation?: McpObservation): Promise<"ok" | "deleting" | "stale">;
  admitProviderWrite(key: string, generation: number | undefined, expiresAt: number | undefined): Promise<{ admitted: true; admissionId: string } | { admitted: false }>;
  settleProviderWrite(admissionId: string): Promise<"keep" | "compensate">;
  closeCompensation(admissionId: string, now: number): Promise<void>;
  markProviderDelete(key: string): Promise<void>;
  status(): Promise<Json>;
  usage(generation?: number): Promise<Json>;
  usageForDevice(deviceId: string, credential: string): Promise<Json>;
  authorizeDevice(deviceId: string, credential: string): Promise<boolean>;
  revokeDevice(deviceId: string, credential: string): Promise<RevocationOutcome>;
  rosterForDevice(deviceId: string, credential: string): Promise<Json>;
  revokeSibling(deviceId: string, credential: string, target: string): Promise<RevocationOutcome>;
  describeDevice(deviceId: string): Promise<Json | null>;
  command(payload: Json, generation?: number): Promise<Json>;
} {
  return session(env, owner);
}

/** The remote account deletion pages' view of the relay (account-deletion-web.ts).
 * Production only: single-owner compatibility mode has no per-account
 * namespace to delete. */
function accountDeletionDeps(env: Env): AccountDeletionWebDeps {
  return {
    enabled: relayEnvironment(env.QCRC_ENVIRONMENT) === "production",
    styles: pairingStyles(),
    authorityFor: (subject) => {
      const account = rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, subject));
      return {
        requestAccountDeletion: () => account.requestAccountDeletion(subject),
        continueAccountDeletion: (operationId) => account.continueAccountDeletion(operationId),
        accountDeletionStatus: () => account.accountDeletionStatus(),
      };
    },
  };
}

const baseHandler: ExportedHandler<Env> = {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/health") {
      return json({ service: "qc-remote-cloud", environment: env.QCRC_ENVIRONMENT, status: "ok" });
    }
    if (request.method === "GET" && url.pathname === "/") return new Response(landingPage(), { headers: pairingPageHeaders() });
    // Remote account deletion (account-deletion-web.ts). The apps open the
    // first page; nothing deletes without a Google sign-in confirmation and
    // the typed confirmation on the page after it.
    if (request.method === "GET" && url.pathname === ACCOUNT_DELETION_WEB_PATH) return showAccountDeletionStart(accountDeletionDeps(env));
    if (request.method === "POST" && url.pathname === `${ACCOUNT_DELETION_WEB_PATH}/sign-in`) return startAccountDeletionSignIn(request, env, accountDeletionDeps(env));
    if (request.method === "GET" && url.pathname === "/auth/google/delete/callback") return showAccountDeletionConfirmation(request, env, accountDeletionDeps(env));
    if (request.method === "POST" && url.pathname === `${ACCOUNT_DELETION_WEB_PATH}/confirm`) return confirmAccountDeletion(request, env, accountDeletionDeps(env));
    if (request.method === "GET" && url.pathname === `${ACCOUNT_DELETION_WEB_PATH}/status`) return showAccountDeletionStatus(request, env, accountDeletionDeps(env));
    if (request.method === "POST" && url.pathname === `${ACCOUNT_DELETION_WEB_PATH}/continue`) return continueAccountDeletion(request, env, accountDeletionDeps(env));
    if (request.method === "GET" && url.pathname === "/pair") {
      // A pairing link opens a page that says what is being added and asks
      // for an explicit Continue (host-pairing.ts). It used to go straight to
      // Google on the reasoning that scanning the code was the intent - true
      // for the person who scanned it, and exactly wrong for a link someone
      // else sent, which attached the sender's computer to the recipient's
      // account with no screen in between.
      const hostSession = url.searchParams.get("hostSession")?.trim();
      if (hostSession) return showPairingConfirmation(request, env, hostSession, pairingStyles());
      return new Response(pairingWelcomePage(), { headers: pairingPageHeaders() });
    }
    // Continue on the confirmation page, or the manual path with no session.
    // A session is taken only from the form body, and only alongside the
    // confirmation token the page set for this browser, so this route cannot
    // be reached from another site or walked around with a GET.
    if (request.method === "POST" && url.pathname === "/pair/sign-in") {
      return startPairingSignIn(request, env, pairingStyles());
    }
    if (request.method === "POST" && url.pathname === "/pair/cancel") {
      return cancelPairing(request, env, pairingStyles());
    }
    if (request.method === "GET" && url.pathname === "/auth/google/pair/callback") {
      try {
        const owner = await completePairingSignIn(request, env);
        // A host session completes once. Checked before the offer is minted,
        // because minting replaces this account's pending offer.
        const open = owner.hostSession ? await openHostPairingSession(env, owner.hostSession) : undefined;
        if (open && !open.ok) return hostPairingUnavailable(open.reason, pairingStyles());
        const ownerKey = ownerSessionKey(env.QCRC_ENVIRONMENT, owner.sub);
        let offer: PairOfferResult;
        try { offer = await rpcSession(env, ownerKey).createOffer(); }
        catch (reason) {
          if (reason instanceof Error && reason.message === ACCOUNT_DELETION_PENDING_MESSAGE) return accountBeingDeletedPage();
          throw reason;
        }
        const pairing: PairingMetadata = { email: owner.email, device: authenticatedDeviceLabel(request), pairedAt: Date.now() };
        await env.QCRC_OAUTH.put(`qcrc:pair:${offer.codeHash}`, JSON.stringify({ owner: ownerKey, expiresAt: offer.expiresAt, pairing } satisfies PairingDirectory), { expirationTtl: OFFER_TTL_MS / 1000 });
        // A deletion that began after the offer was minted has already deleted
        // the offer and may have missed this entry: remove it, and stop.
        if (!await rpcSession(env, ownerKey).offerCurrent(offer.codeHash)) {
          await env.QCRC_OAUTH.delete(`qcrc:pair:${offer.codeHash}`).catch(() => {});
          return accountBeingDeletedPage();
        }
        if (owner.hostSession && open?.ok) {
          if (!await completeHostPairingSession(env, owner.hostSession, open.claimId, offer.pairingCode, pairing)) {
            return hostPairingUnavailable("used", pairingStyles());
          }
          return new Response(hostPairingCompletePage(owner.email, pairing.device, pairing.pairedAt), { headers: pairingPageHeaders() });
        }
        return new Response(pairingPage(offer.pairingCode, offer.expiresAt, owner.email), { headers: pairingPageHeaders() });
      } catch (cause) {
        return clientFailure(cause, { status: 403, fallback: "Google sign-in could not be completed. Open REMOTE in QC Remote and start again.", context: "pairing-callback" });
      }
    }

    // Anyone can start a pairing session: the host has no credential yet, and
    // requiring one would put an owner secret into the app. What the session
    // cannot do is prove whose host it is, so the link to it is a capability -
    // whoever opens it decides which account the host that created it will
    // belong to. A host that already belongs to an account now refuses a
    // receipt naming a different one, so the link can no longer take a device
    // away from its owner, and the session itself needs nothing further.
    //
    // The host may declare a label and platform in the body; the confirmation
    // page shows them, marked as self-reported, beside when and where the
    // request came from.
    if (request.method === "POST" && url.pathname === "/v1/host/pairing-sessions") {
      return createHostPairingSession(request, env, await body(request));
    }

    const hostSessionMatch = url.pathname.match(/^\/v1\/host\/pairing-sessions\/([A-Za-z0-9_-]{20,128})$/);
    if (request.method === "GET" && hostSessionMatch) {
      const receiverSecret = hostPairingSecret(request);
      const sessionId = hostSessionMatch[1];
      let status: Awaited<ReturnType<typeof hostPairingStatus>>;
      // A storage failure while handing out the code returns nothing and keeps
      // the code undelivered (host-pairing-state.ts); the host may poll again.
      try { status = receiverSecret ? await hostPairingStatus(env, sessionId, receiverSecret) : null; }
      catch (reason) { return clientFailure(reason, { status: 503, fallback: "Pairing status is temporarily unavailable. Try again.", context: "host-pairing-poll" }); }
      // Unknown, expired, wrong secret, or already collected once.
      if (!status) return error("Pairing session is unavailable", 404);
      if (status.state === "waiting") return json(status, 202);
      return json(status);
    }

    if (request.method === "GET" && url.pathname === "/v1/device/usage") {
      const deviceId = url.searchParams.get("deviceId");
      const credential = bearer(request);
      if (!deviceId || !credential) return error("Device credentials are required", 401);
      const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${deviceId}`, "json");
      if (!directory) return error("Unknown paired device", 401);
      try { return json(await rpcSession(env, directory.owner).usageForDevice(deviceId, credential)); }
      catch { return error("Invalid device credentials", 401); }
    }

    // Every device paired to the account that owns this one.
    //
    // Answered to any device holding a live credential for that account. A host
    // knows only the pairing it performed itself, so the desktop panel could
    // list the computer it runs on and nothing else: pairing a phone wrote a
    // record on the relay that no screen anywhere could show, and an account
    // with three devices on it looked empty from all three.
    //
    // Requiring an owner secret instead would put one in the app, which is the
    // same reason pairing does not ask for one. Credential hashes stay in the
    // Durable Object; this returns only what the owner already saw when they
    // paired each device.
    if (request.method === "GET" && url.pathname === relayProfile.deviceRosterPath) {
      const deviceId = url.searchParams.get("deviceId");
      const credential = bearer(request);
      if (!deviceId || !credential) return error("Device credentials are required", 401);
      const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${deviceId}`, "json");
      if (!directory) return error("Unknown paired device", 401);
      try { return json(await rpcSession(env, directory.owner).rosterForDevice(deviceId, credential)); }
      catch { return error("Invalid device credentials", 401); }
    }

    // OAuth grants issued to remote AI apps. These are the durable links the
    // owner expects under "Linked Remote MCP apps"; paired QC Remote hosts are
    // a different roster and must never be presented as connector clients.
    if (url.pathname === relayProfile.deviceConnectorsPath) {
      const deviceId = url.searchParams.get("deviceId");
      const credential = bearer(request);
      if (!deviceId || !credential) return error("Device credentials are required", 401);
      const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${deviceId}`, "json");
      if (!directory) return error("Unknown paired device", 401);
      let authenticated = false;
      try { authenticated = await rpcSession(env, directory.owner).authorizeDevice(deviceId, credential); }
      catch { /* Reported below as the same credential failure. */ }
      if (!authenticated) return error("Invalid device credentials", 401);
      const subject = ownerSubject(env.QCRC_ENVIRONMENT, directory.owner);
      if (!subject) return json({ connectors: [] });
      if (request.method === "GET") {
        const activity = await rpcSession(env, directory.owner).mcpClientActivity();
        return json({ connectors: await listConnectorGrants(env.OAUTH_PROVIDER, subject, activity) });
      }
      if (request.method === "POST") {
        const target = url.searchParams.get("target");
        if (!target || !/^[A-Za-z0-9_-]{8,128}$/.test(target)) return error("A connector grant is required", 400);
        // The provider's revoke is idempotent (an unknown grant deletes
        // nothing and succeeds), so a throw is a storage failure, not "not
        // found": say so, rather than the 404 hosts read as already gone.
        try { await env.OAUTH_PROVIDER.revokeGrant(target, connectorGrantUser(subject)); }
        catch { return error("Remote MCP connector could not be unlinked", 503); }
        // Withdraw that grant's presence now; a failure here only leaves the
        // rows to lapse with their lease, as before.
        await rpcSession(env, directory.owner).forgetMcpGrant(target).catch(() => {});
        return json({ revoked: true });
      }
      return error("Method not allowed", 405);
    }

    // Retiring a device that is not the one asking.
    //
    // Revoking has always required the credential being withdrawn, which is
    // right for a device retiring itself and useless for every other one: a
    // phone's credential exists only on that phone, so a phone that was lost,
    // wiped or simply left behind could not be removed from anywhere. Any live
    // device of the account may now do it, which is the trust the roster above
    // already extends. Both ids are checked against the same owner first, so a
    // credential for one account cannot name a device on another.
    if (request.method === "POST" && url.pathname === `${relayProfile.deviceRosterPath}/revoke`) {
      const deviceId = url.searchParams.get("deviceId");
      const target = url.searchParams.get("target");
      const credential = bearer(request);
      if (!deviceId || !target || !credential) return error("Device credentials are required", 401);
      const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${deviceId}`, "json");
      if (!directory) return error("Unknown paired device", 401);
      const owned = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${target}`, "json");
      if (!owned || owned.owner !== directory.owner) return error("Unknown paired device", 404);
      let outcome: RevocationOutcome;
      try { outcome = await rpcSession(env, directory.owner).revokeSibling(deviceId, credential, target); }
      catch { return error("Invalid device credentials", 401); }
      if (outcome.revoked) await env.QCRC_OAUTH.delete(`qcrc:device:${target}`);
      await retireGrantsForEmptiedAccount(env.OAUTH_PROVIDER, env.QCRC_ENVIRONMENT, directory.owner, outcome.remaining);
      return json({ revoked: outcome.revoked });
    }

    // Authenticated by the device credential being withdrawn, not by an owner
    // secret: the host that holds it is exactly the party entitled to retire it,
    // and requiring anything else would mean an unpair on the phone could not
    // reach the relay at all.
    if (request.method === "POST" && url.pathname === relayProfile.deviceRevokePath) {
      const deviceId = url.searchParams.get("deviceId");
      const credential = bearer(request);
      if (!deviceId || !credential) return error("Device credentials are required", 401);
      const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${deviceId}`, "json");
      if (!directory) return json({ revoked: true });
      const outcome = await rpcSession(env, directory.owner).revokeDevice(deviceId, credential);
      if (!outcome.revoked) return error("Invalid device credential", 401);
      await env.QCRC_OAUTH.delete(`qcrc:device:${deviceId}`);
      await retireGrantsForEmptiedAccount(env.OAUTH_PROVIDER, env.QCRC_ENVIRONMENT, directory.owner, outcome.remaining);
      return json({ revoked: true });
    }

    if (request.method === "GET" && url.pathname === relayProfile.deviceConnectPath) {
      const deviceId = url.searchParams.get("deviceId");
      if (!deviceId || !bearer(request)) return error("Device credentials are required", 401);
      const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(`qcrc:device:${deviceId}`, "json");
      if (!directory) return error("Unknown paired device", 401);
      const response = await session(env, directory.owner).fetch(request);
      // Only an accepted credential extends the directory entry's KV TTL, at
      // most once a day; a failure here never fails the connection.
      if (response.status === 101) {
        await refreshDeviceDirectory(directoryKv(env), deviceId, directory.owner, Date.now(), async () => await rpcSession(env, directory.owner).describeDevice(deviceId) !== null).catch(() => false);
      }
      return response;
    }

    // Ordinary pairing (authorization: see pairDevice). Any previous* fields are ignored here: replacing a
    // previous record is only ever done by the versioned route below, so a
    // host can never trigger a replacement on a Worker that does not speak the
    // two-phase contract (an older Worker answers that route 404 and mutates
    // nothing).
    if (request.method === "POST" && url.pathname === relayProfile.devicePairPath) {
      return pairDevice(env, await body(request), false);
    }

    // Two-phase re-pair. The previous device id and credential are
    // required; the Durable Object marks that record for replacement only when
    // the credential matches it on this same account, and deletes it when the
    // new credential first connects (device-retention.ts).
    if (request.method === "POST" && url.pathname === DEVICE_PAIR_REPLACE_PATH) {
      return pairDevice(env, await body(request), true);
    }

    // Everything below is the single-owner compatibility and bootstrap API.
    // It does not exist in production (admin-auth.ts): 404, token or not.
    const rejected = await adminGate(request, env);
    if (rejected) return rejected;

    if (request.method === "POST" && url.pathname === "/v1/pairing/offers") {
      const offer = await rpcSession(env).createOffer();
      await env.QCRC_OAUTH.put(`qcrc:pair:${offer.codeHash}`, JSON.stringify({ owner: OWNER, expiresAt: offer.expiresAt } satisfies PairingDirectory), { expirationTtl: OFFER_TTL_MS / 1000 });
      return json({ pairingCode: offer.pairingCode, expiresAt: offer.expiresAt, protocol: "qc-relay.v1" });
    }
    // Every paired device and the account that owns it.
    //
    // `/v1/device/status` below reports the single-owner compatibility
    // namespace, which a production pairing does not use: a host reaches its
    // owner through this directory, and an assistant
    // through the Google subject in its token. When those disagree the device
    // heartbeats into a namespace nobody is reading and there was no way to
    // see that from outside -- the operator could only ask the user to read an
    // email off the app's screen. This answers it directly.
    if (request.method === "GET" && url.pathname === "/v1/device/directory") {
      const listed = await env.QCRC_OAUTH.list({ prefix: "qcrc:device:" });
      const devices = await Promise.all(listed.keys.map(async ({ name }) => {
        const deviceId = name.slice("qcrc:device:".length);
        const directory = await env.QCRC_OAUTH.get<DeviceDirectory>(name, "json");
        if (!directory) return { deviceId, owner: null };
        const described = await rpcSession(env, directory.owner).describeDevice(deviceId);
        return { deviceId, owner: directory.owner, ...(described ?? { missing: true }) };
      }));
      return json({ devices, truncated: !listed.list_complete });
    }

    if (request.method === "GET" && url.pathname === "/v1/device/status") return json(await rpcSession(env).status());
    if (request.method === "POST" && url.pathname === "/v1/device/command") {
      const payload = await body(request);
      if (!payload || typeof payload.method !== "string" || !/^[a-z][a-zA-Z0-9_.-]{2,127}$/.test(payload.method)) return error("A valid contract method is required", 400);
      let projected: ReturnType<typeof projectAdminCommand>;
      try { projected = projectAdminCommand(payload); }
      catch (reason) { return error(reason instanceof Error ? reason.message : "Invalid QC action input", 400); }
      try { return json(await rpcSession(env).command(projected)); }
      catch (reason) { return clientFailure(reason, { status: 503, fallback: "The device command could not be completed.", context: "admin-device-command" }); }
    }
    if (url.pathname === "/mcp") return error("MCP OAuth is not configured in single-owner compatibility mode", 503);
    return error("Not found", 404);
  }
};

/** `accountGeneration` is the account generation the grant was admitted
 * under (account-deletion.ts); grants from before it existed carry none (0). */
type PrincipalProps = { subject: string; email: string; name: string; scopes?: string[]; clientName?: string; clientId?: string; accountGeneration?: number };
type AuthenticatedContext = ExecutionContext & { props?: PrincipalProps };

function toolResult(value: Json): { content: Array<{ type: "text"; text: string }> } {
  return { content: [{ type: "text", text: JSON.stringify(value) }] };
}

async function observeMcpRequest(request: Request, env: Env, props: PrincipalProps, defer = false): Promise<McpObservation | undefined> {
  const token = bearer(request);
  if (!token || request.method !== "POST") return;
  const initialized = await requestInitializeClient(request);
  const caller = requestClientMetadata(request);
  const fallbackName = observedClientName(props.clientName);
  // One record per grant/client, independent of rotating tokens or sessions.
  const grantId = accessTokenGrantId(token, props.subject);
  const key = await hash(JSON.stringify([props.subject, grantId ?? props.clientId ?? "unknown-client"]));
  const observation: McpObservation = { key, clientId: props.clientId, grantId, name: initialized.name, version: initialized.version, fallbackName, ...caller };
  if (!defer) await rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, props.subject)).observeMcpClient(observation, props.accountGeneration ?? 0);
  return observation;
}

function mcpHandler(env: Env, props: PrincipalProps) {
  const scopes = new Set(props.scopes ?? []);
  const route = async (command: Json, requiredScope: "qc:read" | "qc:control"): Promise<Json> => {
    if (!props.subject) throw new Error("QC Remote identity is required");
    if (!scopes.has(requiredScope)) throw new Error(`This tool requires the ${requiredScope} OAuth permission.`);
    try {
      return await rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, props.subject)).command(command, props.accountGeneration ?? 0);
    } catch (reason) {
      // Name the account this connector is signed in as. The device's account
      // is shown in QC Remote's REMOTE panel, so saying ours turns a diagnosis
      // into a comparison: two different Google accounts is by far the most
      // likely reason this namespace is empty while the app says it is paired.
      if (reason instanceof Error && reason.message === NO_PAIRED_HOST) {
        throw new Error(`${reason.message} This assistant is signed in as ${props.email}.`);
      }
      throw reason;
    }
  };
  // A resource callback passes RESOURCE_READ_ORIGIN; a tool never does.
  const readStatus = async (origin?: InvokeOrigin) => {
    const status = await route({ action: "get_status", method: "system.status", params: {}, ...(origin ? { origin } : {}) }, "qc:read");
    // This is a cloud admission capability, not evidence about USB or native
    // host support. Clients must not send target-aware writes to an older
    // cloud whose tool schema could silently discard the optional guard.
    const capabilities = Array.isArray(status.capabilities) ? status.capabilities.filter(value => typeof value === "string") : [];
    return { ...status, capabilities: [...new Set([...capabilities, "mcpBrokerTargetGuard"])] };
  };
  const readCurrentPreset = (origin?: InvokeOrigin) => {
    const projected = projectAction("get_current_preset", {});
    return route({ action: projected.action.name, method: projected.method, params: projected.params, ...(origin ? { origin } : {}) }, "qc:read");
  };
  return createMcpHandler(() => {
    // `instructions` is carried at `initialize`, which is the only place a
    // client reads it. Omitting it here meant a cloud client received no
    // operating guidance at all, while the same string was returned only
    // inside a `list_qc_actions` tool result that nothing had to call.
    const server = new McpServer(
      { name: "QC Remote", version: "0.1.0" },
      { instructions: SHARED_QC_MCP_INSTRUCTIONS }
    );
    // The shared instructions name these URIs, so this deployment has to serve
    // them too. They route exactly where the equivalent tools route.
    const resourceJson = (uri: URL, value: Json) =>
      ({ contents: [{ uri: uri.href, mimeType: "application/json", text: JSON.stringify(value) }] });
    server.registerResource("assistant-guide", "qc://assistant-guide", {
      title: "QC Remote assistant guide",
      description: "How to decide when to use QC Remote and operate the connected Quad Cortex safely.",
      mimeType: "text/markdown"
    }, async (uri) => ({ contents: [{ uri: uri.href, mimeType: "text/markdown", text: SHARED_QC_ASSISTANT_GUIDE }] }));
    server.registerResource("status", "qc://status", {
      title: "Quad Cortex status",
      description: "Connection and gateway capability status.",
      mimeType: "application/json"
    }, async (uri) => resourceJson(uri, await deviceResourceRead(() => readStatus(RESOURCE_READ_ORIGIN))));
    server.registerResource("current-preset", "qc://current-preset", {
      title: "Current preset",
      description: "Authoritative preset, scene, blocks, tempo, mode and dirty state.",
      mimeType: "application/json"
    }, async (uri) => resourceJson(uri, await deviceResourceRead(() => readCurrentPreset(RESOURCE_READ_ORIGIN))));
    server.registerResource("models", "qc://models", {
      title: "Model catalog",
      description: "Models in the catalog the paired Quad Cortex published to this session; plugin ownership is not checked.",
      mimeType: "application/json"
    }, async (uri) => {
      const projected = projectAction("list_models", { query: null });
      return resourceJson(uri, await deviceResourceRead(() => route({ action: projected.action.name, method: projected.method, params: projected.params, origin: RESOURCE_READ_ORIGIN }, "qc:read")));
    });
    // Resources and `instructions` both need a cooperating client, and this
    // deployment's clients are the connector surfaces that drop one and never
    // fetch the other. A tool description is the only channel every client
    // reads, so the guide is a tool here as well as a resource.
    server.registerTool("get_qc_guide", {
      title: "QC Remote operating guide",
      description: "Read before the first Quad Cortex write of a conversation, and whenever a call is refused or a result is not read back. Covers which operations are unsaved versus written immediately, how to source every expected_* guard, how to tell a completed write reported as a failure from a real one, and which reference data is known to be wrong.",
      inputSchema: {},
      annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false }
    }, async () => ({ content: [{ type: "text" as const, text: SHARED_QC_ASSISTANT_GUIDE }] }));
    server.registerTool("get_qc_status", { title: "Quad Cortex status", description: "Read the paired Quad Cortex connection and readiness status.", inputSchema: {}, annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false } }, async () => deviceToolCall(async () => toolResult(await readStatus())));
    server.registerTool("get_qc_identity", { title: "Quad Cortex device information", description: "Read the connected Quad Cortex identity and reported firmware versions. Requires a host supporting remote device identity.", inputSchema: {}, annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false } }, async () => deviceToolCall(async () => toolResult(await route({ action: "get_qc_identity", method: "device.identity", params: {} }, "qc:read"))));
    server.registerTool("get_current_preset", { title: "Current preset", description: "Read the current Quad Cortex preset, scene, tempo, and Grid state.", inputSchema: {}, annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false } }, async () => deviceToolCall(async () => toolResult(await readCurrentPreset())));
    server.registerTool("get_remote_usage", { title: "Remote usage", description: "Read today's QC Remote Cloud command count. Cloudflare platform limits remain authoritative in the Cloudflare dashboard.", inputSchema: {}, annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false } }, async () => {
      if (!scopes.has("qc:read")) throw new Error("This tool requires the qc:read OAuth permission.");
      return toolResult(await rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, props.subject)).usage(props.accountGeneration ?? 0));
    });
    server.registerTool("list_qc_actions", { title: "QC Remote action catalog", description: "List every supported remote Quad Cortex action with its exact input schema, safety class, and required confirmations.", inputSchema: {}, annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false } }, async () => {
      if (!scopes.has("qc:read")) throw new Error("This tool requires the qc:read OAuth permission.");
      return toolResult(actionCatalog());
    });
    server.registerTool("execute_qc_action", {
      title: "Execute QC Remote action",
      description: "Execute one named action from list_qc_actions, whose catalog carries each action's safety class and per-action warnings. Read get_qc_guide before the first write of a conversation and get_current_preset before preset-scoped work. Every expected_* argument must come from a fresh read, never from what you asked for. A confirmation field is not an authorization boundary: this worker can only check that the boolean is true, not that a person set it, so describe the exact action to the user and get an answer before setting one.",
      inputSchema: {
        action: z.string().min(1).max(128), params: z.record(z.string(), z.unknown()),
        expectedBrokerTarget: z.object({
          source: z.enum(["physical", "demo"]),
          sessionId: z.string().min(1).max(256).refine(value => value.trim().length > 0),
          generation: z.number().int().min(1).max(Number.MAX_SAFE_INTEGER)
        }).strict().optional().describe("The brokerTarget from the client or UI's accepted status. Prevents an action from moving to another target while it is queued.")
      },
      annotations: { readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: false }
    }, async ({ action, params, expectedBrokerTarget }) => {
      const projected = projectAction(action, params);
      const requiredScope = projected.action.classification === "read" ? "qc:read" : "qc:control";
      try {
        const result = await route({ action: projected.action.name, method: projected.method, params: projected.params, ...(expectedBrokerTarget ? { expectedBrokerTarget } : {}) }, requiredScope);
        // Inputs with no gateway slot (list_models.query) are applied here,
        // exactly as both local MCP servers apply them.
        return toolResult(applyWorkerInputs(projected.action.name, params, result));
      } catch (reason) {
        const deviceError = deviceInvocationToolError(reason);
        if (deviceError) return deviceError;
        throw reason;
      }
    });
    server.registerTool("set_tempo", { title: "Set QC tempo", description: SHARED_SET_TEMPO.description, inputSchema: SHARED_SET_TEMPO.shape, annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false } }, async (params) => deviceToolCall(async () => {
      const projected = projectAction("set_tempo", params);
      return toolResult(await route({ action: projected.action.name, method: projected.method, params: projected.params }, "qc:control"));
    }));
    return server;
  }, { route: "/mcp", authContext: { props } });
}

/** The account fence an AI-app grant passes through (account-deletion.ts). */
function grantFence(env: Env) {
  const account = (subject: string) => rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, subject));
  return relayEnvironment(env.QCRC_ENVIRONMENT) === "production"
    ? { admit: (subject: string, admissionId: string) => account(subject).admitGrant(admissionId), settle: (subject: string, admissionId: string, generation: number) => account(subject).settleGrant(admissionId, generation) }
    : undefined;
}

const OAUTH_OPTIONS: OAuthProviderOptions<Env> = {
  apiRoute: "/mcp",
  apiHandler: { async fetch(request, env, ctx) {
    const props = (ctx as AuthenticatedContext).props;
    if (!props?.subject || !props.email || !props.name) return error("QC Remote identity is required", 401);
    // A grant from before a deletion of this account can reach nothing, and
    // is revoked here when it is used; requests during a deletion wait.
    const production = relayEnvironment(env.QCRC_ENVIRONMENT) === "production";
    // Prepare metadata without storing it; production records it inside the
    // same DO call that checks the account fence, saving one RPC per request.
    const presence = await observeMcpRequest(request, env, props, production).catch(() => undefined);
    if (production) {
      const admission = await rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, props.subject)).admitMcpRequest(props.accountGeneration ?? 0, presence);
      if (admission === "stale") {
        const grantId = accessTokenGrantId(bearer(request), props.subject);
        // Inert either way; a failed revoke is logged, and the account's next
        // deletion (or the grant's own expiry) removes it.
        if (grantId) await env.OAUTH_PROVIDER.revokeGrant(grantId, connectorGrantUser(props.subject)).catch((cause: unknown) => console.warn(JSON.stringify({ event: "stale-grant-revoke-failed", detail: cause instanceof Error ? cause.name : "unknown" })));
        return error(STALE_GENERATION_MESSAGE, 401);
      }
      if (admission === "deleting") return error(ACCOUNT_DELETION_PENDING_MESSAGE, 503);
    }
    // Admission records each authenticated client once; requests never renew it.
    return mcpHandler(env, props)(request, env, ctx);
  } },
  defaultHandler: { async fetch(request, env, ctx) { return await handleGoogleAuthorization(request, env, grantFence(env)) ?? baseHandler.fetch!(request, env, ctx); } },
  authorizeEndpoint: "/authorize",
  tokenEndpoint: "/token",
  clientRegistrationEndpoint: "/register",
  allowPlainPKCE: false,
  // Exactly the scopes something enforces, and exactly the scopes the
  // protected-resource metadata below advertises: both read one list.
  scopesSupported: [...QC_SCOPES],
  resourceMetadata: { resource: "https://qcrc.freevia.org/mcp", authorization_servers: ["https://qcrc.freevia.org"], scopes_supported: [...QC_SCOPES], bearer_methods_supported: ["header"], resource_name: "QC Remote Cloud" },
  clientIdMetadataDocumentEnabled: true,
  // Stated rather than inherited from the library's defaults (oauth-lifetimes.ts).
  ...OAUTH_TOKEN_LIFETIMES
};
const oauth = new OAuthProvider<Env>(OAUTH_OPTIONS);

// Claude Mobile currently probes and invokes custom MCP connectors at the
// origin even when the connector is configured with a path. Preserve `/mcp`
// as the canonical audience while accepting that origin-form request shape.
export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext) {
    const url = new URL(request.url);
    // Unauthenticated routes that write KV are limited per client address
    // before anything else runs, /register and /authorize included, which the
    // OAuth provider would otherwise answer itself (rate-limit.ts).
    const limited = await enforceRateLimit(request, env);
    if (limited) return limited;
    if (isMcpSetupPageRequest(request, url)) {
      return new Response(mcpSetupPage(url.origin), { headers: pairingPageHeaders() });
    }
    if (url.pathname === "/" && request.method === "POST") {
      url.pathname = "/mcp";
      request = new Request(url, request);
    }
    return oauth.fetch(request, providerEnv(env), ctx);
  }
};

/** The environment the OAuth provider runs with: in production its KV is
 * fenced so no grant or token for an account can be written once that
 * account's deletion has begun (account-deletion.ts, `fenceProviderKv`). */
function providerEnv(env: Env): Env {
  if (relayEnvironment(env.QCRC_ENVIRONMENT) !== "production") return env;
  const fenced = fenceProviderKv(env.OAUTH_KV as unknown as KVNamespace & { put: (key: string, value: unknown, options?: unknown) => Promise<unknown> }, (subject) => rpcSession(env, ownerSessionKey(env.QCRC_ENVIRONMENT, subject)));
  return fencedProviderEnv(env, fenced);
}

export class DeviceSession extends DurableObject<Env> {
  private pending = new Map<string, PendingCommand>();
  private lateResults = new LateResultLog<string>();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Read the log back before this session answers anything, so a result that
    // arrives after an eviction is still recognised as late rather than
    // silently dropped.
    initializeDeviceSessionState(
      () => this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong")),
      (callback) => this.ctx.blockConcurrencyWhile(async () => {
        await callback();
        await enrollProviderWritesRetention(this.ctx.storage, Date.now());
      }),
      this.ctx.storage,
      this.lateResults,
    );
  }

  /** Account-internal inventory only. This method performs storage reads and
   * returns aggregate retention metadata; it never repairs or mutates state.
   * Waking an object cannot suppress an independently due alarm. */
  async inspectRetentionStateForOperator(): Promise<DeviceSessionInventory> {
    return inspectDeviceSessionStorage(this.ctx.storage);
  }

  /** Account-internal legacy rollout only. The transaction reclassifies this
   * object and, only for a valid owner, makes the existing retention alarm due.
   * All cleanup remains in alarm(), with its existing deletion fences. */
  async scheduleLegacyRetentionForOperator(): Promise<DeviceSessionRetentionBackfillResult> {
    return requestLegacyRetentionBackfill(this.ctx.storage);
  }

  private async persistLateResults(): Promise<void> {
    await this.ctx.storage.put(DEVICE_SESSION_LATE_RESULTS_KEY, this.lateResults.snapshot());
  }

  /** The identity a deadline is recorded against.
   *
   * Written on upgrade, and minted here for any socket that predates this
   * field, because two sockets sharing one absent id would let a late result
   * from either be credited to the other. Every live socket therefore has a
   * distinct id before it can be the owner of anything. */
  private static connectionOf(socket: WebSocket): string | undefined {
    const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
    if (!attachment) return undefined;
    if (attachment.connectionId) return attachment.connectionId;
    const connectionId = crypto.randomUUID();
    socket.serializeAttachment({ ...attachment, connectionId } satisfies DeviceAttachment);
    return connectionId;
  }

  /** Called only from the authenticated MCP route; names are self-reported
   * client metadata, never inferred from the account holder's personal name. */
  async observeMcpClient(observation: McpObservation, generation?: number): Promise<void> {
    await this.refreshMcpPresence(observation, undefined, generation);
  }

  /** Called only from the authenticated connector revoke route, after the
   * grant was revoked: hosts get a fresh presence frame without it. */
  async forgetMcpGrant(grantId: string): Promise<void> {
    await this.refreshMcpPresence(undefined, grantId);
  }

  /** Sanitized MCP client registry for the owning account's connector roster.
   * Each OAuth client is recorded at first observation, not refreshed per request. */
  async mcpClientActivity(): Promise<McpClientPresence[]> {
    const now = Date.now();
    const rows = await this.ctx.storage.get<StoredMcpPresence[]>(MCP_PRESENCE_KEY) ?? [];
    return rememberedMcpPresence(rows, now);
  }

  private async refreshMcpPresence(observation?: McpObservation, forgetGrantId?: string, generation?: number): Promise<McpClientPresence[]> {
    await this.expireDeviceSockets();
    const now = Date.now();
    const rows = await this.ctx.storage.transaction(async (transaction) => {
      const stored = await transaction.get<StoredMcpPresence[]>(MCP_PRESENCE_KEY) ?? [];
      const previous = forgetGrantId ? forgetMcpGrant(stored, forgetGrantId) : stored;
      const next = updateMcpPresence(previous, observation, now, () => crypto.randomUUID());
      const changed = JSON.stringify(stored) !== JSON.stringify(next);
      // A repeated MCP request is strictly read-only: do not touch account
      // retention, alarms, or storage after recognizing its client record.
      if (observation && !changed) return next;
      // A newly observed client is account data, so reject it during deletion
      // before any write. Retention work is needed only on a state change.
      if (observation) await assertAccountWritable(transaction, generation);
      await scheduleProviderWritesRetention(transaction, now);
      if (changed && next.length > 0) await transaction.put(MCP_PRESENCE_KEY, next);
      else if (changed) await transaction.delete(MCP_PRESENCE_KEY);
      // One alarm serves socket credential expiry and device retention
      // (device-retention.ts): it is set for the earliest of them.
      const presenceExpiry = nextPresenceExpiry(next, now);
      const socketExpiry = this.nextDeviceExpiry();
      const retentionWork = await transaction.get<number>(DEVICE_SWEEP_KEY);
      const providerWritesWork = await transaction.get<number>(PROVIDER_WRITES_SWEEP_KEY);
      const deletionWork = accountDeletionDeadline(await readAccountDeletion(transaction));
      const expiry = earliestDeadline(presenceExpiry, socketExpiry, retentionWork, providerWritesWork, deletionWork);
      // An ordinary MCP request that finds a known client must be read-only.
      // Other callers use this path to schedule socket/device retention work.
      if (!observation) {
        if (expiry === null) await transaction.deleteAlarm();
        else await transaction.setAlarm(expiry);
      }
      return next;
    });
    const clients = publicMcpPresence(rows, now);
    const frame = mcpPresenceFrame(clients);
    if (frame === null) { console.warn("MCP presence frame exceeded the host limit"); return clients; }
    for (const socket of this.ctx.getWebSockets()) {
      const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
      if (socket.readyState !== 1 || attachment?.mcpPresence !== true) continue;
      try { socket.send(frame); } catch { /* A disconnected host clears its own watch. */ }
    }
    return clients;
  }

  async alarm(): Promise<void> {
    await dispatchDeviceSessionAlarm(this.ctx.storage, Date.now(), async () => {
      // A pending deletion owns the alarm; a receipt's reconciliation and
      // expiry run first and then the ordinary work reschedules.
      const deletion = await this.ctx.storage.transaction((transaction) => readAccountDeletion(transaction));
      const now = Date.now();
      // Re-arm first: a throw below must not leave a pending deletion with no alarm.
      if (deletion?.status === "pending") await this.ctx.storage.transaction(async (transaction) => {
        await scheduleAccountDeletionRetry(transaction, now, deletion);
      });
      if (deletion && await runAccountDeletionAlarm(this.ctx.storage, this.directoryKv(), this.grantRegistry(), this.providerKv(), now)) return;
      try { await sweepProviderWrites(this.ctx.storage, Date.now()); }
      catch { console.warn("Provider-write journal sweep did not complete; it is retried on the next alarm."); }
      await this.sweepRetainedDevices();
      return this.refreshMcpPresence();
    });
  }

  /** Delete device records past retention and their KV directory entries
   * The Durable Object half commits first; KV deletes that fail stay
   * queued and are retried on a later alarm. A failure here must not stop the
   * presence refresh that reschedules the alarm. */
  private async sweepRetainedDevices(): Promise<void> {
    try {
      const { deleted } = await sweepExpiredDevices(this.ctx.storage, Date.now());
      for (const socket of this.ctx.getWebSockets()) {
        const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
        if (attachment && deleted.includes(attachment.deviceId)) socket.close(4001, "Device credential expired; pair again");
      }
      await drainDirectoryCleanup(this.ctx.storage, this.env.QCRC_OAUTH as unknown as DirectoryKv);
    } catch {
      console.warn("Device retention sweep did not complete; it is retried on the next alarm.");
    }
  }

  private nextDeviceExpiry(): number | null {
    let next: number | null = null;
    for (const socket of this.ctx.getWebSockets()) {
      const expiry = (socket.deserializeAttachment() as DeviceAttachment | null)?.credentialExpiresAt;
      if (socket.readyState === 1 && typeof expiry === "number" && Number.isFinite(expiry) && expiry > Date.now()) {
        next = next === null ? expiry : Math.min(next, expiry);
      }
    }
    return next;
  }

  /** Old hibernated sockets acquire the expiry from their authenticated record. */
  private async deviceSocketValid(socket: WebSocket): Promise<boolean> {
    const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
    if (!attachment || socket.readyState !== 1) return false;
    // Re-read the authoritative record even for hibernated sockets. This
    // observes both migration to persistent credentials and explicit revokes.
    const record = await this.ctx.storage.get<DeviceRecord>(`device:${attachment.deviceId}`);
    if (!record || (typeof attachment.credentialHash === "string" && record.credentialHash !== attachment.credentialHash)) {
      this.failPending(socket, "This device was unpaired.");
      if (socket.readyState === 1) socket.close(1008, "This device was unpaired");
      return false;
    }
    const expiresAt = record.expiresAt === null ? null : record.expiresAt ?? record.pairedAt + CREDENTIAL_TTL_MS;
    if (expiresAt !== null && (!Number.isFinite(expiresAt) || expiresAt <= Date.now())) {
      this.failPending(socket, "The device host credential expired.");
      socket.close(4001, "Device credential expired; pair again");
      return false;
    }
    // A socket opened by the old release must reconnect once so the normal
    // authenticated connect migrates both its record and directory TTL. 1012
    // is retryable by hosts; it does not discard credentials or request login.
    if (expiresAt !== null || attachment.credentialExpiresAt !== null) {
      this.failPending(socket, "The device host is reconnecting to renew its saved authorization.");
      socket.close(1012, "Reconnect to update saved authorization");
      return false;
    }
    if (attachment.credentialExpiresAt !== expiresAt || !attachment.credentialHash) {
      socket.serializeAttachment({ ...attachment, credentialExpiresAt: expiresAt, credentialHash: record.credentialHash });
    }
    return true;
  }

  private async expireDeviceSockets(): Promise<void> {
    for (const socket of this.ctx.getWebSockets()) await this.deviceSocketValid(socket);
  }

  /** Pairing objects use a distinct, namespaced id in this existing class.
   * Durable Object storage transactions serialize claims across worker
   * isolates; KV remains only a recovery seed and status mirror. */
  async initializeHostPairingSession(session: HostPairingSession): Promise<boolean> {
    return initializeHostPairingState(this.ctx.storage, session);
  }

  async readHostPairingSession(candidate?: HostPairingSession) {
    return readHostPairingState(this.ctx.storage, candidate);
  }

  async claimHostPairingSession(candidate: HostPairingSession | null, claimId: string, now: number) {
    return claimHostPairingState(this.ctx.storage, candidate, claimId, now);
  }

  async completeHostPairingSession(claimId: string, pairingCode: string, pairing: PairingMetadata, now: number) {
    return completeHostPairingState(this.ctx.storage, claimId, pairingCode, pairing, now);
  }

  async cancelHostPairingSession(candidate: HostPairingSession | null, now: number) {
    return cancelHostPairingState(this.ctx.storage, candidate, now);
  }

  async pollHostPairingSession(candidate: HostPairingSession | null, receiverSecretHash: string, now: number) {
    return pollHostPairingState(this.ctx.storage, candidate, receiverSecretHash, now);
  }

  async createOffer(): Promise<PairOfferResult> {
    const code = randomToken(24);
    const offer: PairingOffer = { codeHash: await hash(code), expiresAt: Date.now() + OFFER_TTL_MS, redeemed: false };
    await this.ctx.storage.transaction(async (transaction) => {
      await assertAccountWritable(transaction);
      await transaction.put("pairing-offer", offer);
    });
    return { pairingCode: code, codeHash: offer.codeHash, expiresAt: offer.expiresAt, protocol: "qc-relay.v1" };
  }

  /** Whether `codeHash` is still this account's offer (the pairing callback's
   * check after writing its KV entry). */
  async offerCurrent(codeHash: string): Promise<boolean> {
    return (await this.ctx.storage.get<PairingOffer>("pairing-offer"))?.codeHash === codeHash;
  }

  // ---- Remote account deletion (account-deletion.ts) ----

  private directoryKv(): DirectoryKv { return this.env.QCRC_OAUTH as unknown as DirectoryKv; }

  /** The OAuth provider's grant API, built from the Worker's own options so
   * the alarm can revoke grants without a request. */
  private grantRegistry() { return getOAuthApi(OAUTH_OPTIONS, this.env); }

  /** The provider's KV, unfenced: the deletion only deletes and lists there. */
  private providerKv(): ProviderKv { return this.env.OAUTH_KV as unknown as ProviderKv; }

  /** Start (or rejoin) and run this account's deletion. Device credentials
   * stop working at the first step, so every socket is closed then. */
  async requestAccountDeletion(subject: string): Promise<AccountDeletionOutcome> {
    if (!subject || ownerSessionKey(this.env.QCRC_ENVIRONMENT, subject) === OWNER) throw new Error("Remote account deletion needs a production account");
    await beginAccountDeletion(this.ctx.storage, subject, Date.now());
    this.closeAllSockets();
    return (await runAccountDeletion(this.ctx.storage, this.directoryKv(), this.grantRegistry(), this.providerKv(), Date.now()))!;
  }

  /** Continue exactly the pending operation `operationId`; never starts one. */
  async continueAccountDeletion(operationId: string): Promise<AccountDeletionOutcome | null> {
    const state = await this.ctx.storage.transaction((transaction) => readAccountDeletion(transaction));
    if ((state?.status !== "pending" && state?.status !== "reconciling") || state.operationId !== operationId) return null;
    this.closeAllSockets();
    return runAccountDeletion(this.ctx.storage, this.directoryKv(), this.grantRegistry(), this.providerKv(), Date.now());
  }

  async accountDeletionStatus(): Promise<PublicDeletionStatus> {
    return publicDeletionStatus(await this.ctx.storage.transaction((transaction) => readAccountDeletion(transaction)));
  }

  async admitGrant(admissionId: string) { return admitGrant(this.ctx.storage, admissionId, Date.now()); }
  async settleGrant(admissionId: string, generation: number) { return settleGrant(this.ctx.storage, admissionId, generation); }
  async admitMcpRequest(generation: unknown, observation?: McpObservation) {
    const admission = await admitMcpRequest(this.ctx.storage, generation);
    if (admission === "ok" && observation) {
      // Presence remains best-effort telemetry. Its own transaction repeats
      // the generation/deletion check if deletion starts after admission.
      try { await this.observeMcpClient(observation, typeof generation === "number" ? generation : 0); }
      catch { /* Never turn failed presence tracking into failed authentication. */ }
    }
    return admission;
  }
  async admitProviderWrite(key: string, generation: number | undefined, expiresAt: number | undefined) { return admitProviderWrite(this.ctx.storage, key, generation, expiresAt, Date.now()); }
  async settleProviderWrite(admissionId: string) { return settleProviderWrite(this.ctx.storage, admissionId); }
  async closeCompensation(admissionId: string, now: number) { return closeCompensation(this.ctx.storage, admissionId, now); }
  async markProviderDelete(key: string) { return markProviderDelete(this.ctx.storage, key, Date.now()); }

  private closeAllSockets(): void {
    for (const socket of this.ctx.getWebSockets()) {
      this.failPending(socket, "This remote account was deleted.");
      try { socket.close(4003, "Remote account deleted"); } catch { /* already closed */ }
    }
    this.lateResults = new LateResultLog<string>();
  }

  /** `deviceId` is minted by the worker, which writes its KV directory entry
   * before calling here (device-retention.ts `completeDevicePairing`). */
  async pair(pairingCode: string, deviceName: string, pairing: PairingMetadata | undefined, deviceId: string, previous?: PreviousCredential, operationId?: string): Promise<PairReceipt> {
    const codeHash = await hash(pairingCode);
    const name = deviceName.trim().slice(0, 80);
    if (!name) throw new Error("A device name is required");
    const credential = randomToken();
    const pairedAt = Date.now();
    const record: DeviceRecord = { credentialHash: await hash(credential), name, pairedAt, expiresAt: relayProfile.credentialTtlDays === 0 ? null : pairedAt + relayProfile.credentialTtlDays * 24 * 60 * 60_000, lastUsedAt: pairedAt, identityProvider: "google", pairing };
    const previousDevice = previous ? { deviceId: previous.deviceId, credentialHash: await hash(previous.credential) } : undefined;
    // The offer is validated inside the transaction that consumes it, so the
    // crypto awaits above cannot let two requests redeem one code. A proven
    // previous record is only marked for replacement: it keeps working, and
    // its sockets stay open, until the new credential connects.
    let previousReplacementPending: boolean;
    try { ({ previousReplacementPending } = await redeemPairingOffer(this.ctx.storage, { codeHash, deviceId, operationId, record, previous: previousDevice, now: pairedAt, guard: (transaction) => this.accountWriteGuard(transaction) }, constantTimeEqual)); }
    catch (reason) {
      // The worker wrote this id's KV entry before calling here; a refusal
      // by the deletion fence queues it so it cannot outlive the account.
      if (reason instanceof Error && reason.message === ACCOUNT_DELETION_PENDING_MESSAGE) {
        await queueRefusedDirectoryEntry(this.ctx.storage, deviceId, pairedAt).catch(() => {});
        throw new Error("Pairing offer is invalid or expired");
      }
      throw reason;
    }
    return { deviceId, deviceCredential: credential, protocol: "qc-relay.v1", identityProvider: "google", pairing, ...(previous ? { previousDeviceReplacementPending: previousReplacementPending } : {}) };
  }

  /** Admit one pair-route attempt before its KV directory entry is written:
   * only against this account's live offer for this code, and within the
   * account's caps (device-retention.ts `admitPairingAttempt`). */
  async admitPairing(operationId: string, deviceId: string, pairingCode: string): Promise<PairingAdmission> {
    return admitPairingAttempt(this.ctx.storage, { operationId, deviceId, codeHash: await hash(pairingCode), now: Date.now() }, constantTimeEqual);
  }

  /** Decide a failed `pair()` call from committed state; only `aborted` lets
   * the worker delete the directory entry (device-retention.ts). */
  async resolvePairing(operationId: string, deviceId: string, pairingCode: string): Promise<PairingResolution> {
    return resolvePairingAttempt(this.ctx.storage, operationId, deviceId, Date.now(), { codeHash: await hash(pairingCode), equal: constantTimeEqual });
  }

  /** Refuses account writes that must not commit; called inside the
   * transaction that would commit them. The account-deletion fence plugs in
   * here; with no deletion state it allows everything. */
  protected async accountWriteGuard(transaction: RetentionTransaction): Promise<AccountWriteDenial | null> { return accountDeletionDenial(transaction); }

  /** Close the sockets of the exact predecessor a connect admission deleted,
   * and try its queued KV cleanup once (the alarm retries a failure). */
  /** Synchronous on purpose: the connect path calls this between its
   * admission decision and acceptWebSocket, where no await may sit. The
   * predecessor's KV entry was queued in the admission transaction, and its
   * alarm retries it; the connect only nudges a drain after acceptance. */
  private closeReplacedDevice(revokedDeviceId: string): void {
    for (const socket of this.ctx.getWebSockets()) {
      const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
      if (attachment?.deviceId !== revokedDeviceId) continue;
      this.failPending(socket, "This device was paired again.");
      socket.close(1008, "This device was paired again");
    }
  }

  async status(): Promise<Json> {
    const devices = this.ctx.getWebSockets().filter((socket) => socket.readyState === 1).map((socket) => socket.deserializeAttachment() as DeviceAttachment).filter(Boolean);
    // A non-empty `lateResults` means a caller was told the device did not
    // respond and the device then answered, so a deadline here is still shorter
    // than the work it fronts. Reporting it is the only way that is visible.
    const lateResults = this.lateResults.arrivals();
    return {
      connected: devices.filter((item) => item.ready).length,
      devices: devices.map(({ deviceId, deviceName, ready, usbConnected, brokerTarget }) => ({ deviceId, deviceName, ready, ...(usbConnected !== undefined ? { usbConnected } : {}), ...(brokerTarget ? { brokerTarget } : {}) })),
      ...(lateResults.length > 0 ? { lateResults } : {})
    };
  }

  async usage(generation?: number): Promise<Json> {
    if (generation !== undefined) await this.ctx.storage.transaction((transaction) => assertAccountWritable(transaction, generation));
    const usage = await this.ctx.storage.get<DailyUsage>("usage:daily");
    const day = new Date().toISOString().slice(0, 10);
    return { day, commands: usage?.day === day ? usage.commands : 0, lastCommandAt: usage?.day === day ? usage.lastCommandAt : undefined };
  }

  /** The single place a device credential is checked. Expiry lives here rather
   * than at each call site so a new entry point cannot forget it. */
  private async authenticateDevice(deviceId: string, credential: string): Promise<DeviceRecord | null> {
    const record = await this.ctx.storage.get<DeviceRecord>(`device:${deviceId}`);
    if (!record || !await constantTimeEqual(await hash(credential), record.credentialHash)) return null;
    const now = Date.now();
    if (record.expiresAt !== null && (record.expiresAt ?? record.pairedAt + CREDENTIAL_TTL_MS) <= now) return null;
    // Migrate valid legacy credentials, then update activity at most daily.
    // The transaction never restores a revoked or replaced record.
    try { await touchDeviceLastUse(this.ctx.storage, deviceId, record.credentialHash, now); record.expiresAt = null; }
    catch { /* A missed write only shortens retention toward its pairing-time floor. */ }
    return record;
  }

  async authorizeDevice(deviceId: string, credential: string): Promise<boolean> {
    return Boolean(await this.authenticateDevice(deviceId, credential));
  }

  async usageForDevice(deviceId: string, credential: string): Promise<Json> {
    if (!await this.authenticateDevice(deviceId, credential)) throw new Error("Invalid device credential");
    return this.usage();
  }

  /** Every device on this account, as one of those devices may see it.
   *
   * Sorted newest pairing first so a device just added is the one at the top
   * of the list that prompted it. `current` marks the caller, because a roster
   * with three similar names on it is otherwise unreadable: the panel needs to
   * know which row is the machine the person is looking at.
   *
   * Expiry is reported rather than filtered. A credential that lapsed is why
   * remote access stopped working, and dropping the row would answer that
   * question by showing nothing at all. */
  async rosterForDevice(deviceId: string, credential: string): Promise<Json> {
    if (!await this.authenticateDevice(deviceId, credential)) throw new Error("Invalid device credential");
    const now = Date.now();
    const live = new Set(this.ctx.getWebSockets()
      .map((socket) => (socket.deserializeAttachment() as DeviceAttachment | null)?.deviceId)
      .filter((value): value is string => typeof value === "string"));
    const stored = await this.ctx.storage.list<DeviceRecord>({ prefix: "device:" });
    const devices = [...stored].map(([key, record]) => {
      const id = key.slice("device:".length);
      const expiresAt = record.expiresAt === null ? null : record.expiresAt ?? record.pairedAt + CREDENTIAL_TTL_MS;
      return {
        deviceId: id,
        name: record.name,
        pairedAt: record.pairedAt,
        expiresAt: expiresAt ?? 0,
        expired: expiresAt !== null && expiresAt <= now,
        connected: live.has(id),
        current: id === deviceId,
        // A re-pair whose new credential has not connected yet: the record it
        // replaces is still listed and still works until then.
        ...(record.replaces ? { replacementPending: true, replacesDeviceId: record.replaces.deviceId } : {}),
        ...(record.pairing ? { email: record.pairing.email, signedInFrom: record.pairing.device } : {})
      };
    });
    devices.sort((left, right) => right.pairedAt - left.pairedAt);
    return { devices };
  }

  /** Withdraw another device's credential on behalf of a device of the same
   * account. The caller proves the account by holding a live credential for
   * it; the entry point above proves both ids belong to that one account. */
  async revokeSibling(deviceId: string, credential: string, target: string): Promise<RevocationOutcome> {
    if (!await this.authenticateDevice(deviceId, credential)) throw new Error("Invalid device credential");
    if (!await this.ctx.storage.get<DeviceRecord>(`device:${target}`)) return { revoked: false, remaining: await this.deviceCount() };
    await this.ctx.storage.delete(`device:${target}`);
    for (const socket of this.ctx.getWebSockets()) {
      const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
      if (attachment?.deviceId === target) socket.close(1008, "This device was unpaired");
    }
    return { revoked: true, remaining: await this.deviceCount() };
  }

  /** Devices still paired to this account. Counted after a removal, so the
   * request handler can tell an account that has just emptied from one that
   * still holds a computer the apps on it can reach. */
  async deviceCount(): Promise<number> {
    // Expired records stay for the retention grace period so a roster can say
    // why access stopped, but no AI app can reach them: they do not count.
    return liveDeviceCount((await this.ctx.storage.list<DeviceRecord>({ prefix: "device:" })).values(), CREDENTIAL_TTL_MS, Date.now());
  }

  /** What the operator can be told about one paired device.
   *
   * Never the credential hash, and never anything the owner did not already
   * see on the pairing page. Enough to answer "which account holds this
   * device", which is the question the admin routes could not answer at all. */
  async describeDevice(deviceId: string): Promise<Json | null> {
    const record = await this.ctx.storage.get<DeviceRecord>(`device:${deviceId}`);
    if (!record) return null;
    return {
      name: record.name,
      pairedAt: record.pairedAt,
      expiresAt: record.expiresAt === null ? 0 : record.expiresAt ?? record.pairedAt + CREDENTIAL_TTL_MS,
      identityProvider: record.identityProvider,
      ...(record.replaces ? { replacementPending: true, replacesDeviceId: record.replaces.deviceId } : {}),
      ...(record.pairing ? { email: record.pairing.email, device: record.pairing.device } : {}),
      connected: this.ctx.getWebSockets().some((socket) => {
        const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
        return attachment?.deviceId === deviceId;
      })
    };
  }

  /** Withdraw a credential. The host calls this when the user unpairs, so
   * unpairing stops being a local delete of a secret the relay still honours. */
  async revokeDevice(deviceId: string, credential: string): Promise<RevocationOutcome> {
    if (!await this.authenticateDevice(deviceId, credential)) return { revoked: false, remaining: await this.deviceCount() };
    await this.ctx.storage.delete(`device:${deviceId}`);
    for (const socket of this.ctx.getWebSockets()) {
      const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
      if (attachment?.deviceId === deviceId) socket.close(1008, "This device was unpaired");
    }
    return { revoked: true, remaining: await this.deviceCount() };
  }

  async command(payload: Json, generation?: number): Promise<Json> {
    // An AI-app grant from before a deletion of this account, or any grant
    // while one is pending, is refused before a host is even chosen.
    if (generation !== undefined) await this.ctx.storage.transaction((transaction) => assertAccountWritable(transaction, generation));
    await this.expireDeviceSockets();
    const method = typeof payload.method === "string" ? payload.method : "";
    const params = typeof payload.params === "object" && payload.params !== null && !Array.isArray(payload.params) ? payload.params as Json : {};
    const action = typeof payload.action === "string" ? payload.action : method;
    // Set only by this worker's own MCP resource callbacks; any other value is
    // dropped, so a command is never narrowed by accident nor widened at all.
    const origin: InvokeOrigin | undefined = payload.origin === RESOURCE_READ_ORIGIN ? RESOURCE_READ_ORIGIN : undefined;
    if (!method) throw new Error("A device method is required");
    const socket = selectDeviceHost(this.ctx.getWebSockets(), method);
    const observedBrokerTarget = (socket.deserializeAttachment() as DeviceAttachment).brokerTarget;
    const acceptedBrokerTarget = parseBrokerTarget(payload.expectedBrokerTarget);
    if (acceptedBrokerTarget) assertBrokerTarget(acceptedBrokerTarget, observedBrokerTarget);
    const expectedBrokerTarget = acceptedBrokerTarget ?? observedBrokerTarget;
    const day = new Date().toISOString().slice(0, 10);
    // A daily activity marker is enough for the owner-facing usage display.
    // Updating it for every command would turn normal use into storage-write
    // amplification, so it is created at most once per UTC day.
    await this.ctx.storage.transaction(async (transaction) => {
      await assertAccountWritable(transaction, generation);
      const current = await transaction.get<DailyUsage>("usage:daily");
      if (current?.day !== day) {
        await transaction.put("usage:daily", { day, commands: 1, lastCommandAt: Date.now() } satisfies DailyUsage);
      }
    });
    // Storage may yield to a host disconnect. Recheck the selected socket;
    // never move the caller's operation to another connected host.
    if (!this.ctx.getWebSockets().includes(socket)) throw new Error("The selected device host disconnected before dispatch");
    selectDeviceHost([socket], method);
    let request = params;
    if (isVersionSensitive(method)) {
      // This worker is generated from one gateway API version and every paired
      // host runs whatever build is installed on it. Send an older host only
      // what it accepts, and check here any guard it cannot receive.
      const adapted = adaptForHost(method, params, await this.hostGatewayApiVersion(socket, expectedBrokerTarget));
      request = adapted.params;
      if (adapted.modelGuard) {
        const details = await this.dispatch(socket, action, "device.blockDetails", adapted.modelGuard.detailsParams, origin, expectedBrokerTarget);
        const failure = legacyModelGuardFailure(adapted.modelGuard, details);
        if (failure) throw new Error(failure);
      }
      if (!this.ctx.getWebSockets().includes(socket)) throw new Error("The selected device host disconnected before dispatch");
    }
    return this.dispatch(socket, action, method, request, origin, expectedBrokerTarget);
  }

  /** The gateway API version the host behind `socket` reports, asked once per connection. */
  private async hostGatewayApiVersion(socket: WebSocket, expectedBrokerTarget?: BrokerTarget): Promise<number> {
    const known = (socket.deserializeAttachment() as DeviceAttachment | null)?.gatewayApiVersion;
    if (typeof known === "number") return known;
    const version = reportedGatewayApiVersion(await this.dispatch(socket, "get_status", "system.status", {}, undefined, expectedBrokerTarget));
    const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
    if (attachment) socket.serializeAttachment({ ...attachment, gatewayApiVersion: version });
    return version;
  }

  /** Send one request to one host and wait for its answer. */
  private async dispatch(socket: WebSocket, action: string, method: string, params: Json, origin?: InvokeOrigin, expectedBrokerTarget?: BrokerTarget): Promise<Json> {
    if (!await this.deviceSocketValid(socket)) throw new Error("The selected device host credential expired or disconnected before dispatch");
    assertBrokerTarget(expectedBrokerTarget, (socket.deserializeAttachment() as DeviceAttachment | null)?.brokerTarget);
    // The deadline is the one this method's own local verification needs. A
    // single shared number expired every long workflow early and reported it as
    // a device that had not answered.
    const gatewayApiVersion = (socket.deserializeAttachment() as DeviceAttachment | null)?.gatewayApiVersion;
    const deadline = commandDeadlineMs(method, gatewayApiVersion);
    const requestId = crypto.randomUUID();
    const response = new Promise<Json>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(requestId);
        this.lateResults.abandon(requestId, method, deadline, Date.now(), DeviceSession.connectionOf(socket));
        void this.persistLateResults().catch(() => console.warn("Could not persist late device-result evidence"));
        reject(new Error(deadlineExceededMessage(method, gatewayApiVersion)));
      }, deadline);
      this.pending.set(requestId, { socket, method, expectedBrokerTarget, resolve, reject, timeout });
    });
    try {
      // Tell the host how long this call has. `innerBudgetMs` is the host's own
      // share of the deadline, without the transport margin the worker keeps for
      // itself, so a host that honours it always answers inside `deadline`.
      socket.send(JSON.stringify({ type: "invoke", id: requestId, action, method, params, deadlineMs: innerBudgetMs(method), ...(origin ? { origin } : {}), ...(expectedBrokerTarget ? { expectedBrokerTarget } : {}) }));
    } catch {
      const pending = this.pending.get(requestId)!;
      clearTimeout(pending.timeout);
      this.pending.delete(requestId);
      pending.reject(new Error(`The device host send failed for ${method}; read current device state before repeating the operation.`));
    }
    return response;
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade") !== "websocket") return error("Expected a WebSocket upgrade", 426);
    const url = new URL(request.url);
    const deviceId = url.searchParams.get("deviceId");
    const credential = bearer(request);
    if (!deviceId || !credential) return error("Device credentials are required", 401);
    const record = await this.authenticateDevice(deviceId, credential);
    if (!record) return error("Invalid or expired device credential", 401);
    // Fail closed: a revoke, re-pair or deletion that committed after the
    // authentication above denies the connect, and a storage failure is
    // retryable; neither upgrades. Checked in one transaction.
    let admission: ConnectAdmission;
    try { admission = await adoptReplacement(this.ctx.storage, { deviceId, credentialHash: record.credentialHash, now: Date.now(), credentialTtlMs: CREDENTIAL_TTL_MS, guard: (transaction) => this.accountWriteGuard(transaction) }); }
    catch { return error("The relay could not admit this device right now; try again", 503); }
    if (!admission.admitted) return error("Invalid or expired device credential", 401);
    // No await between the admission decision above and acceptWebSocket
    // below: a revoke that runs after this point finds the accepted socket and
    // closes it by device id, and every later message or dispatch re-checks
    // the record against `credentialHash` (deviceSocketValid).
    if (admission.revokedDeviceId) this.closeReplacedDevice(admission.revokedDeviceId);
    const replacedDeviceId = admission.replacedDeviceId;
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    server.serializeAttachment({ deviceId, deviceName: record.name, ready: false, connectionId: crypto.randomUUID(), credentialExpiresAt: record.expiresAt === null ? null : record.expiresAt ?? record.pairedAt + CREDENTIAL_TTL_MS, credentialHash: record.credentialHash } satisfies DeviceAttachment);
    this.ctx.acceptWebSocket(server);
    if (admission.revokedDeviceId) void drainDirectoryCleanup(this.ctx.storage, this.env.QCRC_OAUTH as unknown as DirectoryKv).catch(() => {});
    // Opt-in only: hosts built before this frame treat an unknown frame type
    // as a protocol error. Sent only for an admitted connect, naming exactly
    // the predecessor the committed transaction reported, on every connect of
    // the new record, so a lost first one is repeated.
    const ack = url.searchParams.get(REPLACEMENT_ACK_PARAM) === "1" ? replacementAckFrame(replacedDeviceId) : null;
    if (ack) server.send(ack);
    await this.refreshMcpPresence();
    // The Rust relay client verifies that the server explicitly selects the
    // generated protocol version. An upgrade without this response header is
    // intentionally rejected as an invalid endpoint.
    return new Response(null, { status: 101, webSocket: client, headers: { "Sec-WebSocket-Protocol": "qc-relay.v1" } });
  }

  async webSocketMessage(socket: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (!await this.deviceSocketValid(socket)) return;
    let parsed: Json;
    try { parsed = decodeDeviceMessage(message); } catch { socket.close(1003, "Invalid message"); return; }
    const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
    if (!attachment || typeof parsed.type !== "string") return;
    if (parsed.type === "ready") {
      const reported = typeof parsed.gatewayApiVersion === "number" && Number.isInteger(parsed.gatewayApiVersion)
        ? { gatewayApiVersion: parsed.gatewayApiVersion }
        : {};
      let readiness;
      try { readiness = deviceReadiness(parsed); } catch { socket.close(1003, "Invalid readiness"); return; }
      const next = { ...attachment, ...reported, ...readiness, brokerTarget: readiness.brokerTarget, mcpPresence: parsed.mcpPresence === true };
      if (next.ready !== attachment.ready || next.usbConnected !== attachment.usbConnected || !sameBrokerTarget(next.brokerTarget, attachment.brokerTarget) || next.mcpPresence !== attachment.mcpPresence || next.gatewayApiVersion !== attachment.gatewayApiVersion) {
        socket.serializeAttachment(next);
      }
      // Readiness is event-driven; unchanged frames require no attachment write. Send its initial
      // snapshot only on opt-in; activity and alarms publish later changes.
      if (parsed.mcpPresence === true && attachment.mcpPresence !== true) await this.refreshMcpPresence();
      return;
    }
    if (parsed.type === "result" && typeof parsed.id === "string") {
      const pending = this.pending.get(parsed.id);
      if (!pending) {
        // Not pending means either a duplicate reply or, far more importantly, a
        // result for a request this worker already gave up on. Discarding the
        // second case is how "the device did not respond" survived being
        // disproved by the device itself.
        const late = this.lateResults.observe(parsed.id, parsed.ok === true, Date.now(), DeviceSession.connectionOf(socket));
        if (late) {
          await this.persistLateResults();
          console.warn(
            `late device result: ${late.method} answered ${late.arrivedAfterDeadlineMs}ms after its ${late.deadlineMs}ms deadline (ok=${late.ok}). The caller was told the device did not respond; it did.`
          );
        }
        return;
      }
      if (pending.socket !== socket) return;
      clearTimeout(pending.timeout);
      this.pending.delete(parsed.id);
      if (!sameBrokerTarget(pending.expectedBrokerTarget, attachment.brokerTarget)
          && !(pending.expectedBrokerTarget && parsed.ok === true
            && verifiedTargetTransition(pending.method, pending.expectedBrokerTarget, attachment.brokerTarget, parsed.result))) {
        pending.reject(new Error("The selected broker target changed while the operation was pending; inspect current state before repeating it."));
        return;
      }
      if (parsed.ok === true) pending.resolve(sanitizePublicResult(typeof parsed.result === "object" && parsed.result !== null ? parsed.result as Json : { result: parsed.result }));
      else {
        const failure = parsed.error as Json | undefined;
        pending.reject(deviceInvocationError(failure?.code, failure?.message, { retryable: failure?.retryable, details: failure?.details }));
      }
    }
  }

  async webSocketClose(socket: WebSocket): Promise<void> {
    const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
    if (attachment) socket.serializeAttachment({ ...attachment, ready: false });
    this.failPending(socket, "The device host disconnected before the Quad Cortex answered.");
  }

  async webSocketError(socket: WebSocket): Promise<void> {
    const attachment = socket.deserializeAttachment() as DeviceAttachment | null;
    if (attachment) socket.serializeAttachment({ ...attachment, ready: false });
    this.failPending(socket, "The connection to the device host failed before the Quad Cortex answered.");
  }

  /** A host that goes away owes no further answer. Deadlines are now as long as
   * the local verification they front, so waiting one out after the host has
   * gone would hold the caller for minutes to report a fact already known — and
   * would make a disconnect indistinguishable from a slow device. */
  private failPending(socket: WebSocket, detail: string): void {
    for (const [requestId, pending] of this.pending) {
      if (pending.socket !== socket) continue;
      clearTimeout(pending.timeout);
      this.pending.delete(requestId);
      pending.reject(new Error(`${detail} ${pending.method} was in flight, so its outcome is unknown: read the device's current state before repeating it.`));
    }
  }
}

/** Named service-binding entrypoint for a local operator harness. It has no
 * HTTP route and deliberately accepts only opaque ids from the account-level
 * Durable Object inventory API. */
export class DeviceSessionInventoryEntrypoint extends WorkerEntrypoint<Env> {
  async inspect(ids: string[]): Promise<Array<{ id: string; inventory: DeviceSessionInventory }>> {
    return inspectDeviceSessionIds(ids, async (id) => {
      const objectId = this.env.DEVICE_SESSION.idFromString(id);
      return this.env.DEVICE_SESSION.get(objectId).inspectRetentionStateForOperator();
    });
  }
}

/** Separate account-internal mutation capability. Deploying this named RPC
 * does not invoke it, and the default HTTP handler never routes to it. */
export class DeviceSessionRetentionEntrypoint extends WorkerEntrypoint<Env> {
  async provenance(): Promise<{ schemaVersion: 1; worker: "qc-remote-cloud"; deploymentId: string; deploymentTag: string; deployedAt: string }> {
    const metadata = this.env.QCRC_VERSION_METADATA;
    return {
      schemaVersion: 1,
      worker: "qc-remote-cloud",
      deploymentId: metadata.id,
      deploymentTag: metadata.tag,
      deployedAt: metadata.timestamp,
    };
  }

  async scheduleReviewedRetentionV1(request: {
    schemaVersion: 1;
    ids: string[];
    expectedDeployment: { id: string; tag: string; timestamp: string };
  }): Promise<Array<{ id: string; backfill: DeviceSessionRetentionBackfillResult }>> {
    if (!request || request.schemaVersion !== 1 || !request.expectedDeployment) {
      throw new Error("Retention scheduling refused because the reviewed request envelope is invalid.");
    }
    const metadata = this.env.QCRC_VERSION_METADATA;
    if (metadata.id !== request.expectedDeployment.id || metadata.tag !== request.expectedDeployment.tag || metadata.timestamp !== request.expectedDeployment.timestamp) {
      throw new Error("Retention scheduling refused because the active deployment changed.");
    }
    return backfillDeviceSessionIds(request.ids, async (id) => {
      const objectId = this.env.DEVICE_SESSION.idFromString(id);
      return this.env.DEVICE_SESSION.get(objectId).scheduleLegacyRetentionForOperator();
    });
  }
}

function accountBeingDeletedPage(): Response {
  return new Response(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Account being deleted</title><style>${pairingStyles()}</style></head><body><main><h1>This remote account is being deleted</h1><p>A deletion of this Google account's QC Remote remote account is in progress, so nothing can be paired to it right now. Open <a href="${ACCOUNT_DELETION_WEB_PATH}/status">the deletion status</a> in the browser you started it from, or try again later.</p></main></body></html>`, { status: 409, headers: pairingPageHeaders() });
}

function landingPage(): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>QC Remote</title><style>${pairingStyles()}</style></head><body><main><h1>Remote access for your Quad Cortex</h1><p>QC Remote links an AI app to a Quad Cortex already connected by USB to your computer or phone. The device host makes an outbound encrypted connection; no inbound port is opened.</p><p class="help">Open <b>REMOTE</b> in QC Remote to pair a device. After pairing, use the MCP address to add it to a compatible AI app.</p></main></body></html>`;
}

/** Every page here refuses framing, as the consent page does (http-security.ts). */
function pairingPageHeaders(): HeadersInit {
  return htmlPageHeaders();
}

function pairingStyles(): string {
  return `:root{color-scheme:dark}*{box-sizing:border-box}body{margin:0;background:${QC_COLORS.app.canvas};color:${QC_COLORS.app.text};font:16px ${QC_TYPOGRAPHY.app};display:grid;min-height:100vh;place-items:center}main{width:min(38rem,100%);padding:2rem}h1{margin:.4rem 0 .75rem;font-size:2rem;line-height:1.15}h2{margin:0;font-size:1rem}p{color:${QC_COLORS.app.textMuted};line-height:1.55}.badge{color:${QC_COLORS.app.success};font-weight:700;font-size:.78rem;letter-spacing:.1em}.account,.step{padding:.8rem 1rem;border:1px solid ${QC_COLORS.app.border};border-radius:.55rem;background:${QC_COLORS.app.panelRaised};color:${QC_COLORS.app.text};overflow-wrap:anywhere}.steps{display:grid;gap:.6rem;margin:1.25rem 0}.step{display:grid;grid-template-columns:1.6rem 1fr;gap:.55rem}.step b{color:${QC_COLORS.app.success}}.actions{display:grid;gap:.65rem;margin:1.25rem 0}.button,button{display:block;width:100%;padding:.78rem 1rem;border:1px solid ${QC_COLORS.app.border};border-radius:.55rem;background:${QC_COLORS.app.panelRaised};color:${QC_COLORS.app.text};font:700 .95rem ${QC_TYPOGRAPHY.app};text-align:center;text-decoration:none;cursor:pointer}.button.primary,button.primary{border-color:${QC_COLORS.app.success};background:${QC_COLORS.app.success};color:${QC_COLORS.app.canvas}}.help{font-size:.88rem}.client-list{display:grid;margin:1.25rem 0;border-top:1px solid ${QC_COLORS.app.border}}.client{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:1rem;align-items:center;padding:.85rem 0;border-bottom:1px solid ${QC_COLORS.app.border}}.client-heading{display:flex;align-items:center;gap:.55rem}.client-heading span{color:${QC_COLORS.app.info};font-size:.72rem;font-weight:700;letter-spacing:.08em;text-transform:uppercase;white-space:nowrap}.client.supported .client-heading span{color:${QC_COLORS.app.success}}.client.custom .client-heading span{color:${QC_COLORS.app.info}}.client p{margin:.3rem 0 0;font-size:.86rem;line-height:1.4}.provider-action{display:block;padding:.62rem .78rem;border:1px solid ${QC_COLORS.app.success};border-radius:.5rem;background:${QC_COLORS.app.success};color:${QC_COLORS.app.canvas};font:700 .86rem ${QC_TYPOGRAPHY.app};text-align:center;text-decoration:none;cursor:pointer}.provider-action.endpoint{max-width:15rem;overflow-wrap:anywhere;text-align:left}.server-address{display:block;width:100%;margin:.45rem 0 0;padding:.7rem;border:1px solid ${QC_COLORS.app.border};border-radius:.45rem;background:${QC_COLORS.app.canvas};color:${QC_COLORS.app.text};font:.88rem ${QC_TYPOGRAPHY.mono}}small{color:${QC_COLORS.app.textMuted};line-height:1.45}@media(max-width:30rem){main{padding:1.25rem}.client{grid-template-columns:1fr;gap:.6rem}.provider-action{justify-self:start}}`;
}

/** The manual pairing path: someone who opened /pair with no session.
 *
 * A pairing link carries a session and opens the confirmation page in
 * host-pairing.ts instead. What remains here is the person who typed the
 * address, who has no session to carry and is holding a pairing code from the
 * app instead. */
function pairingWelcomePage(): string {
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Pair device</title><style>${pairingStyles()}</style></head><body><main><h1>Pair device</h1><p>Sign in with the Google account that will own this Quad Cortex.</p><form method="post" action="/pair/sign-in"><button class="button primary" type="submit">Continue with Google</button></form></main></body></html>`;
}

function pairingPage(code: string, expiresAt: number, email: string): string {
  const expiry = new Date(expiresAt).toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit" });
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Pair device</title><style>${pairingStyles()}</style></head><body><main><h1>Enter this code in QC Remote</h1><p>Signed in as <strong>${escapeHtml(email)}</strong>. On the device host, open <b>REMOTE</b> and enter this one-time code.</p><input class="server-address" value="${code}" readonly aria-label="One-time pairing code" onfocus="this.select()"><p class="help">Expires at ${expiry}. This links this device host to your account. It does not connect an AI app yet.</p></main></body></html>`;
}

/** What the browser sees once the computer has been claimed.
 *
 * The account is the headline, not a footnote: this page is the only place the
 * person sees which Google account now owns the rig, and picking the wrong one
 * is the failure that otherwise surfaces much later as "no paired device is
 * connected to this account".
 *
 * There is deliberately no Unpair or Switch account control here. Both are
 * honest-looking and neither can work from this page: the credential that
 * authorises a revoke lives on the computer, and a second sign-in would mint an
 * offer against a session the computer has already consumed - so the page would
 * report a switch that never happened. Changing hands is done where the
 * credential is. */
function hostPairingCompletePage(email: string, device: string, pairedAt: number): string {
  const serverUrl = "https://qcrc.freevia.org/mcp";
  const paired = new Date(pairedAt).toLocaleString("en-US", { dateStyle: "medium", timeStyle: "short" });
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Device paired</title><style>${pairingStyles()}</style></head><body><main><h1>Device paired</h1><p>Your Quad Cortex now belongs to this Google account.</p><div class="account"><strong>${escapeHtml(email)}</strong><br>${escapeHtml(device)} · ${escapeHtml(paired)}</div><p class="help">Not the account you wanted? Open REMOTE in QC Remote, press Unpair, and scan the new code.</p><p>QC Remote reconnects while the app and Quad Cortex are available.</p><h2>Connect an AI app</h2>${providerSetupCards(serverUrl)}<small>Sign in to the connector with this same Google account. It grants the app access; it does not pair another device. Each provider controls its own connector, permissions, and availability.</small></main></body></html>`;
}

function mcpSetupPage(origin: string): string {
  const serverUrl = `${origin}/mcp`;
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Remote MCP Connector</title><style>${pairingStyles()}</style></head><body><main><h1>Connect an AI app</h1><p>This is the Remote MCP Connector for a Quad Cortex paired in QC Remote.</p><input class="server-address" value="${escapeHtml(serverUrl)}" readonly aria-label="Remote MCP Connector address" onfocus="this.select()"><p class="help">First open <b>REMOTE</b> in QC Remote and pair the Quad Cortex with your Google account. Then add this connector in one of the apps below and sign in with the same account.</p>${providerSetupCards(serverUrl)}<small>Each provider controls where custom connectors are available. If your app is not listed, use Other MCP App.</small></main></body></html>`;
}

/** Provider cards document the true third-party setup surface. */
function providerSetupCards(serverUrl: string): string {
  const address = escapeHtml(serverUrl);
  return `<div class="client-list">
    <section class="client supported"><div><div class="client-heading"><h2>Claude</h2><span>Mobile + desktop</span></div><p>Open Claude to manage your QC.</p></div><a class="provider-action" href="https://claude.ai/new">Open</a></section>
    <section class="client"><div><div class="client-heading"><h2>ChatGPT</h2><span>Web only</span></div><p>Open ChatGPT on the web to add QC Remote.</p></div><a class="provider-action" href="https://chatgpt.com/">Open</a></section>
    <section class="client"><div><div class="client-heading"><h2>Gemini Spark</h2><span>Eligible accounts</span></div><p>Add QC Remote in Gemini, then use it on mobile or web.</p></div><a class="provider-action" href="https://gemini.google.com/">Open</a></section>
    <section class="client"><div><div class="client-heading"><h2>Grok</h2><span>Web only</span></div><p>Open Grok on the web to add QC Remote.</p></div><a class="provider-action" href="https://grok.com/connectors">Open</a></section>
    <section class="client custom"><div><div class="client-heading"><h2>Other MCP App</h2><span>Extensible</span></div><p>Copy the address to add QC Remote to any OAuth-capable MCP app.</p></div><button class="provider-action endpoint" type="button" data-mcp-url="${address}" onclick="if(navigator.clipboard){navigator.clipboard.writeText(this.dataset.mcpUrl).then(()=&gt;this.textContent='Copied').catch(()=&gt;this.textContent='Copy failed')}">Use ${address}</button></section>
  </div>`;
}

function escapeHtml(value: string): string { return value.replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character] ?? character); }
