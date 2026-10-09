#!/usr/bin/env node

// Provider-neutral stdio-to-Streamable-HTTP bridge. It discovers the endpoint
// published by the running QC Remote app for the current Windows user. This
// process never loads device code, starts the broker, or opens USB.
const { createHmac, randomBytes, timingSafeEqual } = require("node:crypto");
const { readFile } = require("node:fs/promises");
const { mkdirSync, readFileSync, renameSync, writeFileSync } = require("node:fs");
const path = require("node:path");
const readline = require("node:readline");

const STATE_OVERRIDE = "QC_REMOTE_LOCAL_MCP_STATE";
const TIMEOUT_OVERRIDE = "QC_REMOTE_LOCAL_MCP_TIMEOUT_MS";
const APP_DIRECTORY = "org.freevia.qcremote";
const STATE_FILE = "local-mcp.json";
// QC Remote stamps this marker when installing the bridge so clients can
// report whether their local route matches the running Windows app.
const QC_REMOTE_APP_VERSION = "__QC_REMOTE_APP_VERSION__";
// QC Remote rewrites this slot with the tool contract digest it was built from
// when it generates a client package. A copy taken straight from the source
// tree keeps the placeholder, which means "unknown" rather than "mismatched".
const EXPECTED_TOOL_CONTRACT_SHA256 = "__QC_REMOTE_TOOL_CONTRACT_SHA256__";
// Matches the tool timeout the packaged client configurations advertise.
const DEFAULT_TIMEOUT_MS = 300000;
const HEALTH_TIMEOUT_MS = 5000;
// QC Remote is an ordinary desktop app. A client may well start before it, so
// the last session's surface is remembered next to the descriptor and replayed
// until the app publishes a live one.
const SESSION_CACHE_FILE = "client-session.json";
const APP_WATCH_INTERVAL_MS = 2000;
// QC Remote counts a client as live if it has announced itself inside a 12s
// window, so announce well inside it. The managed routes run QC Remote's own
// executable, which does this on /rpc; without it the LOCAL panel can never
// honestly show this client as connected.
const CLIENT_ID_OVERRIDE = "QC_REMOTE_LOCAL_MCP_CLIENT";
// The same argument the native bridge reads, for the same reason: a host can
// keep the configured env in its own MCP server state and never hand it to the
// child it spawns, and a bridge identified only that way announces itself
// anonymously however well its tool calls work.
const CLIENT_ID_ARGUMENT = "--local-mcp-client";
const ANNOUNCE_INTERVAL_MS = 5000;
// How long an announcement that keeps failing the same way waits before stderr
// hears about it again. Announcing every 5s is 720 attempts an hour, so a line
// for each buries whatever else the host captured - and writing none of them is
// the defect this closes: a bridge whose announcements were failing said nothing
// at all, so the LOCAL panel showed that host as disconnected with no record of
// why anywhere. One line when it breaks, one when the reason changes, one every
// ten minutes while it stays broken and one when it recovers is seven lines for
// that hour instead of 720.
const ANNOUNCE_FAILURE_REPEAT_AFTER_MS = 10 * 60 * 1000;
// A running app re-stamps publishedAtUnixMs on every descriptor copy every 30s.
// Five missed refreshes means the process is gone — and its ephemeral port may
// by now belong to something else, so an expired descriptor must not be dialled.
// Only descriptorRevision 4 and later refresh; earlier releases stamp once at
// startup, so they are exempt or every long-running old app would look dead.
const DESCRIPTOR_REFRESH_REVISION = 4;
const DESCRIPTOR_STALE_AFTER_MS = 150000;
const OFFLINE_ADVICE =
  "QC Remote is not running. Start QC Remote on this PC and connect the Quad Cortex over USB, then try again.";
// The identity check local_identity.rs defines, byte for byte: the path, the
// HMAC context, the nonce size and the client's patience all match the Rust
// clients, so either kind of client trusts exactly the same listeners.
const IDENTITY_PATH = "/identity";
const IDENTITY_CONTEXT = "qc-remote-local-identity-v1";
const IDENTITY_TIMEOUT_MS = 2000;
const IDENTITY_NONCE_BYTES = 32;

function statePaths() {
  if (process.env[STATE_OVERRIDE]) return [path.resolve(process.env[STATE_OVERRIDE])];
  // Claude Desktop launches extensions through a restricted host that can omit
  // LOCALAPPDATA even though it remains the current interactive Windows user.
  // USERPROFILE and Node's home directory are stable fallbacks for that case.
  const localAppData = process.env.LOCALAPPDATA
    || (process.env.USERPROFILE && path.join(process.env.USERPROFILE, "AppData", "Local"))
    || (() => {
      try {
        return path.join(require("node:os").homedir(), "AppData", "Local");
      } catch {
        return undefined;
      }
    })();
  const paths = [];
  if (localAppData) paths.push(path.join(localAppData, APP_DIRECTORY, STATE_FILE));
  const roamingAppData = process.env.APPDATA
    || (process.env.USERPROFILE && path.join(process.env.USERPROFILE, "AppData", "Roaming"));
  if (roamingAppData) paths.push(path.join(roamingAppData, "Claude", "qc-remote", STATE_FILE));
  if (!paths.length) {
    throw new Error("LOCALAPPDATA is unavailable; this bridge requires QC Remote on Windows");
  }
  return [...new Set(paths)];
}

// The app being closed is a normal state, not a fault: it is reported where the
// user asked for something the device has to answer, and never as a broken MCP
// server. Transport wording and descriptor paths stay on stderr.
class AppUnavailableError extends Error {
  constructor(detail) {
    super(OFFLINE_ADVICE);
    this.name = "AppUnavailableError";
    this.detail = detail;
  }
}

function validateDescriptor(value) {
  if (!value || value.schemaVersion !== 1 || value.transport !== "streamable-http") {
    throw new Error("QC Remote published an unsupported local MCP descriptor");
  }
  const endpoint = new URL(value.url);
  if (
    endpoint.protocol !== "http:" ||
    endpoint.hostname !== "127.0.0.1" ||
    endpoint.pathname !== "/mcp" ||
    !endpoint.port ||
    endpoint.username ||
    endpoint.password ||
    endpoint.search ||
    endpoint.hash
  ) {
    throw new Error("QC Remote published an invalid local MCP endpoint");
  }
  if (typeof value.bearerToken !== "string" || !/^[A-Za-z0-9_-]{43,}$/.test(value.bearerToken)) {
    throw new Error("QC Remote published an invalid local MCP credential");
  }
  const instanceId = typeof value.instanceId === "string" ? value.instanceId : "";
  // Absent (or null) only in a descriptor from a release that predates the
  // identity check, which is dialled as before. One that names a check is held
  // to it, as validate_descriptor does in local_stdio.rs: the check must sit
  // beside this endpoint and have an instance to prove.
  let identityUrl;
  if (value.identityUrl !== undefined && value.identityUrl !== null) {
    identityUrl = sameOriginPath(value.identityUrl, endpoint, IDENTITY_PATH);
    if (!identityUrl || !instanceId) {
      throw new Error("QC Remote published an invalid local identity address");
    }
  }
  return {
    url: endpoint.toString(),
    token: value.bearerToken,
    publishedAtUnixMs:
      Number.isSafeInteger(value.publishedAtUnixMs) && value.publishedAtUnixMs >= 0
        ? value.publishedAtUnixMs
        : 0,
    descriptorRevision: Number.isSafeInteger(value.descriptorRevision) ? value.descriptorRevision : 0,
    healthUrl: sameOriginHealthUrl(value.healthUrl, endpoint),
    rpcUrl: sameOriginPath(value.rpcUrl, endpoint, "/rpc"),
    toolContractSha256:
      typeof value.toolContractSha256 === "string" ? value.toolContractSha256 : undefined,
    identityUrl,
    instanceId,
  };
}

// Why a listener was not trusted with the token. An AppUnavailableError on
// purpose: the descriptor of a crashed app stays fresh for up to
// DESCRIPTOR_STALE_AFTER_MS while its port is free to bind, so whatever answers
// there without the proof is exactly "QC Remote is not running" - and the
// handle() path already turns that into an offline reply instead of a fault.
class IdentityProofError extends AppUnavailableError {
  constructor(reason) {
    super(reason);
    this.name = "IdentityProofError";
  }
}

function identityProof(token, instanceId, nonce) {
  // HMAC-SHA256 under the bearer token over context, instance and nonce joined
  // with NUL, base64url without padding: local_identity::prove.
  return createHmac("sha256", Buffer.from(token, "utf8"))
    .update(IDENTITY_CONTEXT)
    .update("\0")
    .update(instanceId)
    .update("\0")
    .update(nonce)
    .digest("base64url");
}

// Prove the listener on the published port holds the published token, before
// anything carries that token there: local_identity::verify. Only a fresh nonce
// is sent, unauthenticated. Every reason is one fixed string per cause, because
// the announcement's rate limiter keys on it.
//
// Deliberately not cached per descriptor. A crashed app's descriptor is
// indistinguishable from a live one - same token, same instance, same port -
// so a remembered success is precisely what would hand the token to whatever
// binds that port after the crash. The Rust clients prove before every send for
// the same reason; on loopback the check costs one small round trip.
async function proveIdentity(current) {
  if (!current.identityUrl) return;
  const nonce = randomBytes(IDENTITY_NONCE_BYTES).toString("base64url");
  let answer;
  try {
    const response = await fetch(current.identityUrl, {
      method: "POST",
      headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify({ nonce }),
      // The genuine listener answers in place; nothing legitimate redirects it.
      redirect: "error",
      signal: AbortSignal.timeout(IDENTITY_TIMEOUT_MS),
    });
    if (!response.ok) {
      await response.text().catch(() => undefined);
      throw new IdentityProofError(
        `the published endpoint refused the identity check (HTTP ${response.status})`,
      );
    }
    answer = JSON.parse(await response.text());
  } catch (error) {
    if (error instanceof IdentityProofError) throw error;
    const cause = error?.cause;
    const named = cause?.code ?? error?.name ?? "unknown";
    throw new IdentityProofError(`the published endpoint did not prove its identity (${named})`);
  }
  if (
    !answer || typeof answer !== "object"
    || typeof answer.instanceId !== "string" || typeof answer.proof !== "string"
  ) {
    throw new IdentityProofError("the published endpoint gave no identity proof");
  }
  const expected = Buffer.from(identityProof(current.token, current.instanceId, nonce), "utf8");
  const presented = Buffer.from(answer.proof, "utf8");
  const proven = presented.length === expected.length && timingSafeEqual(presented, expected);
  if (proven && answer.instanceId === current.instanceId) return;
  if (answer.instanceId === current.instanceId) {
    throw new IdentityProofError(
      "the listener on the published port could not prove it holds the published credential",
    );
  }
  throw new IdentityProofError("the process on the published port is not the QC Remote that published it");
}

// The descriptor is additive, so an absent or unexpected health URL degrades to
// "no readiness detail available" instead of rejecting an otherwise valid app.
function sameOriginHealthUrl(value, endpoint) {
  return sameOriginPath(value, endpoint, "/health");
}

function sameOriginPath(value, endpoint, pathname) {
  if (typeof value !== "string") return undefined;
  try {
    const candidate = new URL(value);
    if (candidate.origin !== endpoint.origin || candidate.pathname !== pathname) return undefined;
    if (candidate.username || candidate.password || candidate.search || candidate.hash) return undefined;
    return candidate.toString();
  } catch {
    return undefined;
  }
}

// `quiet` belongs to the announcement loop, which reads this every five seconds
// and decides for itself what is worth writing. The stale-descriptor line below
// carries an age that changes on every tick, so from there it is 720 lines an
// hour, no two alike; worse, folding that age into the announcement's failure
// reason would make every attempt look like new information to the rate limiter.
// The request path still writes it, where the user has asked for something.
async function descriptor({ quiet = false } = {}) {
  const attempts = [];
  const valid = [];
  // Captured once, because the "looked in" diagnostic below has to name every
  // path considered rather than only the ones whose read threw. A descriptor
  // that is present and valid but expired leaves `attempts` empty, and that
  // made the one message whose whole job is to say where we looked name
  // nowhere at all - which is exactly the state an abnormally exited app leaves
  // behind, and the state the watch loop sits in.
  const considered = statePaths();
  for (const stateFile of considered) {
    try {
      valid.push(validateDescriptor(JSON.parse(await readFile(stateFile, "utf8"))));
    } catch (error) {
      attempts.push({ stateFile, error, missing: error?.code === "ENOENT" });
    }
  }
  const expired = valid.filter(
    (candidate) =>
      candidate.descriptorRevision >= DESCRIPTOR_REFRESH_REVISION &&
      Date.now() - candidate.publishedAtUnixMs > DESCRIPTOR_STALE_AFTER_MS,
  );
  let ignoredStale = false;
  for (const candidate of expired) {
    valid.splice(valid.indexOf(candidate), 1);
    ignoredStale = true;
    if (quiet) continue;
    process.stderr.write(
      `[qc-remote-mcp] ignoring a descriptor QC Remote stopped refreshing ${Math.round((Date.now() - candidate.publishedAtUnixMs) / 1000)}s ago
`,
    );
  }
  if (valid.length) {
    // Either mirrored copy can survive an abnormal app exit. The endpoint's
    // ephemeral port makes a valid old descriptor unusable, so path order is
    // not authoritative; the app's publication timestamp is.
    return valid.reduce((newest, candidate) =>
      candidate.publishedAtUnixMs > newest.publishedAtUnixMs ? candidate : newest
    );
  }
  // A descriptor that exists but does not validate is the actual fault. Report
  // it ahead of any later candidate that was simply not present. Either way the
  // user-facing outcome is the same — the app is not reachable — so the
  // diagnostic goes to stderr and the message stays actionable.

  // The announcement loop is still told a stale descriptor was dropped, which is
  // often the whole explanation, but without the age - so the reason it keys on
  // stays one string for as long as the same descriptor is being ignored. A live
  // descriptor was chosen above, so this note never reaches a working bridge.
  const alsoStale = quiet && ignoredStale
    ? "; a descriptor QC Remote stopped refreshing was ignored"
    : "";
  const blamed = attempts.find((attempt) => !attempt.missing);
  if (blamed) {
    const others = attempts.filter((attempt) => attempt !== blamed).map((attempt) => attempt.stateFile);
    const also = others.length ? ` (not present: ${others.join(", ")})` : "";
    const detail = blamed.error instanceof Error ? blamed.error.message : String(blamed.error);
    throw new AppUnavailableError(
      `QC Remote published an unusable local MCP descriptor at ${blamed.stateFile}: ${detail}${also}${alsoStale}`,
    );
  }
  const looked = considered.join(", ");
  throw new AppUnavailableError(`no local MCP descriptor. Looked in: ${looked}${alsoStale}`);
}

function sessionCacheFile() {
  try {
    return path.join(path.dirname(statePaths()[0]), SESSION_CACHE_FILE);
  } catch {
    return undefined;
  }
}

let sessionId;
let protocolVersion;
let establishedToken;
let handshakeRequest;
let initializedNotification;
let handshakeInFlight;
let warnedToolContract = false;

function requestTimeoutMs() {
  const configured = Number(process.env[TIMEOUT_OVERRIDE]);
  return Number.isFinite(configured) && configured > 0 ? configured : DEFAULT_TIMEOUT_MS;
}

function emitJson(value) {
  // One write per message keeps stdout lines whole while requests overlap.
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

async function consumeEventStream(response, observe) {
  if (!response.body) throw new Error("QC Remote returned an empty MCP event stream");
  const decoder = new TextDecoder();
  let buffer = "";
  let data = [];
  const flush = () => {
    if (data.length) observe(JSON.parse(data.join("\n")));
    data = [];
  };
  for await (const chunk of response.body) {
    buffer += decoder.decode(chunk, { stream: true });
    const lines = buffer.split(/\r?\n/);
    buffer = lines.pop() ?? "";
    for (const line of lines) {
      if (!line) flush();
      else if (line.startsWith("data:")) {
        const value = line.slice(5).trimStart();
        if (value) data.push(value);
      }
    }
  }
  buffer += decoder.decode();
  if (buffer.startsWith("data:")) data.push(buffer.slice(5).trimStart());
  flush();
}

function warnOnToolContractSkew(current) {
  if (warnedToolContract) return;
  if (!/^[0-9a-f]{64}$/.test(EXPECTED_TOOL_CONTRACT_SHA256)) return;
  const published = current.toolContractSha256;
  if (typeof published !== "string" || published === EXPECTED_TOOL_CONTRACT_SHA256) return;
  warnedToolContract = true;
  // Non-fatal on purpose: a contract revision usually adds tools rather than
  // breaking the ones this client already knows, and refusing every request
  // would strand a working device behind a version mismatch.
  process.stderr.write(
    `[qc-remote-mcp] tool contract mismatch: this client was built against ${EXPECTED_TOOL_CONTRACT_SHA256} but QC Remote publishes ${published}. Reinstall the client package from the running QC Remote if a tool behaves unexpectedly.\n`,
  );
}

// Streamable HTTP reports an unknown session id as 404. The spec answer is a
// fresh InitializeRequest, so this is recoverable rather than a plain failure.
// `sent` is the id the rejected request carried, which is the only one the 404
// is about: requests overlap, so by the time the reply arrives the shared id may
// already have been cleared for a re-handshake or replaced by its result.
class SessionExpiredError extends Error {
  constructor(sent) {
    super("QC Remote reset the local MCP session. Please retry in a moment.");
    this.name = "SessionExpiredError";
    this.sent = sent;
  }
}

async function typedFailure(response) {
  try {
    const body = await response.text();
    if (!body.trim()) return undefined;
    const parsed = JSON.parse(body);
    if (!parsed || typeof parsed !== "object") return undefined;
    const code = typeof parsed.code === "string" ? parsed.code : undefined;
    const reason = typeof parsed.reason === "string" ? parsed.reason : undefined;
    return code || reason ? { code, reason } : undefined;
  } catch {
    return undefined;
  }
}

async function readiness(current) {
  if (!current.healthUrl) return undefined;
  try {
    await proveIdentity(current);
    const response = await fetch(current.healthUrl, {
      method: "GET",
      headers: { accept: "application/json", authorization: `Bearer ${current.token}` },
      signal: AbortSignal.timeout(HEALTH_TIMEOUT_MS),
    });
    return await typedFailure(response);
  } catch {
    return undefined;
  }
}

// Status codes and machine reasons are for stderr. What reaches the user is the
// action they can take about it.
function adviceForReason(reason) {
  switch (reason) {
    case "device_not_ready":
      return "The Quad Cortex is not connected. Connect it over USB and make sure QC Remote shows it as live, then try again.";
    case "unauthorized":
      return "QC Remote did not accept this client's credential. Re-pair this app from QC Remote's LOCAL panel.";
    case "forbidden":
    case "browser_origin_forbidden":
    case "invalid_host":
      return "QC Remote refused this request. Check the control access level in QC Remote and try again.";
    case "device_unavailable":
      return "QC Remote cannot reach the Quad Cortex. Check that it is connected over USB and shown as live in QC Remote, then try again.";
    case "device_error":
      return "The Quad Cortex refused that request. Check the device, then try again.";
    case "unknown_action":
      return "QC Remote does not recognise that action. This client is probably out of date — re-pair it from QC Remote's LOCAL panel.";
    default:
      return undefined;
  }
}

function adviceForStatus(status) {
  if (status === 401) {
    return "QC Remote did not accept this client's credential. Re-pair this app from QC Remote's LOCAL panel.";
  }
  if (status === 403) {
    return "QC Remote refused this request. Check the control access level in QC Remote and try again.";
  }
  if (status === 413) return "That request was too large for QC Remote to accept.";
  if (status >= 500) return "QC Remote could not complete that request. Check the app and try again.";
  return `QC Remote rejected that request (HTTP ${status}).`;
}

function diagnose(typed, status) {
  const label = [typed.code, typed.reason ? `(${typed.reason})` : undefined]
    .filter(Boolean)
    .join(" ");
  process.stderr.write(`[qc-remote-mcp] HTTP ${status} ${label}
`);
  return adviceForReason(typed.reason) ?? adviceForReason(typed.code) ?? adviceForStatus(status);
}

async function failureFor(response, current) {
  const typed = await typedFailure(response);
  if (typed) return new Error(diagnose(typed, response.status));
  // The status is the diagnosis. Readiness is consulted only for a server-side
  // fault, where the device's state may actually explain it — asking after a
  // 401 would turn a rotated credential into "check your USB cable".
  if (response.status < 500) {
    process.stderr.write(`[qc-remote-mcp] HTTP ${response.status} with no typed reason
`);
    return new Error(adviceForStatus(response.status));
  }
  const health = await readiness(current);
  const context = health ? adviceForReason(health.reason) : undefined;
  process.stderr.write(
    `[qc-remote-mcp] HTTP ${response.status} untyped; /health reports ${health?.code ?? "nothing"} ${health?.reason ?? ""}
`,
  );
  return new Error(
    context
      ? `${adviceForStatus(response.status)} QC Remote also reports: ${context}`
      : adviceForStatus(response.status),
  );
}

async function post(current, message, options = {}) {
  // Before the session id is captured below, so the request still carries the
  // freshest one; a failed proof throws IdentityProofError and nothing is sent.
  await proveIdentity(current);
  const headers = {
    accept: "application/json, text/event-stream",
    authorization: `Bearer ${current.token}`,
    "content-type": "application/json",
  };
  // Captured once and used for both the header and the reply's classification.
  // Reading the shared id again at reply time misfiled a sibling's 404: a
  // parallel request that hit the expiry first had already cleared it to
  // re-handshake, so the sibling's "unknown session" read as a bare HTTP 404.
  const sent = sessionId;
  if (sent) headers["mcp-session-id"] = sent;
  if (protocolVersion) headers["mcp-protocol-version"] = protocolVersion;
  const timeoutMs = requestTimeoutMs();
  let response;
  try {
    response = await fetch(current.url, {
      method: "POST",
      headers,
      body: JSON.stringify(message),
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (error) {
    if (error?.name === "TimeoutError" || error?.name === "AbortError") {
      throw new Error(`QC Remote timed out after ${timeoutMs} ms without answering. The app may be busy or not responding.`);
    }
    // No HTTP status came back at all, so nothing is listening on the port the
    // descriptor advertises: the app has closed since it published, or is still
    // starting. Either way this is the app's absence, not a transport fault the
    // user can act on.
    throw new AppUnavailableError(
      `could not reach ${current.url}: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  const nextSession = response.headers.get("mcp-session-id");
  if (nextSession) sessionId = nextSession;
  if (response.status === 202 || response.status === 204) return;
  // A request sent with no session id is answered 422 by the app, so a 404 is
  // only ever "unknown session" - but only when an id was actually sent.
  if (response.status === 404 && sent) {
    await response.text().catch(() => undefined);
    throw new SessionExpiredError(sent);
  }
  if (!response.ok) throw await failureFor(response, current);
  const observe = (value) => {
    // Per the MCP spec the server may answer with a version it supports rather
    // than the one that was proposed; later requests must carry that one.
    if (options.negotiate && typeof value?.result?.protocolVersion === "string") {
      protocolVersion = value.result.protocolVersion;
    }
    if (options.tap) options.tap(value);
    if (!options.silent) emitJson(value);
  };
  const contentType = response.headers.get("content-type") ?? "";
  if (contentType.includes("text/event-stream")) return consumeEventStream(response, observe);
  const body = await response.text();
  if (body.trim()) observe(JSON.parse(body));
}

// A QC Remote restart publishes a new credential against a server that has no
// session for this process, so the recorded handshake has to be replayed before
// the request that discovered the rotation.
async function reestablish(current) {
  sessionId = undefined;
  protocolVersion = undefined;
  await post(current, handshakeRequest, { silent: true, negotiate: true });
  if (initializedNotification) await post(current, initializedNotification, { silent: true });
  establishedToken = current.token;
}

// One handshake at a time, whether a rotated credential or a reset session
// prompted it, so concurrent requests share a single re-initialization.
async function withHandshake(current) {
  if (handshakeInFlight) {
    // Observe the outcome rather than swallowing it. A follower that returns
    // from a *failed* handshake posts with no session, and the server's
    // "expect initialize request" rejection then gets blamed on whatever
    // /health happens to report — a handshake bug read as a USB fault.
    // Defensive: no interleaving reachable from the stdin loop enters this
    // branch today, because ensureSession already absorbs concurrent callers
    // and re-checks the credential afterwards.
    await handshakeInFlight;
    return;
  }
  handshakeInFlight = reestablish(current);
  // A handshake with no follower must not take the process down with it.
  handshakeInFlight.catch(() => {});
  try {
    await handshakeInFlight;
  } finally {
    handshakeInFlight = undefined;
  }
}

// Recovery for a request whose session was reported unknown. Parallel requests
// sent on the same expired id all get that 404, but only the first to see it
// may replace the session: one arriving while that handshake runs waits for it,
// and one arriving after it finished finds the shared id already moved past the
// id it sent and simply retries on the fresh session. Re-handshaking there would
// replace the session the leader just established, and every request still on
// it would earn a 404 of its own. An id cleared by a handshake that failed has
// not moved on to anything, so that case still re-handshakes.
async function recoverSession(current, sent) {
  if (handshakeInFlight) {
    await handshakeInFlight;
    return;
  }
  if (sessionId && sessionId !== sent) return;
  await withHandshake(current);
}

async function ensureSession(current) {
  if (handshakeInFlight) await handshakeInFlight.catch(() => {});
  if (establishedToken === current.token) return;
  if (!handshakeRequest) {
    // Nothing to replay, but the recorded session belonged to the previous app
    // instance; carrying its id into a new one only earns a 404.
    sessionId = undefined;
    protocolVersion = undefined;
    establishedToken = current.token;
    return;
  }
  await withHandshake(current);
}

function rememberSession(value) {
  const file = sessionCacheFile();
  if (!file) return;
  try {
    mkdirSync(path.dirname(file), { recursive: true });
    // Every client on this machine shares one cache file, so two bridges can
    // write it at once. Stage and rename, as the app does for the descriptor,
    // or a concurrent reader can catch a half-written file and fall back to an
    // empty tool list for the rest of its session.
    const staged = `${file}.${process.pid}.tmp`;
    writeFileSync(staged, JSON.stringify(value));
    renameSync(staged, file);
  } catch (error) {
    process.stderr.write(
      `[qc-remote-mcp] could not remember the local MCP session: ${error instanceof Error ? error.message : String(error)}\n`,
    );
  }
}

function rememberedSession() {
  const file = sessionCacheFile();
  if (!file) return undefined;
  try {
    const value = JSON.parse(readFileSync(file, "utf8"));
    return value && typeof value === "object" ? value : undefined;
  } catch {
    return undefined;
  }
}

async function forward(message) {
  // Record the handshake before the app is consulted. A client that starts
  // while QC Remote is closed still gets a session, and this is what gets
  // replayed against the app once it appears.
  if (message.method === "initialize") {
    handshakeRequest = message;
    initializedNotification = undefined;
    establishedToken = undefined;
    sessionId = undefined;
    protocolVersion = undefined;
    // The host has opened a session, which is what "connected" means here — not
    // that it went on to send notifications/initialized, which some do not.
    announceWhileConnected();
  }
  if (message.method === "notifications/initialized") initializedNotification = message;
  // Re-read on every request so a QC Remote restart rotates credentials without
  // requiring the provider process to restart.
  const current = await descriptor();
  warnOnToolContractSkew(current);
  if (message.method === "initialize") {
    handshakeInFlight = post(current, message, { negotiate: true, tap: rememberHandshake });
    try {
      await handshakeInFlight;
    } finally {
      handshakeInFlight = undefined;
    }
    establishedToken = current.token;
    return;
  }
  await ensureSession(current);
  try {
    await post(current, message, message.method === "tools/list" ? { tap: rememberTools } : {});
  } catch (error) {
    if (!(error instanceof SessionExpiredError) || !handshakeRequest) throw error;
    // Exactly one recovery attempt: a server that rejects the fresh session too
    // is reporting a real fault, not a stale identifier.
    await recoverSession(current, error.sent);
    await post(current, message);
  }
}

function unavailable(message, detail) {
  return {
    jsonrpc: "2.0",
    id: Object.hasOwn(message, "id") ? message.id : null,
    error: {
      code: -32000,
      message: `QC Remote local MCP is unavailable: ${detail || OFFLINE_ADVICE}`,
    },
  };
}

function rememberHandshake(value) {
  const result = value?.result;
  if (!result || typeof result.protocolVersion !== "string") return;
  rememberSession({ ...(rememberedSession() ?? {}), handshake: result });
}

function rememberTools(value) {
  const tools = value?.result?.tools;
  if (!Array.isArray(tools)) return;
  rememberSession({ ...(rememberedSession() ?? {}), tools });
}

// What to answer while QC Remote is closed. The handshake and the tool list are
// replayed from the last session with the app, so the assistant still sees the
// QC tools and can say why it cannot run them; anything that needs the device
// reports the app's absence where the user actually asked for it.
function offlineHandshake(message) {
  const remembered = rememberedSession()?.handshake;
  const requested = message?.params?.protocolVersion;
  const version =
    typeof remembered?.protocolVersion === "string"
      ? remembered.protocolVersion
      : typeof requested === "string"
        ? requested
        : "2025-06-18";
  return {
    protocolVersion: version,
    capabilities: { tools: { listChanged: true }, resources: { listChanged: true } },
    serverInfo: remembered?.serverInfo ?? {
      name: "QC Remote",
      // A copy taken straight from the source tree keeps the placeholder; the
      // client is told "unknown" rather than shown the slot name.
      version: /^\d/.test(QC_REMOTE_APP_VERSION) ? QC_REMOTE_APP_VERSION : "0.0.0",
    },
    instructions: [remembered?.instructions, OFFLINE_ADVICE].filter(Boolean).join("\n\n"),
  };
}

function offlineReply(message) {
  const id = Object.hasOwn(message, "id") ? message.id : null;
  const answer = (result) => ({ jsonrpc: "2.0", id, result });
  switch (message.method) {
    case "initialize":
      return answer(offlineHandshake(message));
    case "ping":
      return answer({});
    case "tools/list":
      return answer({ tools: rememberedSession()?.tools ?? [] });
    case "resources/list":
      return answer({ resources: [] });
    case "resources/templates/list":
      return answer({ resourceTemplates: [] });
    case "prompts/list":
      return answer({ prompts: [] });
    case "tools/call":
      // A tool-level failure, not a protocol error: the assistant can read this
      // and relay it, where a JSON-RPC error reads to the host as a dead server.
      return answer({ content: [{ type: "text", text: OFFLINE_ADVICE }], isError: true });
    default:
      return unavailable(message, OFFLINE_ADVICE);
  }
}

// Once the app appears, tell the client to re-read the surface it was served
// from memory. Stat-only polling: no request is made until the client sends one.
let appWatch;
function watchForApp() {
  if (appWatch) return;
  appWatch = setInterval(async () => {
    try {
      // Quiet for the announcement's reason, only more so. An app that exited
      // abnormally leaves an expired descriptor on disk permanently, and this
      // runs every two seconds for as long as it is gone, so the loud path
      // writes the stale-descriptor line 1800 times an hour, each carrying a
      // different age, into whatever the host captured. The request path still
      // names that copy, age and all, wherever the user asked for something;
      // a presence poll has nothing to tell them.
      // A listener that fails the proof is not the app appearing: telling the
      // client to re-read its tools then would only earn another offline
      // answer and re-arm this loop, every two seconds, for as long as the
      // squatter holds the port.
      await proveIdentity(await descriptor({ quiet: true }));
    } catch {
      // Not the announcement's bare catch. That one was the only witness to its
      // own fault, so silence there left no record anywhere. This loop is armed
      // only from the AppUnavailableError branch below, which has already
      // written the whole reason - descriptor paths, parse failure and all -
      // and writes it again on every later request. What this swallows is
      // "still closed", the state the loop exists to wait out, on a clock the
      // user never set.
      return;
    }
    clearInterval(appWatch);
    appWatch = undefined;
    emitJson({ jsonrpc: "2.0", method: "notifications/tools/list_changed" });
    emitJson({ jsonrpc: "2.0", method: "notifications/resources/list_changed" });
  }, APP_WATCH_INTERVAL_MS);
  appWatch.unref();
}

// Valid JSON is not necessarily a JSON-RPC message. Anything that cannot carry
// a reply gets none, and losing one reply must never cost the client its session.
function canReply(message) {
  return Boolean(message) && typeof message === "object" && Object.hasOwn(message, "id");
}

async function handle(message) {
  try {
    await forward(message);
  } catch (error) {
    if (error instanceof AppUnavailableError) {
      // Expected whenever QC Remote is simply not open. Diagnostics — descriptor
      // paths, transport wording — stay on stderr; the client gets a usable
      // session and one actionable sentence.
      process.stderr.write(
        `[qc-remote-mcp] QC Remote is not reachable: ${error.detail ?? error.message}\n`,
      );
      if (canReply(message)) emitJson(offlineReply(message));
      if (handshakeRequest) watchForApp();
      return;
    }
    const detail = error instanceof Error ? error.message : String(error);
    process.stderr.write(`[qc-remote-mcp] ${detail}\n`);
    if (canReply(message)) emitJson(unavailable(message, detail));
  }
}

// Streamable HTTP keeps a session until the client deletes it or it idles out.
// The host closing stdin is the client going away, so say so rather than
// leaving a worker on the app to expire on its own.
async function releaseSession() {
  if (!sessionId) return;
  const id = sessionId;
  sessionId = undefined;
  let current;
  try {
    current = await descriptor();
    await proveIdentity(current);
  } catch {
    // No descriptor, or a listener that is not the app: either way there is
    // no session of ours there to release, and the token stays here.
    return;
  }
  try {
    await fetch(current.url, {
      method: "DELETE",
      headers: {
        authorization: `Bearer ${current.token}`,
        "mcp-session-id": id,
        ...(protocolVersion ? { "mcp-protocol-version": protocolVersion } : {}),
      },
      signal: AbortSignal.timeout(HEALTH_TIMEOUT_MS),
    });
  } catch {
    // The app may already be gone; there is nothing left to release.
  }
}

// Announce this client to the running app so the LOCAL panel can show it as
// connected. Starts only once the host has completed a handshake — a process
// sitting on stdin that was never initialized is not a connected client — and
// writes to stderr when it cannot, rather than failing in silence.
let announcing;
let warnedAnonymous = false;
function announceWhileConnected() {
  if (announcing) return;
  const named = process.argv.indexOf(CLIENT_ID_ARGUMENT);
  const client = ((named >= 0 ? process.argv[named + 1] : undefined)
    ?? process.env[CLIENT_ID_OVERRIDE]
    ?? "").trim();
  if (!client || client.length > 64) {
    // Not an announcement failure - the app would answer one - but it is the
    // reason a working host reads as never-attached, and returning in silence
    // here left nothing to find. Worth exactly one line: a host can open a
    // second session on this same process, and this is not per-interval news.
    if (warnedAnonymous) return;
    warnedAnonymous = true;
    process.stderr.write(`[qc-remote-mcp] this bridge was started without ${CLIENT_ID_ARGUMENT}, so QC Remote cannot credit its heartbeat to a host and the LOCAL panel cannot show that host as connected.
`);
    return;
  }
  const report = announcementReport();
  const startedAt = performance.now();
  const announce = async () => {
    try {
      const line = report.observe(client, await attemptAnnouncement(client), performance.now() - startedAt);
      if (line) {
        process.stderr.write(`${line}
`);
      }
    } catch {
      // The announcement is telemetry, never a dependency: a fault in the
      // reporting must not take the MCP session with it, and the next tick
      // reports the same thing again anyway.
    }
  };
  void announce();
  announcing = setInterval(announce, ANNOUNCE_INTERVAL_MS);
  announcing.unref();
}

// What stderr is told about an announcement attempt, and whether it is worth
// saying at all. Both of the failure paths attemptAnnouncement now names used
// to be a bare `catch {}` - one of them commented that nothing about it was
// worth telling the user - so a bridge that could not announce itself was
// invisible: the LOCAL panel showed the host as disconnected and no record of
// why existed anywhere.
function announcementReport() {
  // The reason the last written line gave, while the failure lasts.
  let reported;
  // When that line was written, in milliseconds since the loop started.
  let reportedAtMs = 0;
  // Attempts that failed since then and said nothing new.
  let suppressed = 0;
  return {
    // Folds one attempt into the report and returns the line to write, or
    // nothing. An undefined reason means the attempt succeeded. elapsedMs is a
    // parameter rather than a clock read in here so the rate limit can be
    // asserted without spending ten real minutes to reach it.
    observe(client, reason, elapsedMs) {
      if (reason === undefined) {
        // Nothing was ever reported, so there is no recovery to announce and a
        // bridge that never failed stays silent for its whole life.
        if (reported === undefined) return undefined;
        const failures = suppressed + 1;
        const last = reported;
        reported = undefined;
        reportedAtMs = elapsedMs;
        suppressed = 0;
        return `[qc-remote-mcp] ${client} announced itself again after ${failures} failed attempts; the last reason was: ${last}`;
      }
      const repeating = reported === reason;
      const due = elapsedMs - reportedAtMs >= ANNOUNCE_FAILURE_REPEAT_AFTER_MS;
      if (repeating && !due) {
        suppressed += 1;
        return undefined;
      }
      const stoodInFor = suppressed;
      reported = reason;
      reportedAtMs = elapsedMs;
      suppressed = 0;
      return repeating
        ? `[qc-remote-mcp] ${client} still cannot announce itself: ${reason} (${stoodInFor} further attempts failed for the same reason since the last line)`
        : `[qc-remote-mcp] ${client} could not announce itself: ${reason}`;
    },
  };
}

// One attempt, returning why it failed or undefined once the app has recorded
// it. Every branch names one distinguishable cause, because these strings are
// the only record of why a working host reads as disconnected - and they are
// the rate limiter's key, so each has to stay the same string for as long as
// the same fault lasts.
async function attemptAnnouncement(client) {
  let current;
  try {
    current = await descriptor({ quiet: true });
  } catch (error) {
    if (error?.detail) return error.detail;
    return error instanceof Error ? error.message : String(error);
  }
  if (!current.rpcUrl) return "the published descriptor names no /rpc endpoint to announce to";
  try {
    await proveIdentity(current);
  } catch (error) {
    return error?.detail ?? (error instanceof Error ? error.message : String(error));
  }
  let response;
  try {
    response = await fetch(current.rpcUrl, {
      method: "POST",
      headers: {
        authorization: `Bearer ${current.token}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ rpc: "local.bridgeHeartbeat", params: { client } }),
      signal: AbortSignal.timeout(HEALTH_TIMEOUT_MS),
    });
  } catch (error) {
    // Node words every transport fault as "fetch failed" and puts the part that
    // identifies it on the cause: ECONNREFUSED while the app is closed, a
    // TimeoutError when it is wedged. A reason of "fetch failed" would be no
    // reason at all, and both of these stay the same string while the fault
    // lasts, which is what the rate limiter's key needs.
    const cause = error?.cause;
    const named = cause?.code ?? cause?.message
      ?? (error instanceof Error ? error.message : String(error));
    return `the loopback endpoint did not answer (${named})`;
  }
  // A non-2xx means the app did not record this host, which is the only reason
  // the call exists; the previous version discarded the response unread.
  if (!response.ok) return `QC Remote refused the announcement (${response.status})`;
  return undefined;
}

async function main() {
  const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  const inFlight = new Set();
  for await (const line of lines) {
    if (!line.trim()) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      process.stderr.write(`[qc-remote-mcp] ${error instanceof Error ? error.message : String(error)}\n`);
      continue;
    }
    // A pre-2025-06-18 client may send a batch. The app's MCP server has no
    // batch variant, so forwarding the array whole reached nothing and, having
    // no `id` of its own, produced no reply at all — the requests inside simply
    // hung. Each element is dispatched on its own and answered on its own.
    const batch = Array.isArray(message) ? message : [message];
    for (const single of batch) {
      // Dispatch without awaiting so stdin keeps draining: a cancellation or a
      // ping must not queue behind a slow device call.
      const pending = handle(single);
      inFlight.add(pending);
      void pending.catch(() => {}).finally(() => inFlight.delete(pending));
    }
  }
  await Promise.allSettled([...inFlight]);
  await releaseSession();
}

main().catch((error) => {
  process.stderr.write(`[qc-remote-mcp] ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
