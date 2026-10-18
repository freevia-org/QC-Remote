#!/usr/bin/env node

// Provider-neutral stdio-to-Streamable-HTTP bridge. It discovers the endpoint
// published by the running QC Remote app for the current Windows user. This
// process never loads device code, starts the broker, or opens USB.
const { createHmac, randomBytes, randomUUID, timingSafeEqual } = require("node:crypto");
const { open } = require("node:fs/promises");
const { closeSync, fsyncSync, mkdirSync, openSync, readSync, renameSync, unlinkSync, writeFileSync } = require("node:fs");
const path = require("node:path");
const { PassThrough } = require("node:stream");

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

// Best-effort diagnostic output has an independent, small software envelope.
// A stalled stderr never blocks RPC/control input or becomes an unbounded log.
function createBridgeDiagnostics(output, options = {}) {
  const lineBytes = options.lineBytes ?? 4096;
  const queueBytes = options.queueBytes ?? 64 * 1024;
  const queueMessages = options.queueMessages ?? 16;
  const records = new Set();
  let bytes = 0;
  let dropped = 0;
  let attached = false;
  let stopped = false;
  let closed = false;
  let closeObserved = false;
  let stopPromise;
  let notifyStop;
  const suppress = () => { dropped = Math.min(Number.MAX_SAFE_INTEGER, dropped + 1); return false; };
  const detach = () => {
    if (!attached) return;
    attached = false;
    output.removeListener("error", onError);
    output.removeListener("close", onClose);
  };
  const complete = (record) => {
    if (!records.delete(record)) return;
    bytes -= record.bytes.length;
    record.bytes = undefined;
    if (stopped && !records.size && closeObserved) { detach(); notifyStop?.(true); }
    if (!stopped && !closed && dropped) {
      const count = dropped;
      dropped = 0;
      write(`[qc-remote-mcp] ${count} diagnostic messages were suppressed while stderr was congested.\n`);
    }
  };
  const onError = () => {
    closed = true;
    // Error is not a write-callback/closed-handle acknowledgement. Keep every
    // exact queued buffer charged until its callback or the stream closes.
    try { output.destroy(); } catch {}
  };
  const onClose = () => {
    closed = true;
    closeObserved = true;
    for (const record of [...records]) complete(record);
    if (stopped) { detach(); notifyStop?.(true); }
  };
  function write(value) {
    if (stopped || closed) return suppress();
    // Slice UTF16 first so even a large parser diagnostic is bounded before
    // UTF8 encoding/queue retention. Iterate code points to avoid cutting UTF8.
    const original = String(value);
    const sample = original.slice(0, lineBytes);
    let text = "";
    let length = 0;
    for (const character of sample) {
      const count = Buffer.byteLength(character);
      if (count > lineBytes - 1 - length) break;
      text += character;
      length += count;
    }
    if (!text.endsWith("\n")) text += "\n";
    const required = Buffer.byteLength(text);
    if (records.size >= queueMessages || required > queueBytes - bytes) return suppress();
    const record = { bytes: Buffer.from(text) };
    records.add(record);
    bytes += record.bytes.length;
    if (!attached) {
      attached = true;
      output.on("error", onError);
      output.on("close", onClose);
    }
    try { output.write(record.bytes, (error) => { if (error) closed = true; complete(record); }); }
    catch { closed = true; try { output.destroy(); } catch {} }
    return true;
  }
  function stop() {
    if (stopPromise) return stopPromise;
    stopped = true;
    if (!records.size) { if (!output.destroyed || closeObserved) detach(); return Promise.resolve(true); }
    stopPromise = new Promise((resolve) => {
      let settled = false;
      const timer = setTimeout(() => finish(false), options.stopWithinMs ?? 500);
      const finish = (acknowledged) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(acknowledged);
      };
      notifyStop = finish;
      // Do not report delivery: destruction cancels this process's own stderr
      // stream. Only callback/close releases queued originals and their bytes.
      try { output.destroy(); } catch { finish(false); }
    });
    return stopPromise;
  }
  return { write, stop, snapshot: () => ({ bytes, messages: records.size, dropped, stopped, closed }) };
}


// These are software envelopes, not Quad Cortex firmware limits.
function createBridgeEnvelope(options = {}) {
  const backendBytes = options.backendBytes ?? 16 * 1024 * 1024;
  const requestBytes = options.requestBytes ?? 4 * 1024 * 1024;
  const batchBytes = options.batchBytes ?? 4 * requestBytes;
  const controlBytes = options.controlBytes ?? 64 * 1024;
  const controlCount = 4;
  const ordinaryCount = 8;
  const overhead = options.overhead ?? 4096;
  // rmcp structured(value): escaped text JSON + structuredContent + optional
  // base64 image. The request id is bounded by the individual request envelope.
  const replyBytes = 4 * backendBytes + overhead + requestBytes;
  const controlReplyBytes = 2 * controlBytes + overhead;
  // Account bytes/strings/parsed-JSON allowance/output before retaining them.
  // Shared charging, not eight independent full-size reservations, bounds the
  // simultaneous retained responses/output. Saturation is a truthful failure.
  // This is an accounted retention budget, not a promise about JS-engine heap
  // implementation overhead, GC timing, kernel buffers or OS hard deadlines.
  const retainedBytes = options.retainedBytes
    ?? 6 * replyBytes + batchBytes + 4 * controlReplyBytes + 16 * 1024 * 1024;
  for (const value of [backendBytes, requestBytes, batchBytes, controlBytes, replyBytes, retainedBytes]) {
    if (!Number.isSafeInteger(value) || value <= 0) throw new Error("Invalid bridge software envelope");
  }
  const input = options.input ?? process.stdin;
  const output = options.output ?? process.stdout;
  const abort = new AbortController();
  let fatal;
  let used = 0;
  let ordinary = 0;
  let control = 0;
  const outputs = new Set();
  const writes = new Set();
  let outputAttached = false;
  let outputClosed = false;
  const diagnostics = createBridgeDiagnostics(options.diagnosticOutput ?? process.stderr, options.diagnostics);
  const fail = (error) => {
    if (fatal) return fatal;
    fatal = error instanceof Error ? error : new Error(String(error));
    abort.abort(fatal);
    input.destroy();
    // Output cannot acknowledge more bytes. Close honestly, not a synthetic
    // successful delivery or a replay of an already-dispatched tool mutation.
    output.destroy();
    return fatal;
  };
  const charge = (bytes) => {
    if (fatal) throw fatal;
    if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > retainedBytes - used) {
      throw new Error("Bridge aggregate retained-byte budget exhausted");
    }
    used += bytes;
    let held = true;
    return () => { if (held) { held = false; used -= bytes; } };
  };
  const retireWrite = (record) => {
    if (!writes.delete(record)) return;
    record.bytesLease(); record.textLease();
  };
  const onOutputError = (error) => {
    fail(error);
    for (const record of writes) record.finish(error);
  };
  const onOutputClose = () => {
    outputClosed = true;
    fail(new Error("Bridge stdout closed"));
    for (const record of [...writes]) {
      retireWrite(record);
      record.finish(new Error("Bridge stdout closed before its queued answer was acknowledged"));
    }
  };
  const decode = (bytes) => new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  const parseJson = (bytes) => {
    // UTF16 text and decoded-object allowance remain charged until observation
    // (including stdout acknowledgement) settles. No per-event lease escapes.
    const textLease = charge(2 * bytes.length);
    let objectLease;
    try {
      objectLease = charge(2 * bytes.length);
      const value = JSON.parse(decode(bytes));
      return { value, release: objectLease };
    } catch (error) { objectLease?.(); throw error; }
    finally { textLease(); }
  };
  const observeJson = async (bytes, observe) => {
    const parsed = parseJson(bytes);
    try { await observe(parsed.value); } finally { parsed.release(); }
  };
  const readBody = async (response, maximum, observe) => {
    const declared = response.headers.get("content-length");
    if (declared !== null && /^\d+$/.test(declared) && Number(declared) > maximum) {
      await response.body?.cancel().catch(() => undefined);
      throw new Error("MCP body exceeds its software envelope");
    }
    if (!response.body) return;
    const reader = response.body.getReader();
    const pieces = [];
    const leases = [];
    let size = 0;
    let complete = false;
    let joinedLease;
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) { complete = true; break; }
        if (value.byteLength > maximum - size) throw new Error("MCP body exceeds its software envelope");
        leases.push(charge(value.byteLength));
        pieces.push(Buffer.from(value));
        size += value.byteLength;
      }
      joinedLease = charge(size);
      let joined = Buffer.concat(pieces, size);
      pieces.length = 0;
      for (const release of leases.splice(0)) release();
      if (!size) return;
      const parsed = parseJson(joined);
      joined = undefined;
      joinedLease(); joinedLease = undefined;
      try { await observe(parsed.value); } finally { parsed.release(); }
    } finally {
      if (!complete) await reader.cancel().catch(() => undefined);
      reader.releaseLock();
      for (const release of leases) release();
      joinedLease?.();
    }
  };
  const emit = (value) => {
    if (fatal) return Promise.reject(fatal);
    // A finite output queue also covers app-watch notifications, not just
    // replies attached to the ordinary/control admission counters.
    if (writes.size >= ordinaryCount + controlCount + 4) {
      return Promise.reject(fail(new Error("Bridge stdout queue headroom exhausted")));
    }
    let textLease;
    let bytesLease;
    let bytes;
    try {
      const text = `${JSON.stringify(value)}\n`;
      textLease = charge(2 * text.length);
      bytesLease = charge(Buffer.byteLength(text));
      bytes = Buffer.from(text);
    } catch (error) {
      textLease?.(); bytesLease?.();
      return Promise.reject(fail(error));
    }
    const pending = new Promise((resolve, reject) => {
      let settled = false;
      const finish = (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        if (error) reject(fail(error)); else resolve();
      };
      const timer = setTimeout(() => finish(new Error("Bridge stdout did not acknowledge its queued answer")), options.outputTimeoutMs ?? 5000);
      const record = { finish, bytesLease, textLease };
      writes.add(record);
      if (!outputAttached) {
        outputAttached = true;
        output.on("error", onOutputError);
        output.on("close", onOutputClose);
      }
      try {
        output.write(bytes, (error) => { retireWrite(record); finish(error); });
      } catch (error) { finish(error); }
    });
    outputs.add(pending);
    void pending.finally(() => outputs.delete(pending)).catch(() => undefined);
    return pending;
  };
  async function* lines(source, maximum) {
    let pieces = [];
    let leases = [];
    let size = 0;
    const release = () => {
      pieces = [];
      size = 0;
      for (const done of leases.splice(0)) done();
    };
    try {
      for await (const raw of source) {
        const chunk = Buffer.isBuffer(raw) ? raw : Buffer.from(raw);
        let start = 0;
        for (let index = 0; index <= chunk.length; index++) {
          if (index !== chunk.length && chunk[index] !== 10) continue;
          const length = index - start;
          if (length > maximum - size) throw new Error("Bridge line exceeds its software envelope");
          if (length) {
            leases.push(charge(length));
            pieces.push(Buffer.from(chunk.subarray(start, index)));
            size += length;
          }
          if (index < chunk.length) {
            const combined = charge(size);
            try {
              let line = Buffer.concat(pieces, size);
              release();
              if (line.length && line[line.length - 1] === 13) line = line.subarray(0, line.length - 1);
              yield line;
            } finally { combined(); }
          }
          start = index + 1;
        }
      }
      if (size) {
        const combined = charge(size);
        try { yield Buffer.concat(pieces, size); } finally { combined(); }
      }
    } finally { release(); }
  }
  const events = async (response, maximum, observe) => {
    if (!response.body) throw new Error("QC Remote returned an empty MCP event stream");
    const data = [];
    const leases = [];
    let eventBytes = 0;
    const release = () => {
      data.length = 0;
      eventBytes = 0;
      for (const done of leases.splice(0)) done();
    };
    const flush = async () => {
      if (!data.length) { release(); return; }
      // The join is an additional UTF16 allocation; reserve before creating it.
      const joinedLease = charge(2 * eventBytes);
      let parsedLease;
      let value;
      try {
        parsedLease = charge(2 * eventBytes);
        value = JSON.parse(data.join("\n"));
      } catch (error) { parsedLease?.(); throw error; }
      finally { joinedLease(); release(); }
      try { await observe(value); } finally { parsedLease(); }
    };
    try {
      for await (const line of lines(response.body, maximum)) {
        if (!line.length) { await flush(); continue; }
        if (line.length + 1 > maximum - eventBytes) throw new Error("MCP event exceeds its software envelope");
        eventBytes += line.length + 1;
        if (line.length >= 5 && line.subarray(0, 5).toString("ascii") === "data:") {
          const lease = charge(2 * line.length);
          try {
            const value = decode(line.subarray(5)).trimStart();
            if (value) { data.push(value); leases.push(lease); } else lease();
          } catch (error) { lease(); throw error; }
        }
      }
      await flush();
    } finally { release(); }
  };
  const isControl = (message) => message?.method === "ping"
    || message?.method === "notifications/cancelled"
    || message?.method === "notifications/initialized";
  const admit = (message) => {
    if (fatal) throw fatal;
    if (output.destroyed || output.writableEnded) throw fail(new Error("Bridge stdout is no longer available"));
    const bytes = Buffer.byteLength(JSON.stringify(message));
    const reserved = isControl(message);
    if (bytes > (reserved ? controlBytes : requestBytes)) return undefined;
    if ((reserved ? control : ordinary) >= (reserved ? controlCount : ordinaryCount)) return undefined;
    const retained = charge(2 * bytes);
    if (reserved) control++; else ordinary++;
    let held = true;
    return () => {
      if (!held) return;
      held = false;
      if (reserved) control--; else ordinary--;
      retained();
    };
  };
  return {
    input, output, abort, diagnostics, charge, decode, observeJson, readBody, events, lines, emit, admit, isControl,
    fetch: options.fetch ?? globalThis.fetch,
    watchIntervalMs: options.watchIntervalMs ?? 2000,
    requestBytes, batchBytes, controlBytes, replyBytes, controlReplyBytes,
    signal: (within, extra) => AbortSignal.any([abort.signal, AbortSignal.timeout(within), ...(extra ? [extra] : [])]),
    fail,
    async settleOutput() {
      await Promise.allSettled([...outputs]);
      const until = performance.now() + 500;
      const unknown = () => writes.size || (outputAttached && fatal && output.destroyed && !outputClosed && !output.closed);
      while (unknown() && performance.now() < until) await new Promise((resolve) => setTimeout(resolve, 5));
      if (unknown()) throw new Error("Bridge stdout original writes remain retained after unknown retirement");
      if (outputAttached) {
        output.removeListener("error", onOutputError);
        output.removeListener("close", onOutputClose);
        outputAttached = false;
      }
      if (fatal) throw fatal;
    },
    snapshot: () => ({ used, ordinary, control, pendingOutput: writes.size, retainedBytes, fatal: Boolean(fatal) }),
  };
}



const MAX_DESCRIPTOR_BYTES = 128 * 1024;
const MAX_SESSION_CACHE_BYTES = 16 * 1024 * 1024;
async function readLocalText(file, maximum) {
  const held = await open(file, "r");
  try {
    const bytes = Buffer.alloc(maximum + 1);
    let used = 0;
    while (used < bytes.length) {
      const { bytesRead } = await held.read(bytes, used, bytes.length - used, null);
      if (!bytesRead) break;
      used += bytesRead;
    }
    if (used > maximum) throw new Error("Local MCP document exceeds its software envelope");
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, used));
  } finally { await held.close(); }
}
function readLocalTextSync(file, maximum) {
  const held = openSync(file, "r");
  try {
    const bytes = Buffer.alloc(maximum + 1);
    let used = 0;
    while (used < bytes.length) {
      const count = readSync(held, bytes, used, bytes.length - used, null);
      if (!count) break;
      used += count;
    }
    if (used > maximum) throw new Error("Local MCP document exceeds its software envelope");
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, used));
  } finally { closeSync(held); }
}
let bridgeEnvelope = createBridgeEnvelope();
let runtimeStatePath;

function statePaths() {
  if (runtimeStatePath) return [path.resolve(runtimeStatePath)];
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
// Software limit shared with native local_identity.rs, not a firmware limit.
const MAX_IDENTITY_PROOF_BYTES = 4096;
async function identityEnvelope(response) {
  const declared = response.headers.get("content-length");
  if (declared !== null && /^\d+$/.test(declared) && Number(declared) > MAX_IDENTITY_PROOF_BYTES) {
    await response.body?.cancel().catch(() => undefined);
    throw new IdentityProofError("the published identity proof exceeds its software envelope budget");
  }
  if (!response.body) return "";
  const reader = response.body.getReader();
  let complete = false;
  const parts = [];
  let bytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) { complete = true; break; }
      if (value.byteLength > MAX_IDENTITY_PROOF_BYTES - bytes) {
        throw new IdentityProofError("the published identity proof exceeds its software envelope budget");
      }
      bytes += value.byteLength;
      parts.push(Buffer.from(value));
    }
    return Buffer.concat(parts, bytes).toString("utf8");
  } finally {
    if (!complete) await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
async function proveIdentity(current, stopping) {
  if (!current.identityUrl) return;
  const nonce = randomBytes(IDENTITY_NONCE_BYTES).toString("base64url");
  let answer;
  try {
    const response = await bridgeEnvelope.fetch(current.identityUrl, {
      method: "POST",
      headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify({ nonce }),
      // The genuine listener answers in place; nothing legitimate redirects it.
      redirect: "error",
      signal: bridgeEnvelope.signal(IDENTITY_TIMEOUT_MS, stopping),
    });
    if (!response.ok) {
      await response.body?.cancel().catch(() => undefined);
      throw new IdentityProofError(
        `the published endpoint refused the identity check (HTTP ${response.status})`,
      );
    }
    answer = JSON.parse(await identityEnvelope(response));
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
      valid.push(validateDescriptor(JSON.parse(await readLocalText(stateFile, MAX_DESCRIPTOR_BYTES))));
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
    bridgeEnvelope.diagnostics.write(
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
let stoppingBridge = false;
let startedBridge = false;
const backgroundStop = new AbortController();
const backgroundOriginals = new Set();
function ownBackground(action) {
  if (stoppingBridge) return;
  const original = Promise.resolve().then(action);
  backgroundOriginals.add(original);
  void original.finally(() => backgroundOriginals.delete(original)).catch(() => undefined);
  return original;
}
let warnedToolContract = false;

function requestTimeoutMs() {
  const configured = Number(process.env[TIMEOUT_OVERRIDE]);
  return Number.isFinite(configured) && configured > 0 ? configured : DEFAULT_TIMEOUT_MS;
}

function emitJson(value) {
  return bridgeEnvelope.emit(value);
}

async function consumeEventStream(response, observe, maximum) {
  return bridgeEnvelope.events(response, maximum, observe);
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
  bridgeEnvelope.diagnostics.write(
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

class RequestOutcomeUnknownError extends Error {
  constructor() {
    super("WRITE_OUTCOME_AMBIGUOUS: QC Remote did not return a complete answer after this tool request was dispatched. The action may have completed. Do not retry automatically; inspect the Quad Cortex state before deciding what to do next.");
    this.name = "RequestOutcomeUnknownError";
  }
}

async function typedFailure(response) {
  try {
    let typed;
    await bridgeEnvelope.readBody(response, MAX_DESCRIPTOR_BYTES, (parsed) => {
      if (!parsed || typeof parsed !== "object") return;
      const code = typeof parsed.code === "string" && parsed.code.length <= 128 ? parsed.code : undefined;
      const reason = typeof parsed.reason === "string" && parsed.reason.length <= 256 ? parsed.reason : undefined;
      if (code || reason) typed = { code, reason };
    });
    return typed;
  } catch { return undefined; }
}

async function readiness(current) {
  if (!current.healthUrl) return undefined;
  try {
    await proveIdentity(current);
    const response = await bridgeEnvelope.fetch(current.healthUrl, {
      method: "GET",
      headers: { accept: "application/json", authorization: `Bearer ${current.token}` },
      signal: bridgeEnvelope.signal(HEALTH_TIMEOUT_MS),
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
  bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] HTTP ${status} ${label}
`);
  return adviceForReason(typed.reason) ?? adviceForReason(typed.code) ?? adviceForStatus(status);
}

async function failureFor(response, current, typed) {
  if (typed) return new Error(diagnose(typed, response.status));
  // The status is the diagnosis. Readiness is consulted only for a server-side
  // fault, where the device's state may actually explain it — asking after a
  // 401 would turn a rotated credential into "check your USB cable".
  if (response.status < 500) {
    bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] HTTP ${response.status} with no typed reason
`);
    return new Error(adviceForStatus(response.status));
  }
  const health = await readiness(current);
  const context = health ? adviceForReason(health.reason) : undefined;
  bridgeEnvelope.diagnostics.write(
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
  let serverRejected = false;
  let answered = false;
  let dispatched = false;
  try {
    let response;
    try {
      const body = JSON.stringify(message);
      if (Buffer.byteLength(body) > bridgeEnvelope.requestBytes) throw new Error("MCP request exceeds its software envelope before dispatch");
      const signal = bridgeEnvelope.signal(timeoutMs);
      dispatched = true;
      response = await bridgeEnvelope.fetch(current.url, { method: "POST", headers, body, signal });
    } catch (error) {
      if (error?.name === "TimeoutError" || error?.name === "AbortError") {
        throw new Error(`QC Remote timed out after ${timeoutMs} ms without answering. The app may be busy or not responding.`);
      }
      // Non-tool transport failures retain the offline fallback. A dispatched
      // tool request is instead classified by the outer ambiguity boundary.
      throw new AppUnavailableError(
        `could not reach ${current.url}: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    const nextSession = response.headers.get("mcp-session-id");
    if (nextSession) sessionId = nextSession;
    if (response.status === 202 || response.status === 204) {
      await response.body?.cancel().catch(() => undefined);
      if (message.method === "tools/call" && canReply(message)) throw new RequestOutcomeUnknownError();
      return;
    }
    // A request sent with no session id is answered 422 by the app, so a 404 is
    // only ever "unknown session" - but only when an id was actually sent.
    if (response.status === 404 && sent) {
      await response.body?.cancel().catch(() => undefined);
      throw new SessionExpiredError(sent);
    }
    if (!response.ok) {
      const typed = await typedFailure(response);
      // Only explicit request-level rejection or a complete structured domain
      // error is a known outcome. An untyped/truncated server error may have
      // been raised after the tool's action completed.
      serverRejected = [400, 401, 403, 404, 405, 413, 415, 422].includes(response.status) || typed !== undefined;
      if (message.method === "tools/call" && !serverRejected) throw new RequestOutcomeUnknownError();
      throw await failureFor(response, current, typed);
    }
    const observe = async (value) => {
      // A malformed same-id envelope is not a completed JSON-RPC answer and
      // must not escape before the single ambiguous-outcome reply below.
      if (message.method === "tools/call" && canReply(message) && value?.id === message.id
          && (value.jsonrpc !== "2.0" || (!Object.hasOwn(value, "result") && !Object.hasOwn(value, "error")))) return;
      // Per the MCP spec the server may answer with a version it supports rather
      // than the one that was proposed; later requests must carry that one.
      if (options.negotiate && typeof value?.result?.protocolVersion === "string") {
        protocolVersion = value.result.protocolVersion;
      }
      if (options.tap) options.tap(value);
      if (canReply(message) && value?.jsonrpc === "2.0" && value.id === message.id
          && (Object.hasOwn(value, "result") || Object.hasOwn(value, "error"))) answered = true;
      if (!options.silent) await emitJson(value);
    };
    const contentType = response.headers.get("content-type") ?? "";
    const maximum = bridgeEnvelope.isControl(message) ? bridgeEnvelope.controlReplyBytes : bridgeEnvelope.replyBytes;
    if (contentType.includes("text/event-stream")) await consumeEventStream(response, observe, maximum);
    else await bridgeEnvelope.readBody(response, maximum, observe);
    if (message.method === "tools/call" && canReply(message) && !answered) throw new RequestOutcomeUnknownError();
  } catch (error) {
    // Identity verification above is pre-dispatch and remains an offline
    // outcome. Once fetch has been admitted, neither a reset nor a truncated
    // JSON/SSE answer proves that a tool mutation did not complete.
    if (dispatched && message.method === "tools/call" && !serverRejected && !(error instanceof SessionExpiredError)) {
      // An SSE transport failure after the actual response was delivered must
      // not manufacture a second, conflicting reply for that request.
      if (answered) return;
      throw new RequestOutcomeUnknownError();
    }
    throw error;
  }
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
  let staged;
  let descriptor;
  let owned = false;
  try {
    mkdirSync(path.dirname(file), { recursive: true });
    // Every client on this machine shares one cache file, so two bridges can
    // write it at once. Stage and rename, as the app does for the descriptor,
    // or a concurrent reader can catch a half-written file and fall back to an
    // empty tool list for the rest of its session.
    const contents = JSON.stringify(value);
    if (Buffer.byteLength(contents) > MAX_SESSION_CACHE_BYTES) throw new Error("Local MCP session catalog exceeds its software envelope");
    for (let attempt = 0; attempt < 64; attempt++) {
      staged = `${file}.${process.pid}.${randomUUID()}.tmp`;
      try {
        descriptor = openSync(staged, "wx", 0o600);
        owned = true;
        break;
      } catch (error) {
        if (error?.code !== "EEXIST") throw error;
      }
    }
    if (!owned) throw new Error("Could not reserve a unique local MCP session stage");
    writeFileSync(descriptor, contents);
    fsyncSync(descriptor);
    closeSync(descriptor);
    descriptor = undefined;
    renameSync(staged, file);
    owned = false;
  } catch (error) {
    bridgeEnvelope.diagnostics.write(
      `[qc-remote-mcp] could not remember the local MCP session: ${error instanceof Error ? error.message : String(error)}\n`,
    );
  } finally {
    if (descriptor !== undefined) {
      try { closeSync(descriptor); } catch {}
    }
    if (owned) {
      try { unlinkSync(staged); } catch (error) {
        if (error?.code !== "ENOENT") bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] could not remove the owned session stage: ${error instanceof Error ? error.message : String(error)}\n`);
      }
    }
  }
}

function rememberedSession() {
  const file = sessionCacheFile();
  if (!file) return undefined;
  try {
    const value = JSON.parse(readLocalTextSync(file, MAX_SESSION_CACHE_BYTES));
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
  if (appWatch || stoppingBridge) return;
  let checking = false;
  appWatch = setInterval(() => ownBackground(async () => {
    if (checking || stoppingBridge) return;
    checking = true;
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
      await proveIdentity(await descriptor({ quiet: true }), backgroundStop.signal);
    } catch {
      // Not the announcement's bare catch. That one was the only witness to its
      // own fault, so silence there left no record anywhere. This loop is armed
      // only from the AppUnavailableError branch below, which has already
      // written the whole reason - descriptor paths, parse failure and all -
      // and writes it again on every later request. What this swallows is
      // "still closed", the state the loop exists to wait out, on a clock the
      // user never set.
      return;
    } finally { checking = false; }
    if (stoppingBridge) return;
    clearInterval(appWatch);
    appWatch = undefined;
    void emitJson({ jsonrpc: "2.0", method: "notifications/tools/list_changed" }).catch(bridgeEnvelope.fail);
    void emitJson({ jsonrpc: "2.0", method: "notifications/resources/list_changed" }).catch(bridgeEnvelope.fail);
  }), bridgeEnvelope.watchIntervalMs);
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
    if (error instanceof RequestOutcomeUnknownError) {
      bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] ${error.message}\n`);
      if (canReply(message)) await emitJson({ jsonrpc: "2.0", id: message.id,
        result: { content: [{ type: "text", text: error.message }], isError: true } });
      return;
    }
    if (error instanceof AppUnavailableError) {
      // Expected whenever QC Remote is simply not open. Diagnostics — descriptor
      // paths, transport wording — stay on stderr; the client gets a usable
      // session and one actionable sentence.
      bridgeEnvelope.diagnostics.write(
        `[qc-remote-mcp] QC Remote is not reachable: ${error.detail ?? error.message}\n`,
      );
      if (canReply(message)) await emitJson(offlineReply(message));
      if (handshakeRequest) watchForApp();
      return;
    }
    const detail = error instanceof Error ? error.message : String(error);
    bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] ${detail}\n`);
    if (canReply(message)) await emitJson(unavailable(message, detail));
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
    const response = await bridgeEnvelope.fetch(current.url, {
      method: "DELETE",
      headers: {
        authorization: `Bearer ${current.token}`,
        "mcp-session-id": id,
        ...(protocolVersion ? { "mcp-protocol-version": protocolVersion } : {}),
      },
      signal: bridgeEnvelope.signal(HEALTH_TIMEOUT_MS),
    });
    await response.body?.cancel().catch(() => undefined);
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
    bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] this bridge was started without ${CLIENT_ID_ARGUMENT}, so QC Remote cannot credit its heartbeat to a host and the LOCAL panel cannot show that host as connected.
`);
    return;
  }
  const report = announcementReport();
  const startedAt = performance.now();
  let checking = false;
  const announce = () => ownBackground(async () => {
    if (checking || stoppingBridge) return;
    checking = true;
    try {
      const line = report.observe(client, await attemptAnnouncement(client, backgroundStop.signal), performance.now() - startedAt);
      if (line) {
        bridgeEnvelope.diagnostics.write(`${line}
`);
      }
    } catch {
      // The announcement is telemetry, never a dependency: a fault in the
      // reporting must not take the MCP session with it, and the next tick
      // reports the same thing again anyway.
    } finally { checking = false; }
  });
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
async function attemptAnnouncement(client, stopping) {
  let current;
  try {
    current = await descriptor({ quiet: true });
  } catch (error) {
    if (error?.detail) return error.detail;
    return error instanceof Error ? error.message : String(error);
  }
  if (stopping?.aborted || stoppingBridge) return "the bridge is shutting down";
  if (!current.rpcUrl) return "the published descriptor names no /rpc endpoint to announce to";
  try {
    await proveIdentity(current, stopping);
  } catch (error) {
    return error?.detail ?? (error instanceof Error ? error.message : String(error));
  }
  let response;
  try {
    response = await bridgeEnvelope.fetch(current.rpcUrl, {
      method: "POST",
      headers: {
        authorization: `Bearer ${current.token}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ rpc: "local.bridgeHeartbeat", params: { client } }),
      signal: bridgeEnvelope.signal(HEALTH_TIMEOUT_MS, stopping),
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
  await response.body?.cancel().catch(() => undefined);
  if (!response.ok) return `QC Remote refused the announcement (${response.status})`;
  return undefined;
}

async function main(options = {}) {
  if (startedBridge) throw new Error("This bridge module already owns a main invocation");
  startedBridge = true;
  // Electron's utility host proxies stdin's public event/read methods, but its
  // native async iterator still sees the original Socket's internal state.
  // Wrap exactly once, when starting, not while merely importing this library.
  const hostedInput = !options.envelope && !options.input && process.parentPort
    ? process.stdin.pipe(new PassThrough())
    : undefined;
  bridgeEnvelope = options.envelope ?? createBridgeEnvelope({
    ...options,
    input: options.input ?? hostedInput ?? process.stdin,
  });
  runtimeStatePath = options.stateFile;
  const inFlight = new Set();
  const own = (pending, release = () => {}) => {
    inFlight.add(pending);
    void pending.catch((error) => bridgeEnvelope.fail(error)).finally(() => {
      release();
      inFlight.delete(pending);
    });
  };
  try {
    for await (const line of bridgeEnvelope.lines(bridgeEnvelope.input, bridgeEnvelope.batchBytes)) {
      if (!line.length) continue;
      try {
        await bridgeEnvelope.observeJson(line, (message) => {
          // Legacy batches retain individual replies and do not wait behind an
          // admitted tool. The whole line and each individual request are bounded.
          const batch = Array.isArray(message) ? message : [message];
          for (const single of batch) {
            const release = bridgeEnvelope.admit(single);
            if (release) { own(handle(single), release); continue; }
            // Control overrun cannot silently lose a cancellation. A caller with
            // no request id also cannot be told that refusal was delivered.
            if (bridgeEnvelope.isControl(single) || !canReply(single)) {
              throw new Error("Bridge control admission headroom exhausted");
            }
            own(emitJson({ jsonrpc:"2.0", id:single.id, error:{
              code:-32000, message:"QC Remote bridge is busy; this request was not dispatched.",
              data:{requestSent:false,retryable:true}
            }}));
          }
        });
      } catch (error) {
        if (error instanceof SyntaxError) {
          bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] ${error.message}\n`);
          continue;
        }
        throw error;
      }
    }
  } catch (error) { bridgeEnvelope.fail(error); }
  finally {
    if (hostedInput) {
      process.stdin.unpipe(hostedInput);
      process.stdin.pause();
      hostedInput.destroy();
    }
    stoppingBridge = true;
    if (appWatch) { clearInterval(appWatch); appWatch = undefined; }
    if (announcing) { clearInterval(announcing); announcing = undefined; }
    // Separate cancellation: EOF must not abort an accepted ordinary tool.
    // Descriptor OS I/O has no hard cancellation claim; its exact background
    // promise remains owned here until it really settles.
    backgroundStop.abort(new Error("Bridge telemetry is stopping"));
    await Promise.allSettled([...backgroundOriginals]);
    await Promise.allSettled([...inFlight]);
    await releaseSession();
    try { await bridgeEnvelope.settleOutput(); }
    finally {
      if (!await bridgeEnvelope.diagnostics.stop()) throw new Error("Bridge diagnostic original writes remain retained after unknown retirement");
    }
  }
}

module.exports = { createBridgeEnvelope, main };
// Claude's built-in Node host sets argv[1] to this entry point, then dynamically
// imports it from its own bootstrap. In that host require.main is NOT this
// module. Still keep ordinary library imports inert for embedded/test callers.
if (require.main === module || (process.argv[1] && path.resolve(process.argv[1]) === __filename)) main().catch((error) => {
  bridgeEnvelope.diagnostics.write(`[qc-remote-mcp] ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
