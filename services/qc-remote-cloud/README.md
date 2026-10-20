# QC Remote Cloud

Cloudflare edge relay for QC Remote. It never speaks USB: Windows and Android
remain the USB owners and establish an outbound, hibernatable WebSocket to this
service. Every host is isolated under its verified Google account.

## Safety boundary

The Cloudflare edge never speaks USB. A paired Windows or Android host owns USB
and opens an outbound WebSocket to the relay. Device credentials are stored only
on that host; the relay stores only their SHA-256 hashes.

`/mcp` is an OAuth-protected resource. Google is the upstream product identity
provider; Cloudflare hosts the relay and the MCP OAuth authorization server but
does not authenticate end users. The Worker publishes protected-resource
metadata at `/.well-known/oauth-protected-resource/mcp`.
`QCRC_ADMIN_TOKEN` is an administrative bootstrap credential for the
single-owner compatibility deployment; it is never sent to an MCP client or
embedded in either app, and the routes it unlocks do not exist when
`QCRC_ENVIRONMENT` is `production` (they answer 404 whatever the token).

## MCP polling and request costs

Unchanged foreground and background views must generate no recurring cloud
reads. Display last-observed local state, identify stale observations, and refresh
on explicit demand; command planning and guard reads must still use fresh device
state. A retry interval is a safety limit, not a polling schedule.

Hosts sample USB readiness locally and publish one initial `ready` frame per
connection and subsequent readiness changes only. WebSocket protocol pings keep
the connection alive without calling the Durable Object message handler or
consuming request quota; repeated JSON readiness heartbeats do consume quota.

Every explicit read and control still passes OAuth, scope checks and the account
fence. No device result or successful authorization is cached. Admitted MCP
requests record the immutable client registry inside account admission, reducing
an ordinary device call from three Durable Object RPCs to two without removing
an authorization check. There is no new minimum interval for fresh reads.

Access tokens expire after one hour. Refresh grants, client registrations and
paired-host credentials persist until explicitly revoked. Valid historical
credentials migrate on authenticated use; expired or revoked credentials do not
revive. Device-directory KV expiration is removed once during migration instead
of being renewed periodically. Revocation and account-generation fences remain
authoritative for every request.

## Cloudflare KV namespace id

`wrangler.jsonc` binds `QCRC_OAUTH` and `OAUTH_KV` (the fixed name the OAuth
provider library expects) to the same KV namespace, but the committed file
never carries that namespace's real id -- only the placeholder
`REPLACE_WITH_QCRC_OAUTH_KV_NAMESPACE_ID`. Wrangler has no built-in way to
interpolate an environment variable into a binding id in `wrangler.jsonc`
(that mechanism exists for `vars`/secrets, not structural bindings like KV,
D1, or R2), so every `wrangler` invocation here goes through a generated,
gitignored `wrangler.local.jsonc` instead:

```powershell
npm run kv:configure   # reads QCRC_OAUTH_KV_NAMESPACE_ID, writes wrangler.local.jsonc
```

`npm run deploy` and `npm run dev` run this step first automatically, and
`npm run deploy` then re-reads `wrangler.local.jsonc` with
`apply-kv-namespace-id.mjs --verify-deployable`, refusing to deploy a config that
binds the template token or the dry-run placeholder below.

**Dry runs:** `npm run check` and `npm run deploy:dry` deploy nothing, so they
use `npm run kv:configure:dry-run` instead, which writes a separate gitignored
`wrangler.check.jsonc`. It binds the real id when `QCRC_OAUTH_KV_NAMESPACE_ID`
is set, and otherwise the labelled all-zero placeholder
`00000000000000000000000000000000` (with a NOTE saying so), which is enough for
`wrangler types --check`, `tsc` and `wrangler deploy --dry-run`. The placeholder
never goes into `wrangler.local.jsonc`, and `kv:configure` refuses it even when
it is set explicitly. `npm test` pins that split.

**Local development:** put the real namespace id in your shell environment
before running any of those scripts (find it with
`npx wrangler kv namespace list`, or in the Cloudflare dashboard under
Workers & Pages > KV). On Windows, `setx QCRC_OAUTH_KV_NAMESPACE_ID <id>` keeps
it for future shells; in POSIX shells, export it from your profile.

**Not in a `.env` file, and `kv:configure` refuses to run beside one.** Wrangler
loads `.env` into the Worker's `vars`, so a `.env` carrying this id makes
`wrangler types` emit `QCRC_OAUTH_KV_NAMESPACE_ID: string;` into
`Cloudflare.Env` and add it to `ProcessEnv`. `worker-configuration.d.ts` is
committed and `npm run check` runs `wrangler types --check`, which then fails on
two lines the committed file does not have. Measured on wrangler 4.130.0. The id
is not a Worker variable in the first place: it is a build-time substitution into
a KV binding id, which is why `kv:configure` writes it into
`wrangler.local.jsonc` rather than handing it to the Worker. Regenerating the
committed types with the variable present would also make the chain pass, and is
the wrong fix -- it bakes one developer's local-only build input into the public
type surface of the Worker. With no `.env` and the id in the environment, the
whole chain passes: types up to date, `tsc --noEmit` clean, and
`wrangler deploy --dry-run` resolving both KV bindings.

**CI (GitHub Actions):** the `cloud-relay` job in `.github/workflows/ci.yml`
installs from the repository root (`npm ci`; this service is an npm workspace
and is locked by the root `package-lock.json`, not a lockfile of its own), then
runs `npm test` and `npm run check` for the workspace
(`--workspace @qc-remote/cloud`). It exports the
`QCRC_OAUTH_KV_NAMESPACE_ID` repository secret when one is configured; on a new
repository or a fork pull request the secret is empty and the check runs against
the dry-run placeholder. CI never deploys.

## Deploy

```powershell
npm install
npm run kv:configure
npx wrangler check
npx wrangler secret put QCRC_ADMIN_TOKEN
npx wrangler secret put GOOGLE_CLIENT_ID
npx wrangler secret put GOOGLE_CLIENT_SECRET
npx wrangler secret put COOKIE_ENCRYPTION_KEY
npm run deploy
```

`qcrc.freevia.org` is configured as a Worker Custom Domain. Cloudflare creates
the DNS record and certificate during deploy. Do not add a manual CNAME for
that hostname.

## Rate limits

The unauthenticated routes that write to KV are limited per client address
through Workers rate-limiting bindings declared in `wrangler.jsonc`
(`src/rate-limit.ts`): `QCRC_PAIRING_RATE_LIMIT` (10 per minute for each of
`POST /v1/host/pairing-sessions`, `POST /pair/sign-in` and `POST /pair/cancel`)
and `QCRC_OAUTH_RATE_LIMIT` (30 per minute for each of `GET /authorize` and
`POST /register`). An exceeded limit answers 429 with `Retry-After: 60`. The
bindings are created by `wrangler deploy` and need no dashboard setup. In
production a missing binding makes those routes answer 503 rather than run
unprotected; outside production it is skipped.

## Pairing a host

A host creates a pairing session (`POST /v1/host/pairing-sessions`, optionally
with `{"label": "...", "platform": "..."}`) and opens the returned `pairUrl`.
That page names the device as the app reported it, when and from where the
request was made, and that it will join the Google account chosen next; only
**Continue** reaches Google sign-in, and **Cancel** ends the session. A session
completes once and expires after ten minutes.

## Administrative bootstrap API

These endpoints exist only outside production. With `QCRC_ENVIRONMENT` set to
`production` every one of them answers 404, with or without a token.
Elsewhere they accept `Authorization: Bearer <QCRC_ADMIN_TOKEN>`.
`POST /v1/device/pair` intentionally does not: its short-lived, single-use
pairing code is the host's authorization, so no owner secret is ever copied
into an app.

- `GET /health` is public and exposes no device information.
- `POST /v1/pairing/offers` creates a single-use, ten-minute pairing offer.
- `POST /v1/device/pair` redeems an offer and returns a device credential.
- `GET /v1/device/directory` lists every paired device with the account that
  owns it. A host reaches its owner through this directory and an assistant
  through the Google subject in its token; when those are different accounts
  the device heartbeats into a namespace nobody reads, and this is the only way
  to see that from outside the app.
- `GET /v1/device/status` returns connected-device metadata for the
  single-owner compatibility namespace. Account-based deployments should use
  the directory above.
- `POST /v1/device/command` routes a contract-validated command to the one
  ready paired host. The method must name a contract action and the arguments
  are checked against that action's published schema. The wait is derived per
  method from the host's own timing profile, not a fixed ten seconds: a preset
  read resolves in under two minutes, a setlist duplicate may legitimately take
  twenty.
- `GET /v1/device/connect?deviceId=...` upgrades to the outbound device socket.

The device socket uses the same bearer credential returned from pairing. It
uses Cloudflare Durable Object WebSocket hibernation, so an idle connection
does not accrue Durable Object duration. Account-specific owner namespaces
ensure a paired host cannot be reached by another Google account.

The host protocol is intentionally compact JSON and exactly matches
`qc-relay-protocol`: the host sends `ready` and correlated `result` frames; the
relay sends correlated `invoke` frames. A device id is resolved through a short
directory record to its owner Durable Object, so a device socket cannot cross
an account boundary when public account identity is enabled.

## Google OAuth client

Create one **Web application** OAuth client in the Google Cloud project. Its
only authorized redirect URIs are:

```
https://qcrc.freevia.org/auth/google/pair/callback
https://qcrc.freevia.org/auth/google/mcp/callback
```

Store its generated client ID and secret as `GOOGLE_CLIENT_ID` and
`GOOGLE_CLIENT_SECRET`. The Worker always asks Google to select an account,
validates the signed ID token against Google's JWKS, requires a verified email,
and scopes each paired host to that Google subject. Cloudflare Access is not
part of the user-facing sign-in flow.
