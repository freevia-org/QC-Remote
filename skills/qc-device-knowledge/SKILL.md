---
name: qc-device-knowledge
description: Ground answers about the Neural DSP Quad Cortex -- its Grid/routing model, USB audio behaviour, MIDI implementation, Scenes/Presets/Setlists, footswitch Modes, and which real amps/pedals/plugins its 633 models and device list entries are based on -- in Neural DSP's own published manual, device list, plugin pages, CorOS release notes, and support articles. Use when identifying what hardware a Quad Cortex model or capture is modelled on, explaining Grid routing or a footswitch Mode, answering a MIDI CC or USB-channel question, or checking which plugin unlocks a device. Pairs with QC Remote's own MCP tools for live device control; read qc://assistant-guide (or this skill's own host) for how to operate those tools safely -- this skill is reference knowledge, not an operating manual for the tools themselves.
metadata:
  version: "1.0.0"
---

# Quad Cortex device and plugin knowledge

This skill answers "what is this" and "what does the Quad Cortex do" -- not
"how do I safely call this MCP tool." For tool-safety, unsaved-work, and
confirmation-flag rules when actually operating a connected device through
QC Remote, use the `qc://assistant-guide` resource instead; the two are
complementary and do not repeat each other.

**Every claim below carries a source tag. Do not blend them or drop the
tag when repeating a claim:**

- **MANUAL** -- stated in the official Neural DSP Quad Cortex User Manual
  (version 4.1.0, `https://neuraldsp.com/manual/quad-cortex`). Citations give
  a chapter/section reference, e.g. `[MANUAL 3.2]`.
- **DEVICE_LIST** -- stated on `https://neuraldsp.com/device-list`, Neural
  DSP's own catalogue page of every device and which plugin (if any) it
  requires.
- **PLUGIN_PAGE** -- stated on one of Neural DSP's plugin marketing pages
  (`https://neuraldsp.com/plugins/<slug>`).
- **COROS_RELEASE_NOTE** -- stated in one of Neural DSP's own CorOS/Cortex
  Control version-release posts (`https://neuraldsp.com/quad-cortex-updates/<slug>`
  or `https://neuraldsp.com/news/<slug>`).
- **PLUGIN_MANUAL** / **PLUGIN_MANUAL_PDF** -- a per-plugin manual, either
  web-published (`https://neuraldsp.com/manual/<plugin-slug>`, 4 of 12
  Quad-Cortex-relevant plugins) or PDF-only (17 more plugin manuals,
  downloaded from Neural DSP's own support article). These describe a
  plugin's own knobs and DAW MIDI Learn setup, never Quad Cortex hardware
  behaviour, and are never a source for "based on" hardware claims.
- **SUPPORT_ARTICLE** -- a Neural DSP support-centre article
  (`https://support.neuraldsp.com/help/<slug>`).
- **QC_REMOTE** -- this app's own documented tool/route surface (what a
  QC Remote MCP tool's schema actually declares), not a Neural DSP
  publication. Cited when it corroborates or narrows a MANUAL claim.

If a claim doesn't carry one of these tags, treat it as this skill's own
summary of the tagged sources around it, not an independent fact.

## How to use this skill

- `references/data/qc-model-lineage.json` is a bundled snapshot of this
  repository's device/plugin catalogue: 633 Quad Cortex models, joined
  against the manual's device tables, the device-list page, all twelve
  plugin pages, and 35 CorOS release notes. Each entry carries
  `device_name`, `device_category`, `based_on` (the real-world hardware
  name, when one is stated -- `null` means no source states one, not
  "probably similar to X"), `required_plugin`,
  `plugin_url`, `added_in_coros`, `previous_name`/`replaces` (for devices
  renamed or superseded across CorOS versions). The bundle contains the
  structured catalogue and concise source attribution, not copied manuals,
  pages, or release posts.
- Where QC Remote's MCP server is available, prefer the `search_qc_device_knowledge`
  tool over reading the raw JSON: it ranks matches by device name, based-on
  hardware, category, and description, and needs no connected Quad Cortex.

## Model lineage: what an amp/pedal is modelled on

Of the catalogue's 633 models, 30 are internal/routing blocks (splitters,
ZenLink, FX Loop send/return points, ...) with no source hardware at all.
Of the remaining 603: **481 matched** a manual/device-list/plugin-page/
release-note entry (79.8%) -- **307 with a stated hardware basis**
(e.g. catalogue `"Brit 2203"` -> manual `based_on: "Marshall® JCM800®"`),
174 matched entries have no stated hardware basis (the Quad Cortex's own
original algorithm, a plugin-original design, or a public listing that does
not name underlying hardware). **122 are
honestly unmatched** -- no published Neural DSP source states what, if
anything, they are modelled on. [DEVICE_LIST, MANUAL, PLUGIN_PAGE,
COROS_RELEASE_NOTE]

`(M)` and `(ST)` on a Delay or Modulation model name a genuine device-level
distinction, not the same block twice under different names: Delay and
Modulation devices are documented as "available in mono and stereo."
[MANUAL 3.2] A `(ST)` variant consistently exposes extra parameters its
`(M)` sibling lacks (`PING PONG`, `WIDTH`, `STEREO WIDTH` on Delay models;
`WIDTH` on at least one Modulation model), so picking between them is a real
functional choice, not a naming preference.

## Additional Neural DSP documentation

- **Per-plugin manuals** exist separately from the marketing pages.
  `https://neuraldsp.com/manual/<plugin-slug>` (PLUGIN_MANUAL) exists for 4
  of the 12 Quad-Cortex-relevant plugins (Archetype John Mayer X, Misha
  Mansoor X, Rabea X, Tim Henson X); the other 8, plus 11 more plugins
  entirely, have only a PDF manual or a marketing page. A per-plugin manual
  never states a real-world hardware brand (e.g. "A 40w US combo amplifier
  from 1964 featuring a 15” speaker", never "Fender"), so it never sets
  `based_on`.
- **Expression pedal compatibility is a tested list, not just a spec sheet.**
  [SUPPORT_ARTICLE-adjacent QC blog post] Neural DSP tested Mission
  Engineering SP-1, M-Audio EX-P, Roland EV-5, HoTone Soul Press,
  Electro-Harmonix Expression Pedal, Boss FV-500-L, Dunlop DVP 3/4, and DOD
  FX-17 "with very good results." An untested pedal still works if it has a
  TRS output (not Wah/Volume-wired), a linear or logarithmic taper, a
  minimum-value knob, and a polarity/reverse switch. A toe-switch pedal
  toggles the assigned parameter between 0 and 1.00 on engage; connect only
  the expression jack, not a separate toe-switch output.
- **The support FAQ states a QC-vs-plugin tone-matching recipe.**
  [SUPPORT_ARTICLE, `quad-cortex-faq`] To compare a Quad Cortex preset
  against the same tone in the plugin: connect the instrument to QC IN 1/2
  (or the interface's Hi-Z input), match input headroom by setting either
  (a) plugin INPUT to 0.0 dB and QC Row 1 INPUT GAIN to -2.26 dB, or (b)
  plugin INPUT to +2.3 dB and QC IN1 to 0.0 dB, then monitor the plugin
  through the QC as the audio interface so impedance and monitoring stay
  identical. The Doubler block randomly delays a duplicated signal
  per-note/chord by an amount set by SPREAD; set SOURCE to an analog input,
  not "Off", for clearer transients.
- **CorOS release notes are the version-by-version changelog.**
  [COROS_RELEASE_NOTE] The device list's `added_in_coros`/`updated_in_coros`
  fields say which version changed a device; the release note for that
  version says what changed and often why -- e.g. CorOS 3.1.0 explains that
  "US DLX 65 Reissue" replaced both "US DLX Normal" and "US DLX Vibrato" via
  a new MODE parameter.

## The Grid, signal routing, and blocks

- The Grid is **four rows of eight device-block slots each**. [MANUAL 3.0]
- **Input/Output blocks are position-fixed**, not free-floating, so routing
  stays consistent across Presets. Input blocks represent physical inputs
  (instrument, mic, returns, USB); output blocks represent physical outputs
  (main outs, sends, USB, or routing to another Row). [MANUAL 3.2]
- **Splitter/Mixer blocks** create parallel paths: a Splitter on Row 1 or 3
  sends its "B" path to Row 2 or 4 respectively; a Mixer merges a Row 2/4
  path back into Row 1/3 or out to an output block. Splitter types are
  Balance / A-B / Crossover. [MANUAL 3.2]
- **Side-chaining** is limited to blocks in the Compressor, Filter, and
  Utility categories tagged `(S/C)`; the side-chain source can be any input
  or earlier block on the Grid. [MANUAL 3.2]
- Device-block categories: Plugins, Amp, Neural Capture, Cab, Overdrive,
  Delay, Reverb, Compressor, Pitch, Modulation, Morph, Synth, Filter, EQ,
  IR Loader, Wah, FX Loop, Looper, Utility. [MANUAL 3.2]
- **A block goes into an empty cell only.** [MANUAL 3.0] "To begin building
  an audio signal chain, tap an empty slot on The Grid. This will open the
  Virtual Device List" -- the documented workflow only ever starts from an
  empty slot; tapping an occupied block opens its parameter editor instead
  [MANUAL 3.3]. QC Remote's own `add_block` tool enforces this: it targets
  an empty Grid cell and refuses a cell that already holds a model,
  regardless of arguments passed. [QC_REMOTE]
- **QC Remote does not check plugin ownership.** `add_block` does not
  check whether a plugin model is owned. For a placed plugin block the
  result carries `pluginBlockReadback` (the read-back `bypassed` state and
  whether bypass was requested). QC Remote cannot read ownership, so a
  bypassed plugin block is a fact to report, not proof the plugin is
  unlicensed. [QC_REMOTE]
- **Plugin models can be added.** Plugin ownership is not established by
  QC Remote; inspect the device response. In the tested unowned model, the
  block appeared locked and bypassed. Prefer models the user says they own.
  Once, on one model, adding and then removing an unowned plugin was
  followed by a device DSP error that needed a reboot; it has not been
  reproduced since. [QC_REMOTE]

## USB audio interface

- The Quad Cortex is a **fixed USB 2.0, 24-bit, 48 kHz** audio interface --
  not switchable to another sample rate. [MANUAL 8.0]
- **16 USB audio channels total: 8 inputs + 8 outputs**, from the host's
  point of view. [MANUAL 8.1]
- The 8 USB inputs (host recording) are fixed-purpose, not generic taps:
  - **USB Input 1** = dry D.I. from analog Input 1. **USB Input 2** = dry
    D.I. from analog Input 2.
  - **USB Input 3** = processed signal from analog Output 1/L. **USB Input
    4** = processed signal from analog Output 2/R.
  - **USB Inputs 5-8** = whatever a Grid output block assigned to "USB
    Output 5/6/7/8" sends -- the only USB-input channels a preset's own
    routing controls.
  [MANUAL 8.1]
- The 8 USB outputs (host playback) mirror that split: **USB Outputs 1-4**
  feed analog Outputs 1/L-4/R directly from the host; **USB Outputs 5-8**
  feed whatever Grid input block is set to "USB Input 5/6/7/8." [MANUAL 8.1]
- **Manual's own re-amp recipe**: "To reamp a recorded D.I. track, set the
  track's output as Output 5 and the Grid input block to USB Input 5 during
  DAW playback. Then, arm a stereo track (Inputs 3/4) for recording."
  [MANUAL 8.2]
- **USB LEVEL** (I/O Settings > USB Audio & MIDI Ports) adjusts USB audio
  volume per port -- a real, per-channel gain-staging control separate from
  the analog Master Volume, which can itself be assigned globally or to a
  specific output. [MANUAL 2.13, 2.3, 9.1] The device treats clipping as a
  first-class, visible condition: "The input and output blocks will turn
  red if their assigned I/Os are clipping." [MANUAL 3.2]
- QC Remote's own chain-input/output route tables narrow this further: a
  Grid **input** block can only be assigned `In 1`, `In 2`, `In 1/2`,
  `Return 1`, `Return 2`, `Return 1/2`, `USB 5`, `USB 6`, `USB 5/6`, `USB 7`,
  `USB 8`, `USB 7/8`, `Prev. Row`, or `Not In Use` -- **no USB 1-4 input
  route exists**, consistent with the manual's USB-channel table above
  (channels 1-4 are outbound-only DI/wet-monitor taps from the Grid's
  perspective, never a valid chain-input source). A Grid **output** block
  has a wider set: `Multi Out`, `Out 1/2`, `Out 3/4`, `Send 1/2`, `USB 3/4`,
  `USB 5/6`, `USB 7/8`, individual `Out 1-4`, `Send 1-2`, `USB 3-8`, `Row 3`,
  `Row 4`, `Row 3/4`, `Internal` -- `USB 3/4` and `USB 3`/`USB 4`
  individually **are** valid chain outputs even though they are not valid
  chain inputs. [QC_REMOTE]

## Quad Cortex MIDI

- **The Quad Cortex has a full MIDI chapter with a genuine, complete,
  published CC chart.** [MANUAL 7.0-7.3] Section 3, "Incoming MIDI CC List,"
  tabulates CC#0 through CC#62 by name; CorOS 2.3.1 and 3.0.1 each record
  fixing "an issue where some of the information in the Incoming MIDI
  Reserved CC list was incorrect," confirming this is a maintained,
  authoritative document, not an incidental mention.
- **MIDI travels over USB-B and MIDI DIN, both directions.** [MANUAL
  7.preface, 7.0] "MIDI OVER USB" is a separate on/off setting from the
  physical DIN jacks; MIDI Clock in and out each independently require it.
  MIDI Clock In via DIN shipped in CorOS 1.3.0; outgoing MIDI Clock (USB,
  DIN, or both) shipped in CorOS 2.3.0.
- **The CC table, load-bearing entries:** Preset recall is Bank Select
  (CC#0 = MSB, CC#32 = LSB) then Program Change -- not Program Change alone
  [MANUAL 7.2, 7.3; SUPPORT_ARTICLE `quad-cortex-midi-pc-calculator`].
  Expression Pedals 1/2 = CC#1/CC#2. Footswitches A-H = CC#35-42. Scenes
  A-H = CC#43. Tap Tempo = CC#44. Tuner = CC#45. Gig View = CC#46 (confirmed
  by a CorOS 1.1.0 bugfix note referencing it by number). Modes = CC#47.
  Looper X = CC#48-61. Ignore Duplicate PC = CC#62.
- **CC value semantics changed once, in CorOS 2.1.0.** Before it, CC#45/46/48
  were each a single 0-127 toggle; 2.1.0 split them into a 0-63/64-127 range
  (e.g. "Value 0-63 Gig View Off" / "Value 64-127 Gig View On"), which is
  what the current manual documents.
- **Preset MIDI Out sends, it doesn't just receive.** [MANUAL 7.1] Up to 12
  MIDI messages (CC, CC Toggle, or PC) can be assigned per footswitch (A-H)
  or expression pedal, sent on press/move; a separate up-to-12-message list
  fires on Preset load. Footswitch MIDI Out only fires in SCENE/STOMP/HYBRID
  Mode; expression-pedal MIDI Out always fires.
- **Per-block MIDI note/clock control is real, not just prose.** Minivoicer
  and Multivoicer (Pitch category) carry `MIDI`/`MIDI CH` parameters;
  Overlord Synth (Synth category) carries `MIDI CH.`, `MIDI NOTE`,
  `MIDI VELOCITY`, `MIDI TRIGGER`; Looper X carries `MIDI CLOCK START`
  (confirming CC#59); the internal I/O Settings group carries `MIDI Thru`
  (confirming the global MIDI THRU setting is a real device-side value).
  This corroborates release-note claims like CorOS 3.3.0's "Mono Synth ...
  featuring enhanced functionality such as MIDI note control" and CorOS
  4.1.0's "MIDI-controlled harmonization."
- **Do not confuse this with plugin MIDI.** Per-plugin manuals and
  PLUGIN_MANUAL_PDF documents describe DAW-side "MIDI Learn" -- mapping an
  external controller's notes/CC onto a plugin's own on-screen parameter,
  inside a DAW. That is a completely different subject from Quad Cortex
  hardware MIDI and none of it belongs in an answer about the device.

## Scenes, Presets, and Setlists

- **Scenes** let one Preset store independent values for parameters
  explicitly assigned to Scenes (tap-and-hold a parameter to assign or
  unassign it). A new Preset starts on Scene A by default; saving while a
  different Scene is active makes that the new default. [MANUAL 2.9]
- **Presets** are complete Grid configurations, organized into **Setlists**
  (up to 256 Presets each). Unsaved changes show the Preset name in italics;
  "Save" overwrites, "Save As..." branches a copy, including from Factory
  Presets. [MANUAL 4.2, 4.3]
- Neural Captures and IRs live in a separate Directory structure (up to 2048
  each, 30 folders, nestable 4 levels deep) -- distinct from the
  Preset/Setlist hierarchy. [MANUAL 4.3]

## Tempo & sync

- Tempo mode is either **GLOBAL** (one BPM shared by all Presets) or
  **PRESET** (each Preset stores its own Tempo/Metronome settings).
  [MANUAL 2.6]
- Closing the Tempo menu or changing Presets does **not** stop metronome
  playback (may cause a brief audio dropout on Preset change); it stays in
  sync with the current tempo. [MANUAL 2.6]
- Looper X's DUPLICATE can be tempo-synced (locks loop length to the tempo
  grid) or free-running. [MANUAL 3.6]

## Gig / Preset / Scene / Stomp view model

- **Modes** (PRESET, SCENE, STOMP, plus user-defined **HYBRID**) determine
  what a footswitch press does; the active Mode shows top-right on the
  Grid. Cycle Modes with BANK DOWN + TEMPO. [MANUAL 2.7]
  - **PRESET Mode**: each footswitch (A-H) loads a Preset; Presets are
    organized into Banks of 8. [MANUAL 2.8]
  - **SCENE Mode**: each footswitch loads a Scene; BANK UP/DOWN instead
    navigates Presets. [MANUAL 2.9]
  - **STOMP Mode**: each footswitch toggles the bypass of its assigned
    device block(s); one footswitch can drive multiple blocks. BANK UP/DOWN
    navigates Presets. [MANUAL 2.10]
  - **HYBRID Mode**: assigns a different Mode to each footswitch row (A-D
    vs. E-H) within one Preset; merging two Modes in the Modes
    Configuration menu creates it. When HYBRID pairs with PRESET, Preset
    Banks split into two 4-footswitch halves. [MANUAL 2.11]
- **Gig View** is a dedicated full-screen readout of the current footswitch
  configuration (swipe up from the Grid, or hold BANK DOWN + TEMPO). Its
  layout adapts per active Mode, exposing Mode-specific extras (Edit
  Scene/Swap Scene/Copy Scene in SCENE Mode; Edit Stomp in STOMP Mode).
  [MANUAL 2.12]

## Scope note

This skill is Quad Cortex device/plugin/manual knowledge only. It does not
cover: guitar-tone identification or signal-chain matching (a separate
project, no longer part of QC Remote's shipped app or MCP surface); QC
Remote's own tool-safety,
confirmation-flag, or unsaved-work rules (`qc://assistant-guide`); or
third-party software internals. If a question
needs an answer this skill does not have a published source for, say so
rather than inferring one from adjacent facts.
