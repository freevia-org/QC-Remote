# Changelog

Notable changes across QC Remote app versions. A version entry records project
history and does not by itself mean that a build was publicly distributed. The
Windows and Android apps are
versioned independently (`apps/windows/package.json`,
`apps/android/package.json`); the library and service crates stay at 0.1.0
and are not published to a registry.

## Unreleased

### Windows

- **A Quad Cortex connected to QC Remote can now use this computer's internet
  connection for CorOS updates.** With only QC Remote attached (Cortex Control
  closed), the unit has no network path of its own while USB is connected, so its
  DOWNLOAD UPDATE button used to go unanswered, and the capture shows it asking
  again about once a minute (what it finally does was not observed). The unit sends its
  update request to its host in a plain message (type 61, `UpdaterForward`: the
  URL, the headers and the timeout it chose). The device broker now carries that
  request over the computer's internet connection and returns the reply in
  chunks, with its keepalive running on its own clock for the length of the
  transfer.
- **New Settings switch, "Internet for the Quad Cortex" (Edit > Settings…).** It
  is on by default, including for settings saved by an earlier version, and shows
  what the relay is doing (passing a request through now, the last result, a
  request declined for another domain, or how often the unit asked while the
  switch was off). Turning it off ends a transfer in progress; the broker then
  behaves as before (the frame is counted and dropped) and
  `usbDiagnostics.updaterRelay` counts that the unit asked. The switch is an
  app setting stored as `updaterRelayEnabled` in `device-broker-tray.json`; it
  is not a Quad Cortex action and no AI tool can change it. It applies to every
  shipped broker, including one started over stdio for a local MCP client (a
  developer build that is given no settings directory has no switch and runs on
  the defaults), and a settings file that cannot be read for a moment keeps the
  last known setting instead of switching the relay back on; with no earlier
  value, such as at start, the relay is on. If the broker's value and the
  stored switch differ, the row's line says so.
- The relay carries the unit's update channel only. It does not carry the unit's
  account, cloud-library, backup or log forwards (message types 29, 30, 31) or
  its cloud login (type 18), which the broker still neither decodes nor answers,
  and it adds no cryptography. The unit supplies the request, its headers and its
  own credentials, so its serial number and those credentials reach Neural DSP as
  they would over Wi-Fi. The broker opens the connection on the unit's behalf but
  makes no request of its own: it forwards the unit's headers as they are, and
  its HTTP library adds only what HTTP itself needs (`Host`) and a generic
  `Accept: */*` header when the unit sent no `Accept`; it stores none and never writes a header value or a query value to a
  log or to the status.
- Network hygiene: only HTTPS requests to `neuraldsp.com` and its subdomains
  (matched on a whole label, so `evilneuraldsp.com` is not one), at any path, and
  to any domain named in `updaterRelayExtraDomains` (empty by default). A request
  for any other domain is not carried: the unit gets an error reply, and the host
  name only (no path, no query) is written to the broker's error output and the
  device flight recorder (a local file; its audit lines carry the host name and
  the path as a shape, with query values removed and long or token-like segments
  replaced, short segments as they are). `usbDiagnostics.updaterRelay`, which AI apps can
  read, carries counts (`refusedDomainRequests`) and the outcome word of the last
  request, and no host, path, domain list or time. No address literals, user
  names or ports other than 443; names that resolve to a private, loopback, link-local or carrier-grade-NAT
  address are refused at connection time; at most three redirects, each target
  checked again against the same rules, and a redirect to another host carries
  only the byte-range header; one request at a time; a response size cap
  (`updaterRelayMaxDownloadBytes`, 512 MiB by default); GET only with no request
  body; no system proxy and no retries. See `services/device-broker/src/updater_relay`.
- Known limitations: the update relay was built from one captured request (the
  unit's firmware check). Until it has been run against a real update, the
  request method for the download types, the meaning the unit gives the reply's
  `error_code`, and which host serves the image are unverified assumptions. If the
  image is served from a domain outside `neuraldsp.com` (for example a CDN), the
  request is declined and its host is written to the broker's log until that
  domain is added to `updaterRelayExtraDomains`. The first run against a real
  update will probably need iteration. A computer that must use an HTTP proxy
  cannot use the relay. Host-to-device throughput is bounded by the USB link, so a
  large image takes many minutes; Wi-Fi on the unit remains the faster route.

### Android

- No change. The Android app's USB host does not answer the unit's update
  requests; whether it should is a separate decision.

## Released 10 October 2026 — Windows 0.1.182 / Android 0.3.36

Changes since Windows 0.1.181 / Android 0.3.35.

### Windows

- USB startup progress is reported more precisely. The device
  broker now looks for a Quad Cortex before it reports `connecting`. Before,
  a computer with no unit attached reported an opening phase on every retry
  even though there was nothing to open. Now `searching` means no matching unit
  was found (the broker stays in it while it retries), and `connecting` means a
  matching unit was found and its USB connection is being opened. The Windows
  app treats `connecting` as the first sign that a unit is present: its status
  text moves from "Looking for a Quad Cortex…" to "Quad Cortex found; opening
  its USB connection…", then "USB open; handshaking with the Quad Cortex…" and
  "Quad Cortex connected; synchronizing its live state…", and the
  synchronization progress bar starts at the first of these. The area where the
  preset name goes shows "Opening the Quad Cortex…", "Handshaking with the Quad
  Cortex…" or "Synchronizing the Quad Cortex…" while it connects.
- A reconnect or session-reset request made while the broker is searching, and
  while USB enumeration shows no matching unit (the selected one, or any unit
  when none is selected), now returns the searching state at once instead of
  waiting for the ready timeout. The search stays armed, so a unit plugged in
  afterwards is still picked up. The connection epoch, and the counters it
  resets, now start when a unit is found rather than on every retry.
- The broker publishes a phase update when only its detail text changes, not
  just when the phase or the connected or synchronized flag changes.
- Known limitation: the published domain contract
  (`contracts/public/qc-domain.v1.json`) still maps the broker phase
  `connecting` to `searching` for clients that normalize broker phases, and
  that mapping was not changed. The Windows app's own phase listener reads the
  raw phase, but the result of a reconnect or reset call is normalized, so one
  that arrives while the unit is being opened may still read as searching.
- Managed MCP routes are repaired at every start. Once per start of the app, on
  a background thread, QC Remote checks the `qc-remote` entry it manages in the
  configuration of ChatGPT Desktop/Codex, Google Antigravity, Bionic (or
  pre-rename LM Studio) and Jan. An entry counts as QC Remote's own when it
  carries the managed client identity in its `env` and its command's file name
  matches the QC Remote executable (compared without regard to case); the
  folder may differ, so an app that moved is repaired. Such an entry is now
  rewritten with the current definition whenever it differs from what this
  build would write, unless a newer build recorded it. That includes a changed
  executable path, an older recorded version, arguments or environment keys you
  added to that entry, and a disabled flag: a route you switched off in the
  host is switched back on. Earlier builds left an entry you had edited, and a
  route you had disabled, as they were. A command that names another program,
  or an entry without a valid managed identity, is still left alone, and so are
  your other MCP servers. A timestamped backup copy of the host's configuration
  file is made before the entry is rewritten.

### Android

- Version change only (0.3.36, version code 41). The Android app's code is
  otherwise unchanged since 0.3.35.

## Released 10 October 2026 — Windows 0.1.181 / Android 0.3.35

Changes since Windows 0.1.170 / Android 0.3.29.

### Both apps

- Wire values are no longer given device meanings that have not been shown.
  What is written to the unit is unchanged, but documentation, tool descriptions and the Rust guide now separate what was
  observed, what is only believed and what is not established, and the guide's
  limits register names an experiment for some of the open items. Raw values are
  published beside derived ones: `get_general_settings` returns
  `screenBrightnessWire`, `ledBrightnessWire` and `dimmedLedBrightnessWire`
  beside the brightness steps, and `device.masterVolume` returns the raw wire
  float as `normalized` beside the percentage on the Windows host. The MIDI Out
  message type numbering is now stated once and as observed on a CorOS 4.1.0
  unit (2026-10-06): 1 Control Change, 2 Control Change Toggle, 3 Program
  Change, with the parameter roles the unit shows. The schema text and the app's
  MIDI Out editor had 1 as Program Change and 2 as Control Change; those labels
  were wrong and the editor now labels the numbers as observed and enables the
  values each type uses. The numbers written are untouched. Writing an
  undefined type was followed once by the unit restarting, so it is documented
  as a hazard and refused by the action schemas. The Looper X assignment editor in both apps used a
  name table that did not match the unit's own Looper X screen for the indices
  it read back; it now uses the table observed on a CorOS 4.1.0 unit on
  2026-10-06 (0 Record, 1 Play, 2 Undo, 3 Duplicate, 4 One Shot, 5 Half Speed,
  6 Punch In, 7 Reverse), so the labels shown for an unchanged arrangement
  differ from earlier releases. The same unit and date established that hold
  timing index n is 500 + 100 n ms, that MIDI channel 17 is OMNI (the settings
  screen now says OMNI, not ALL), that the brightness wire value is a 0 to 100
  percentage-style number the unit shows as round(raw * 32 / 100) steps, that a
  Global EQ band's fifth parameter is 1.0 when enabled, and that the preset-load
  MIDI Out list accepts the same three types as every other list. A stored preset's MIDI Out layout (source N in slots N * 12 to N * 12 + 11 of a 120-slot list, preset-load messages in a separate twelve-slot list) and a type 1 message keeping param3 were observed on 2026-10-07, as were the expression bypass modes (0 Stop, 1 Switch, 2 Heel-Toe), the mode numbers 0 Preset, 1 Scene, 2 Stomp (a unit whose cycle was Scene, Preset, Stomp), the Looper X free-samples unit (samples at 96 kHz) and ARMED state, the Looper X quantize and routing value names, that the show-tuner message does nothing, and that Gig View hides on a bare frame.
- The tuner writes (`set_tuner_input`, `set_tuner_mute`, `set_tuner_reference`,
  `set_tuner_meter`) and `set_device_name` now send their single frame and stop.
  The READ ladder that followed them is removed, because Cortex Control never
  reads a write back and a slow or silent reply used to fail as an ambiguous
  write and block every later change until a reconnect. They return
  `accepted_unverified`, not a verified result; read the tuner settings or the
  name yourself. Hardware verification of this path is pending.
- No write sends a type-15 READ (or any other read) to confirm itself any more.
  The verification refreshes at 0/250/1500 ms and on the preset-transition
  ladders, the settle reads after an expired window, the quarantine resync reads
  and the correlated READ ladders after Global EQ, mode cycle, tempo and
  metronome, favorite and pinned-model writes are all removed, as are the
  favorite and pinned-model read-preflights and their skip-if-already-matching
  suppression. Confirmation is now passive (the QC's own pushed state). When no
  confirming state arrives the result is `accepted_unverified`, not an error,
  and nothing is quarantined. The write quarantine remains only for genuine
  delivery ambiguity and clears on a new session or `reset_device_session`.
  `set_favorite` and `set_model_pinned` check `expected_favorite` /
  `expected_pinned` against the list the session already holds and fail closed
  (nothing sent) when it has not been read; call `list_favorites` first. The list
  is unconfirmed again as soon as a favorite or pin is sent, because the unit
  answers with the changed entry alone (measured on CorOS 4.1.0, 2026-10-07: one
  entry within about 10 to 30 ms), and that echo may not arrive; so the next
  `set_favorite` or `set_model_pinned` also needs a fresh `list_favorites` or
  `list_pinned_models` first, and a partial update never stands in for the list.
- `get_block_details` and `get_lane_control_details` are served from pushed
  state instead of forcing a type-15 READ per call.
- `get_global_tempo_settings` reads with the bare type-33 and type-15 READs
  (no request id) that Cortex Control sends at boot; every host type-15 READ is
  now that bare frame.
- File library writes (`save_preset_as`, `rename_current_preset`, `copy_preset`,
  setlists, moves, deletes) verify passively first, then send at most three
  directory reads after a 3 s passive window; with no confirming listing the
  result is `accepted_unverified`. `copy_preset` accepts the unit's `_1` name
  suffix; `rename_current_preset` refuses a name another slot already holds,
  because the unit does not de-duplicate a rename. Both frames are established
  on CorOS 4.1.0 (direct USB, 2026-10-07).
- `undo_device` and `redo_device` no longer read the preset after the frame. The
  unit pushes the changed preset within about 0.25 s of an undo (measured
  2026-10-07), which the app folds in; redo is not measured.
- Tool descriptions and the guides now state, for the multi-frame commands
  (port, Global EQ, tempo and metronome, block footswitch, expression bypass,
  show tuner), how many frames are sent and in what order, that a failure
  between frames can leave a partial state, and which parts are host policy.
  `move_block` documents that it sends the bare frame without the desktop
  application's field-3 sidecar. The preset-directory refresh refusal says an
  explicit refresh is not implemented (the unit does answer a directory read).
  The limits register gained a section on host policy and unverified behaviour.
- `set_global_bypass` takes `cab` and `ir` independently optional (at least one
  required) and sends one complete type-9 frame per supplied family, Cab then IR
  (gateway API 31). An API 30 host still requires both arrays, so send both to
  an older host. Hardware verification of a single-family frame is pending.
- Removed `restore_tuner_audio` (gateway API 30). It sent exactly the frame
  `set_tuner_mute` sends with `muted=false`; use that instead.
- `rename_current_preset` now sends the single File UPDATE frame Cortex Control
  sends for a rename, and `copy_preset` the single File COPY frame it sends for
  a paste, instead of re-saving the live Grid or recalling, saving and
  restoring. `copy_preset` no longer takes `expected_preset_name`,
  `expected_position` or `expected_preset_identity` (gateway API 29), no longer
  changes the active preset and no longer refuses while that preset has unsaved
  changes. `rename_current_preset` still refuses while the preset has unsaved
  changes (host policy). Both then verify by reading the library back.
  Hardware verification of these two direct frames is pending: no run exists
  since the apps began stamping the request id on file mutations. If one
  reports a failure, read the library back instead of retrying.
- Plugin models can be added. QC Remote no longer runs its own plugin
  availability check before placing a model, and does not establish plugin
  ownership; inspect the device response. In the tested unowned model, the
  block appeared locked and bypassed. `add_block` reports a placed plugin
  block's read-back bypass state in `pluginBlockReadback`.
- On the Grid screen, swiping up from the bottom edge opens Gig View and
  swiping down from the top edge opens I/O. A tap in those edge areas still
  reaches the Grid control underneath.
- The unit's Directory, Settings, Tempo, live capture and CPU screens, the block
  editors, lane panels and route pickers follow the unit's own screens more
  closely. The Directory has a category dropdown, library browsers, numbered
  folders and a Sort By control, and its Recent, Favorites and capture rows were
  refined; Settings opens on Connection.
- Block editors show only the parameters the device reports, in the device's
  value formats.
- Looper X workflows are complete, including editing the A–H action
  assignments.
- A block's parameters can be reset to their stored defaults.
- Changes made on the unit itself, such as output routes, Global EQ preset
  recall and other device-initiated state, are now followed in the app.
- Bulk-operation completion remains observable while controls are blocked, and
  a replacement state stream clears stale in-flight status so the app can leave
  SYNC after the device is ready again.
- The Directory re-reads the open setlist, so library changes made from another
  route appear without reopening it.
- Master volume and other continuous controls respond in real time, and an
  echoed volume value no longer fights a drag in progress. The Tempo screen
  dials no longer clamp the offset instead of the value.
- The Grid has a "Default" scene label, a tap outside an open dropdown closes
  it, and holding a block enters block move mode.
- The USB indicator uses the same colours in both apps, and a disconnected
  device is shown as inactive rather than as an error.
- Demo-device controls stay local and are never sent to a connected unit.
- A full-control `drag_screen` action (gateway API 28) drags between two screen
  pixels. It was first built as a bounded, paced stream of touch-move frames;
  measured on hardware (CorOS 4.1.0, 2026-10-06) that stream did not scroll
  and move frames had no visible effect in any variant tried (purpose
  unknown), so it now sends exactly what `swipe_screen` sends: ONE DRAG frame,
  the unit's own pointer drag. `steps` and `hold_ms` are still accepted and
  bounds-checked so existing callers keep working, but they are ignored because
  a DRAG frame carries neither. The Android host and the direct Python transport
  now run it too, since a single DRAG frame begins no held touch. Hosts at
  gateway API 27 or older reject it as an unknown method.
- `swipe_screen` now sends the protocol's built-in swipe, one DRAG frame,
  instead of three frames. Measured on CorOS 4.1.0 (2026-10-06) it scrolls
  lists reliably and leaves no row highlighted or activated. The distance
  scrolled depends only on how far the finger moves: 8 px or less does
  nothing, 10 px scrolls about 60 px of content, 20 px about 170, 30 px about
  255, and 50 px or more to the end of the list. On the Tempo dialog's knob the
  same frame is a linear drag of one BPM per pixel (a swipe of 50 px upward took
  the preset tempo from 120 to 170). Verified only on the preset menu popup, one
  Settings pane and that knob.
- `tap_screen` now sends the protocol's built-in tap, one TAP frame. Measured
  on CorOS 4.1.0 (2026-10-06) it taps reliably and leaves no row looking
  pressed. `long_tap_screen` sends a press, a wait and a release, because the
  protocol has no long-press frame. The same frames sent by hand showed a
  context-dependent long press on CorOS 4.1.0 (2026-10-06): a press held at
  least about 0.6 s detaches a Grid block (0.5 s did not), about 1.5 s broke a
  Modes hybrid, and on a parameter control about 0.9 s then release assigns the
  value to the current scene. The tool's own 1.2 s is a host choice, above the Grid
  and parameter values but below the 1.5 s tried on a hybrid, and its own gesture
  has not been run on hardware. If the release
  cannot be written, every host sends one more at the same point, and the
  Windows host refuses further gestures with `TOUCH_HELD` if that fails too.
- A new full-control `mouse_frame` action sends ONE raw mouse frame of the
  protocol (press, release, move, tap or drag; gateway API 29). `to_x` and
  `to_y` are for drag only. A press leaves the pointer down, so the Windows
  host refuses every other gesture with `TOUCH_HELD` until a release (or a raw
  tap) arrives. The host never sends a release of its own: an earlier build
  released a held press after 5 s, but that frame was unrequested and its delay
  was never measured, and a release is itself a click (it can activate what is
  under it, assign a parameter to a scene or drop a lifted item onto the delete
  bar). The Android host and the direct Python transport withhold it with a
  typed refusal, and hosts at gateway API 28 or older reject it as an unknown
  method.
- The local MCP server returns `capture_screen` and `get_preset_screenshot`
  PNGs as a standard image content block as well, next to the unchanged
  `pngBase64`, `width` and `height` fields. Only the PNG signature is
  checked before the block is added. The cloud connector still returns text
  only.
- The descriptions of the model preset and model catalog actions now say what
  the host establishes. `load_global_eq_model_preset` takes an identity from
  `modelPresetCatalog.entries` in `get_current_preset`: the host's account of
  the model presets the unit announced in this session, which it never treats
  as complete. A recall is refused with `STATE_UNAVAILABLE` when no list was
  announced or the list became unusable (a possible loss, a malformed entry,
  more than 256 Global EQ entries or 20000 in all), and, while the list is
  usable, with a plain error when the identity was not announced, was deleted
  since or is ambiguous; nothing is read back after it. `add_block`, `replace_block` and pinning (not unpinning)
  a model need the session's model catalog to list the model and not hide it;
  `list_models` lists that catalog and does not show plugin ownership.
  `create_model_preset` requests creation of a model preset from the block at
  `row` and `column`: it binds `model_hash` to that block and requires
  `expected_preset_identity`, and `expected_preset_name` is now optional in the
  action schema, as the gateway contract already had it. The host reads
  nothing back, so whether the unit created the preset, what it holds and
  whether it appears in the Global EQ preset list are not verified.

### Windows

- The device broker, its tray icon and the background MCP host now run as one
  signed executable, `qc-device-broker.exe` (since 0.1.178; 0.1.176 and 0.1.177
  shipped a separate `qc-remote-tray.exe`). After an idle release, it reopens
  the requested device and keeps that request pending if a disconnect is
  refused.
- The tray's release-and-exit menu item is labelled "Quit". After Quit, the
  broker, tray icon and background MCP host stay closed until a user action,
  such as opening QC Remote, starts them again.
- The tray lists active local clients and linked remote apps, and can Unlink
  either: local Unlink pauses an app's listing until its next tool call, while
  remote Unlink asks for confirmation and revokes only that app's authorization.
  The remote app must sign in again; other linked apps and this computer's
  pairing stay active. Remote sessions are grouped under their app when the
  matching QC Remote Cloud update is available. "Release QC after last client:
  …" shows and sets how long after the last client leaves the unit is released.
- Opening the unit now waits while another process holds it, names that
  process in the status line, and connects by itself once it lets go.
- Connecting is more robust: the startup read is bounded and control requests
  are answered while the connection is still being established.
- Writes the unit does not confirm are classified after the fact. The write
  pause is limited to the affected area and can clear itself, instead of
  always requiring a session reset.
- Device backup names are validated before the backup starts.
- The Windows installer declares Freevia as its publisher and verifies the
  signing identity of every shipped executable, including the installer and
  uninstaller.
- The tray shows a disconnected unit correctly.

### Android

- **I/O** and **GIG** buttons beside the master volume restore navigation to
  those screens, and the top and bottom edge areas for the swipe gestures are
  larger for touch.
- Opening Gig View while on the demo device no longer sends anything to a
  connected Quad Cortex.

### MCP and relay

- Remote MCP presence frames name each session's OAuth grant (`grantId`), so a
  paired host can revoke exactly that authorization. Hosts that predate the
  field ignore it. Unlinking an app withdraws its sessions from presence at
  once, and a revoke the relay could not store is reported as a failure rather
  than "not found".
- The device diagnostics action now states that the current Quad Cortex does
  not answer it; calling it times out without returning data.
- Broker connection accounting now includes a client before the authentication
  success reply is sent, so concurrent disconnects cannot treat that client as
  unauthenticated.
- Startup replay is bounded by both message count and bytes; the broker records
  omitted replay messages while preserving bounded access to the latest state.
- Deleting a preset or a setlist over the remote route is confirmed with you
  in the AI conversation. The paired app no longer shows a second prompt.
- New actions: replace a block and reset a block's parameters (Modify level);
  request creation of a named device model preset from a block, recall a
  Global EQ model preset, and set the Looper X stomp assignments (Full level).
- Some AI clients can only send the literal text "null" for an optional
  argument that accepts null (for example the expected model of Add Block). The
  MCP server now treats it as null.

### Library

- `quadcortex` state decoding follows the wire more closely, and says where it
  does not. New fields: `rawMode`, `sceneColorsArgb` and `rawColor`,
  `sceneLabelsRaw`, `undoFrames` and `redoFrames`, a block's `bypassSceneMode`,
  `bypassAddress` and `catalogCategory`, a route's `inputWire` and `outputWire`,
  and a MIDI Out source's `presetFlatOffset`. A mode value
  outside the nine the library names reads `UNKNOWN` instead of `PRESET`, with
  no per-row modes. A route id the row did not carry is absent instead of `0`.
  A route id the label table does not name reads as its wire enum name, and
  output 18 and input 0 keep their previous display strings. A bypass record
  that names its own row and column is addressed by them. A MIDI settings push
  whose group has no source is the preset-load list instead of being dropped. A
  sparse global tempo push reads an absent parameter index as 0, as the desktop
  application does. A scene label push shows the same default text a full preset
  does. The snapshot lists in `unobserved` the placeholders no device message
  has set yet (`mode`, `tempo`, `tempoLedEnabled`, `masterVolume`, `dirty`,
  `presetLocation`). The optimistic tempo write no longer switches the tempo LED
  on. Mode names, the preset MIDI source numbering and the undo and redo flags are
  readings, not device facts; the limits register says which.
- `quadcortex`: a guide and tutorial for Rust developers, built into the API
  documentation as the `guide` module (pages under
  `packages/rust/quadcortex/docs/guide/`). Every Rust snippet is a documentation
  test: most run offline against synthetic bytes, and the few that need a device
  are compile-only. Three more offline examples (`command_tour`, `state_watcher`,
  `startup_walkthrough`) accompany it, and the package `include` list now ships
  `docs/**`. Documentation only: the module exists only under `cfg(doc)`, so the
  compiled library and its seven stable domains are unchanged.
- `quadcortex`: preset-copy verification accepts a greater canonical numeric
  suffix on the same name stem (for example, `Preset_1` to `Preset_2`) and
  reconciles the saved catalog name only at the guarded destination. The exact
  suffix sequence remains firmware-owned.
- `quadcortex`: public API documentation, three offline examples
  (`cargo run -p quadcortex --example frame_round_trip`, `replay_trace`,
  `state_decoder`), and a README covering status, stability and licensing. No
  behaviour or signature changes.
- `quadcortex`: package metadata made registry-ready (still `publish = false`):
  the license texts ship inside the crate, `rust-version = "1.87"` (verified for
  library, examples and tests), homepage and documentation links. The only
  metadata-related code change is two test helpers that used `slice::as_chunks`
  (Rust 1.88) and now use `chunks_exact`.

## Initial version baseline — Windows 0.1.170 / Android 0.3.29 — 2026-09-29

- The source baseline uses `MIT OR Apache-2.0`, with the device
  reference material, brand assets, and third-party components carved out as
  described in [`LICENSE`](LICENSE).
- Windows desktop app (Tauri) with the embedded MCP server, the native device
  broker, and local MCP pairing for desktop AI apps.
- Android companion app (Capacitor) with USB host control through the shared
  native library.
- The shared Rust `quadcortex` library, the relay client and protocol crates,
  and the Cloudflare Worker relay (`services/qc-remote-cloud`) behind
  `qcrc.freevia.org`.
