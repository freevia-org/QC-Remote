# Privacy Policy

Last updated 17 October 2026

QC Remote is an independent controller for the Neural DSP Quad Cortex. This
policy explains what data the software handles, what stays on your device,
exactly what the optional remote relay sends and to whom, what changes when you
connect an AI app, how long the relay keeps what it receives, and how to have it
deleted.

The same policy is published at <https://freevia.org/qcremote/privacy>. This
file and that page are the same policy.

## Who we are

QC Remote is published by Freevia EOOD ("Freevia", "we", "us"). Freevia EOOD is
the publisher and the operator of the optional remote relay at
`qcrc.freevia.org`: EIK 208913638, 6 Tsanko Tserkovski St, Sofia 1164, Bulgaria.
Freevia decides what the relay stores and why, and is responsible for the data
in the section "What the relay handles". Freevia has no access to the data the
apps handle only on your own computer or phone. Licensing and trademark
information is in [`LICENSE`](LICENSE) and [`TRADEMARKS.md`](TRADEMARKS.md).

Privacy questions and requests: **privacy@freevia.org**. Security reports:
**security@freevia.org**, as described in [`SECURITY.md`](SECURITY.md).

## What this policy covers

- the QC Remote app for Windows;
- the QC Remote app for Android;
- the local MCP connection that the Windows app offers to AI apps installed on
  the same computer;
- the optional remote relay that Freevia operates at `qcrc.freevia.org`, built
  on Cloudflare Workers.

It does not cover the AI apps you connect, Google, Cloudflare, GitHub or Neural
DSP. Each has its own policy. QC Remote is not affiliated with Neural DSP. The
app forwards internet requests made by the Quad Cortex, including account
sign-in, authenticated cloud requests and CorOS updates. A background component
of the Windows app, and the Android app itself, carry these requests over your
computer's or phone's connection, as described under "Quad Cortex internet
requests through this computer or phone".

## In short

- Using the apps with a Quad Cortex over USB sends nothing to Freevia.
- The relay is off until you pair a computer or phone with it by signing in with
  Google. Only then does any data reach Freevia's relay, and it sends only what
  is described below, to Google and to the relay Freevia operates.
- The relay passes commands and device responses through in memory and does not
  store their contents.
- The apps and the relay contain no advertising, analytics or crash reporting.
- When a Quad Cortex is connected to the Windows or Android app by USB, its own
  account, cloud and CorOS update requests can use your computer's or phone's
  internet connection. This is on by default.
  Requests may contain your Neural DSP account email, password, tokens, the
  unit's serial number and other data the unit supplies. They go directly to
  the addresses requested by the unit. Freevia's relay does not receive them,
  and the apps keep no copy of them. You can switch this off under "Internet for
  the Quad Cortex" (Windows) or "Allow the Quad Cortex to use this device's
  Internet connection" (Android).
- You can unpair a device, unlink an AI app, or ask by email for everything the
  relay holds for your Google account to be deleted.

## Data that stays on your computer or phone

Connecting a Quad Cortex over USB, controlling it, editing presets, local
`.qcw` workspaces, diagnostics exports, and the local MCP connection all run on
your computer or phone. Device state, commands, local preferences and
diagnostics exports stay on your computer or phone in app-private storage. The
Windows app's window loads only its own files and makes no outside network
connections of its own. The exception is internet traffic requested by the Quad Cortex. The Windows
app's background component, and the Android app, open those connections on the
unit's behalf and return the responses over USB. The destinations are supplied
by the unit; there is no domain allowlist. See "Quad Cortex internet requests
through this computer or phone" below.

QC Remote keeps these items on the device it runs on:

- **Relay pairing and access settings.** On Windows, the relay credential and
  the remote access level are stored in Windows Credential Manager. On Android,
  the relay credential is encrypted with a non-exportable Android Keystore key.
  Unpairing removes the stored credential.
- **Local connection details (Windows).** The local MCP endpoint listens only on
  `127.0.0.1` and requires a locally generated access token. The details are
  kept in a file in your local application data folder that is restricted to
  your Windows user (other programs running as you can still read it).
- **Status and troubleshooting files.** A runtime health record, the
  background component's error output, and a short USB flight recorder stay in
  app-private storage on your computer or phone. The flight recorder contains
  timestamps, process numbers, event names, message type numbers and report
  counts, rather than message contents. On Windows it also notes, for each
  request the unit makes for its Neural DSP account, a request number, the
  HTTP method, the time limit, the answer status and the time remaining, and
  each sign-in state change the unit reports (created, updated or removed).
  It does not record URLs, header values, request or response bodies, or
  tokens. It keeps at most 256 entries and up to four frozen incident
  windows on Windows. Cloud/account forwarding envelopes and
  firmware-forwarding requests are excluded from the general message cache
  and event stream. The production forwarder does not keep request or response
  bodies, header values or URLs. Its displayed status has aggregate counts and
  transfer state. Other programs running as your Windows user can read the
  local troubleshooting files.
- **Demo device.** The Windows app's built-in demo device keeps its setlists and
  presets in `%LOCALAPPDATA%\QC Remote\virtual-device`.
- **Files you ask for.** Workspaces you save, device backups, and diagnostics
  exports are written where you choose. A device backup requested by an AI app
  is saved on the computer or phone that holds the USB connection (on Windows,
  in `Documents\QC Remote\Backups`); only its file name, location and size are
  reported to the AI app. A diagnostics export is a file you save and send
  yourself. It contains the app version, platform, connection state, a few
  device state values (such as preset position, scene, tempo, number of blocks)
  and a short list of connection events, and omits serial numbers, MAC
  addresses, user names, file paths, and preset and setlist names.
- **AI app configuration (Windows).** When you pair a desktop AI app with the
  local connection, QC Remote writes its own entry into that app's
  configuration. Unpairing removes it.

On Android, the app's data is excluded from Android cloud backup and
device-to-device transfer.

This data is not sent to Freevia, Neural DSP or anyone else by QC Remote. It
stays until you remove it: delete the files, unpair, or uninstall the app.

### Local AI apps

A desktop AI app that you pair locally (for example Claude Desktop, ChatGPT
Desktop/Codex, Google Antigravity, Bionic/LM Studio, or Jan) talks to QC Remote
over loopback or standard input/output on the same computer. That AI app then
sends the tool results it receives, which can include preset and setlist names
and device state, to its own provider under its own policy.

## What the relay handles

The relay is optional and is used only after you pair a computer or phone with
it. The paired Windows or Android app keeps the USB connection to the Quad
Cortex and opens an outbound secure WebSocket (TLS) to `qcrc.freevia.org`; the
relay never speaks USB. AI apps you connect (such as Claude or ChatGPT) reach
your unit through the relay.

**Who receives it.** Google provides the identity service you sign in with.
Freevia operates the relay at `qcrc.freevia.org`, using Cloudflare Workers,
Durable Objects and KV storage. Cloudflare processes relay traffic and stores
sampled operational logs for Freevia as its infrastructure provider.

**What is sent, and why:**

| Data | Purpose |
| --- | --- |
| A one-time pairing code | Proves the computer and the account belong together, once, at pairing time |
| A generic label naming the app host (`QC Remote on Windows` or `QC Remote on Android`) | Lets you tell paired devices apart in the device list |
| A relay credential | Authenticates the paired computer to the relay on every reconnect. Stored at the relay only as a SHA-256 hash, so the relay cannot recover the credential from what it stores |
| Your Google account subject ID, verified email and profile name | Identifies which account owns which paired devices and AI app authorizations, so remote access is scoped to you |
| An OAuth bearer token, if you connect an AI app | Lets that app reach your paired device through the relay, scoped to the permissions you approved |
| Device commands and device-state responses | This is the remote control itself: what you asked the device to do, and what it reported back. Persistent hardware identifiers are stripped before this leaves the relay; full device-backup payloads are never forwarded through this path |
| A daily count of remote commands | Shown back to you in the app, and used to watch for abuse of the relay |
| MCP client name and version, normalized device/browser, connecting IP address, coarse city/country, and recent request activity | Shows which clients are currently using Remote MCP in the paired host's tray. The host receives an opaque session ID and first/last activity times, never the client's bearer token or its private digest |
| The same normalized device/browser, connecting IP address, and coarse city/country when an AI app is authorized | Lets the REMOTE panel identify a durable linked-app grant so you can recognize and unlink it |
| Sampled relay invocation logs, which can include request time, method, URL and query string, response status, and error details. An OAuth callback URL can contain a short-lived authorization code and state value | Helps Freevia troubleshoot failures, monitor relay operation and investigate abuse. Cloudflare stores these Workers Logs separately from the relay account records described below |

**What is never sent through the relay:** anything from local-only use, such as
your local preferences or a diagnostics export you generate on your own machine.
Full device backups are withheld from remote command responses.

### Pairing a computer or phone

While you pair, the relay holds a short-lived pairing record: a random session
id, a hash of a secret that only your app holds, the label and platform your app
declares (`QC Remote on Windows` or `QC Remote on Android`), the time, and the
approximate city and country of the request, which Cloudflare derives from the
connecting IP address. When you finish signing in, the record also holds your
email address, a short label of the browser you approved it in (such as
"Windows Chrome"), and a one-time pairing code. The relay keeps only a hash of
the code for redemption; when a computer or phone pairs by QR code or link, the
code itself is held only until your app collects it, once, and is then removed.
Pairing records expire after about ten minutes. The approximate location is used
to show you where a pairing request came from, and the pairing page warns you if
the request and your browser are in different countries.

### Your Google account

Signing in to pair or to connect an AI app is done at Google. The relay asks
Google for the `openid email profile` scopes and receives a signed identity
token. It uses Google's stable account identifier (the "subject"), your verified
email address and your name; it does not store Google's tokens and never sees
your Google password. The Google subject identifies your relay account: the
relay keeps each account's data in storage named for it.

### Paired computers and phones

For each paired device the relay keeps: the device label, the time it was
paired, your Google account subject ID and verified email address, the browser
label from pairing, and its credential **as a SHA-256 hash only**; the
credential itself exists only on your device. A directory entry maps the device
to your account. A credential is accepted for 90 days after pairing; after that
it stops being accepted unless the device is paired again, but that expiry does
not delete the record. The device record and directory entry are kept for as
long as the device stays paired, with no automatic deletion, and are removed
when you unpair the device.

### AI apps you connect

When you allow an AI app, the relay keeps an authorization record with: the
app's name as it declares it, the access you granted (`qc:read` to read your
unit, `qc:control` to change it, `offline_access` to stay connected), your
Google subject, email address and name, the time, a label of your browser and
system (such as "Windows Chrome" with the browser version), your IP address,
and the approximate city and country of the authorization. This lets the AI app
reach your unit and lets you recognise and remove it. An access token lasts one
hour; an authorization and its refresh token last 30 days from the time you
approved it, however often the app is used or refreshed. After that you sign in
again.

An AI app also registers details about itself with the relay (its name and the
addresses it redirects to). These describe the app, not you, and are not tied to
your Google account.

### AI app activity

While an AI app uses the relay, the relay keeps a list of recent clients so that
your paired app can show which assistants are connected: the name and version
the app declares, a browser and system label, IP address, approximate city and
country, the first and latest activity times, and which authorization it used.
The recent-client list expires after 15 seconds without requests; a request
still in progress renews that period. The relay remembers at most 128 client
records (normalized client, device and network details, and private
credential/session digests) for 30 days after their last activity so that later
requests can retain their identity. This activity cache is separate from OAuth
grants and does not grant or extend access. The paired host receives an opaque
session ID and the first and last activity times, never the client's bearer
token or its private digest.

### Commands and responses

When an AI app reads or changes your unit, its request travels through the relay
to your paired app, and the unit's response travels back. These can include the
names of your presets, setlists, captures and impulse responses, the contents of
the loaded preset, device settings and state, and an image of the unit's screen
if the AI app asks for one. The relay holds them in memory only while delivering
them and does not store their contents or write a log of them. Because the relay
ends the secure connection on each side, it and Cloudflare, which hosts it, can
technically read this traffic while it is being delivered.

Before a response reaches an AI app, the relay removes the device name and
serial number fields, and replaces the contents of a device backup with a note
that the backup was saved on your computer or phone.

The relay does record, per account, a count of remote commands for the current
day with the time of the latest command (shown to you; used to monitor abuse).
It contains no names or device data.

### IP addresses

Your IP address is used as the key of Cloudflare's rate-limiting counters on
routes that need no credential (pairing, sign-in, and AI app registration and
authorization). The limits are 10 requests per minute for the pairing routes and
30 per minute for the AI app routes, per route and address. The relay writes no
record of the address for this purpose. The relay stores your IP address only in
the AI app authorization records and the client records described above;
Cloudflare, as host, also sees it.

### Cookies

The relay's web pages set only short-lived functional cookies (HttpOnly,
Secure) that tie a pairing or sign-in step to the browser that started it. They
expire after ten minutes. There are no tracking cookies.

### Operational logs

The relay samples approximately 1% of its invocations for Cloudflare Workers
Logs, which Cloudflare stores for Freevia separately from the relay account
records described above. A sampled entry can include the request time, method,
URL and query string, response status, and error details; the URL of an OAuth
callback can contain a short-lived authorization code and state value. These
logs help Freevia troubleshoot failures, monitor relay operation and investigate
abuse. Cloudflare retains Workers Logs for the period set by its service plan
(3 days on Workers Free, 7 days on Workers Paid), and Freevia cannot delete
individual entries. The relay also writes failure messages that carry an event
name and error text with anything shaped like a credential removed.

## Approximate location

When you use the remote relay, it stores the city and country that Cloudflare,
which hosts the relay, derives from the IP address you connect from. This is
recorded when you connect an AI app, while an AI app is using the relay, and
when you start pairing a computer or phone. It is used only to let you tell your
connected AI apps and paired devices apart, and to help you spot a sign-in or
pairing request from somewhere unfamiliar: the pairing page warns you when the
request and the browser approving it are in different countries. It is not
shared with anyone else and is never sold. It is deleted with the record it
belongs to: for a pairing request, after at most 10 minutes; for a connected AI
app, when you unlink that app, when you unpair the last device on your account,
or 30 days after you authorized the app; for client records, when the record
expires as described above. You can ask for it to be deleted sooner at
privacy@freevia.org.

## What we do not do

We do not sell personal data. We do not run advertising or behavioral analytics
inside QC Remote. We do not read the content of your presets or device backups
for any purpose other than relaying the command you issued.

We also do not collect:

- usage statistics, tracking or crash reports; QC Remote sends no crash, fault
  or usage report to Neural DSP or anyone else;
- USB traffic, presets, setlists or backups: the relay does not store them;
- a device serial number or device name for AI apps: neither is passed on;
- payment details;
- your Google password or Google's tokens.

## Third parties and hosting

- **Cloudflare** hosts the relay (Workers, Durable Objects and Workers KV),
  handles its secure connections, and stores its sampled operational logs.
- **Google** authenticates you when you sign in to the relay. Google receives
  what your browser and the relay send during that sign-in and handles it under
  Google's own policy.
- **AI apps you connect** (for example Claude, ChatGPT, Gemini or Grok) receive
  the data your commands and responses contain, within the access you granted.
  What they do with it is governed by their own privacy policies, not this one.
- **GitHub** receives what you submit if you file an issue or feature request.
  Issues are public. The Windows app can pre-fill the form with the app version
  and a redacted diagnostics summary, but nothing is posted until you submit it.
- **Neural DSP and destinations requested by the Quad Cortex** receive the
  unit's internet requests when forwarding is enabled. Neural DSP handles
  account sign-in and cloud services; firmware and changelog downloads may use
  its content delivery providers. See the next section.

The apps open other websites, such as an AI app's download page, only when you
choose to.

### Quad Cortex internet requests through this computer or phone

When connected by USB, the Quad Cortex can ask the Windows background component
or the Android app to make HTTP requests on its behalf. With "Internet for the
Quad Cortex" (Windows) or "Allow the Quad Cortex to use this device's Internet
connection" (Android) on, QC Remote forwards the unit's account sign-in and
cloud requests and its CorOS update requests (checking for an update, change
notes and the update download) and sends the server responses back to the unit.
The unit's cloud backup and log uploads are not forwarded. Server responses,
such as firmware, are passed to the unit without being inspected.

- **Data and recipients.** The unit supplies each destination, method, headers
  and body. These may include account email and password during sign-in,
  authentication tokens, signed URLs, device identifiers and cloud content.
  The receiving server also sees your computer's public IP address. These
  exchanges go directly from your computer to the requested server, through
  neither Freevia's relay nor an AI provider.
- **Forwarding behavior.** The component passes the supplied request headers
  and bodies on unchanged and does not keep them. For Neural DSP HTTPS endpoints
  it adds the client type/version metadata required for compatibility when the
  unit did not supply it. HTTP
  framing and the USB protocol envelopes are handled by the transport. It does
  not follow redirects, add a cookie jar, retry requests, decompress HTTP bodies,
  or inspect firmware contents. HTTPS uses platform certificate verification;
  a device-supplied HTTP URL is forwarded as HTTP.
- **Destinations come from the unit.** There is no domain, address or port
  allowlist. Device requests may address Neural DSP, its download providers or
  another destination, including a local-network address. Headers and bodies
  go to the destination the unit requested.
- **No traffic history.** The production forwarder processes traffic in memory
  without saving credentials, URLs, headers or message bodies to traffic logs.
  Account/cloud envelopes and firmware-forwarding requests do not enter the
  general message cache or event stream. Aggregate status counts remain local.
- **Your control.** Forwarding is on by default, including after you update from
  an earlier version. Clear "Internet for the Quad Cortex" in Windows Settings,
  or "Allow the Quad Cortex to use this device's Internet connection" in the
  Android app's Device connection panel, to stop it, including a transfer in
  progress. With it off on Windows, QC Remote still answers the unit's account
  requests with an error so the unit is not left waiting. It sends nothing to
  the network and keeps nothing.
  This forwarding feature is separate from the optional Freevia remote relay.

### Third-party AI apps you connect

If you connect an AI app (for example, an MCP-capable assistant) to the relay,
you are granting that app the scopes you approve at that time: reading your Quad
Cortex's status and library, or also changing it. That grant is separate from
pairing a computer: pairing attaches a computer to an account, connecting an app
attaches that app to the same account. Removing the connector in that app's own
settings revokes its access. Freevia does not control what that third-party app
itself does with data it receives through the connection, and its own privacy
policy governs that.

## Retention and deletion

| Data | Kept until |
| --- | --- |
| Pairing request and one-time pairing code | At most about 10 minutes after the pairing starts; the code is removed as soon as your app collects it |
| Sign-in state for a pairing or authorization | 10 minutes |
| Paired-device record (Google account subject ID, verified email, hashed credential, device label, browser label) and its directory entry | You unpair the device. There is no automatic deletion: the credential stops being accepted 90 days after pairing, but the record stays until you unpair |
| AI app authorization, including the device and network details recorded when you approved it | You unlink or revoke it, you unpair your account's last device, or 30 days after you approved it; refreshing does not extend this. Its access token lasts one hour |
| Recent-client list | 15 seconds without requests from the client; a request still in progress renews it |
| Client records (at most 128: normalized client, device and network details, and private credential/session digests) | 30 days after the client's last activity |
| Daily command count | You ask for your remote-account data to be deleted |
| Cloudflare Workers Logs (about 1% of invocations) | The period set by Cloudflare's service plan: 3 days on Workers Free, 7 days on Workers Paid |

### What happens when you delete

- **Unpair a computer or phone.** In the app, open REMOTE and press Unpair. The
  app tells the relay, which withdraws the device's credential at once and
  deletes its stored pairing record and directory entry. If you unpair your
  account's last device, the relay also retires every AI app authorization on
  the account, since they would otherwise remain valid against an account with
  nothing paired to it. If the relay cannot be reached, the app still removes
  its own stored credential; the relay's record then stays until you ask for it
  to be deleted by email. Unpairing does not delete the daily command count or
  the client records.
- **Unlink an AI app.** In the Windows app's REMOTE panel, press Unlink next to
  the app. This revokes its authorization and removes its grant and the device
  and network details recorded with it. You can also remove QC Remote inside the
  AI app.
- **Ask for your remote-account data to be deleted.** Email
  **privacy@freevia.org** and ask to delete your QC Remote remote-account data.
  Tell us which Google account you used for remote pairing so we can locate the
  request. We may need to verify that the account is yours before acting. Do
  not send passwords, access tokens, device backups, presets, or unrelated
  personal information. Instructions are also at
  <https://freevia.org/qcremote/account/delete/>; visiting that page does not
  delete anything. Unpairing a single device or unlinking an AI app is not the
  same as requesting deletion of all remote-account data.

  A remote-account deletion request concerns data held by the Freevia-operated
  relay for your paired Google identity, such as device pairings, linked AI app
  grants, recent activity and usage counts. Once those records are deleted, your
  computers, phones and AI apps can no longer reach the relay, and a device must
  be paired again to use it. Third parties may hold separate records under their
  own terms, including Cloudflare's sampled Workers Logs; those records are
  outside the relay's deletion controls. A deletion request does not delete your
  Google account, anything stored on your Quad Cortex, or data kept locally by
  QC Remote on your computer or phone. Uninstalling QC Remote does not
  automatically delete relay data.
- **On your own devices.** Delete the files described under "Data that stays on
  your computer or phone", or uninstall the app.

## Your controls

You choose whether to pair at all, and which Google account to pair with. Before
you sign in, the app shows you which account a device would be paired to. The
REMOTE panel lists your paired devices and the AI apps linked to your account.
You can unpair a device at any time, which revokes it at the relay and locally,
and you can unlink an AI app. A device already paired to one Google account
refuses to be re-paired to a different one without first being unpaired, so the
relay cannot be used to quietly move a rig between accounts.

For a remote preset or setlist deletion, a compatible AI client may ask you to
confirm before sending the request. QC Remote does not add a second approval
prompt on the paired host, and the relay does not independently verify human
approval.

## Security

- All connections to the relay use HTTPS or secure WebSockets; the apps accept
  only an HTTPS relay address, and the Android app does not allow cleartext
  traffic on its own relay connection. Requests forwarded for the Quad Cortex go
  where the unit asks, as described above.
- Device credentials are stored by the relay only as SHA-256 hashes and compared
  in constant time. Credentials are accepted for 90 days after pairing. Access
  tokens expire after one hour.
- Each Google account's data is kept in its own isolated storage object. A
  pairing link opens a confirmation page that names the device and where the
  request came from, and nothing is attached to an account until you press
  Continue and sign in.
- Pairing, sign-in and registration routes are rate limited per address.
  Pairing and sign-in steps use single-use, short-lived state tied to your
  browser by a cookie. Pages refuse to be framed.
- Error messages shown to you never include internal exception text, and
  credential-like strings are removed from the error text the relay logs.
- The apps enforce the access level you choose (read-only, performance, modify
  or full) for local and remote AI apps alike. Deleting a preset or setlist
  requires an explicit confirmation flag from the AI app, whose built-in
  instructions tell it to describe the exact target and get your answer first;
  the paired app does not ask a second time, but still checks the access level
  and the exact target.

No system is perfectly secure. Report vulnerabilities as described in
[`SECURITY.md`](SECURITY.md).

## Children

QC Remote is not directed at children, and remote pairing requires a Google
account, which itself has its own age requirements.

## Changes to this policy

If this policy changes, the updated version will be published in this
repository and at <https://freevia.org/qcremote/privacy> with a revised date.
Material changes affecting the remote relay will be reflected here before they
take effect. The policy's revision history is in this repository.

## Contact

- General privacy questions: **privacy@freevia.org**
- Security reports: **security@freevia.org** (see [`SECURITY.md`](SECURITY.md))
- Product issues, suggestions and feedback: see [`SUPPORT.md`](SUPPORT.md)
- Takedown requests: **takedown@freevia.org**

Do not post personal data, credentials, or security vulnerabilities in a public
GitHub issue.
