Frequently asked questions, and a register of what the library and this guide
say is **not established**. The register links the doc comments that say so. If a
point you care about is on it, treat the library's behaviour there as a policy
choice and not as a statement about the device.

## What is not established here

Each row names an item and separates what has been *observed*, what is only
*believed* and what is *not established*. The linked doc comments are the
authority and are updated as observations accumulate. Read the item's current
wording before you rely on a row.

**Legend.** The three labels mean different things, and a row can use all of
them.

- **Observed.** Seen in this project's own runs: captures of a unit's traffic,
  or exercises against the unit and firmware the doc comment names (CorOS 4.1.0
  where it gives a version). An observation holds for that unit and firmware
  only, and a count of occurrences is not a guarantee.
- **Believed.** A working reading that fits what was seen, offered so a reader
  is not left with nothing. It has **not been verified on the unit used for
  this project's tests**, so it is a hypothesis and not a fact. The doc comment
  uses the word *believed* at the same place. Do not build behaviour on it.
- **Not established.** No reading is claimed, or an open question remains
  whatever is believed. A dash means the register has nothing to record under
  that label.

A statement a doc comment makes with none of these words is stated plainly there
and is not repeated in the register unless a question remains open. A
*believed* entry that is later verified moves to *observed* in both the doc
comment and this register; it does not simply disappear from the register.

| Topic | Observed | Believed (not verified) | Not established | Where it is said |
| --- | --- | --- | --- | --- |
| Envelope device bytes | The device writes varying values in the two trailing bytes, often zero; the same pair turns up on unrelated message types and one type carries many different pairs. The doc records a tally from one passive capture. | Host software ignores both bytes. | What the two bytes mean. They do not appear to be a protocol field, but what they mean is open. | [`Frame::device_bytes`](crate::protocol::framing::Frame::device_bytes); tallied by [`inbound_telemetry`](crate::inbound_telemetry) |
| Diagnostics replies: delivery | Current firmware has not been observed to answer the diagnostics request (it times out), and no diagnostics message appeared in about nine minutes of passive traffic across four sessions. | - | Whether any firmware answers, and when a unit would push one unprompted. | [`DeviceDiagnostics`](crate::protocol::responses::DeviceDiagnostics) |
| Diagnostics replies: figures | No reply of this shape has been seen from a live unit, so none of the figures has been observed. | The CPU, peak CPU, heap and DMA figures are percentages, and the three heap figures are the data-memory, program-memory and external-memory heaps, as on the unit's own diagnostics screen. The USB connected value is a flag that is non-zero while a USB host is connected. | The meaning of the status words and the error flag bits, of the USB buffer read and write indices, and of the dropped-message and dropped-report counts; whether the figures use the same scale or order as the unit's screens. | [`DspDiagnostics`](crate::protocol::responses::DspDiagnostics), [`UsbDiagnostics`](crate::protocol::responses::UsbDiagnostics) |
| System time sync: value | - | `ms_since_epoch` is the host's current time in UTC, in milliseconds since the Unix epoch, with no time-zone offset. | Whether the unit reads the value in those units or against that epoch. | [`commands::sync_system_time`](crate::commands::sync_system_time) |
| System time sync: effect | The unit answers no read of its clock. Two attempts to observe an effect both failed: IR dates came from the host, and a preset saved after setting the clock to 2020 was still stamped with the true time, with and without a request id. | - | What the unit does with the value. It is sent for handshake fidelity. | [`commands::sync_system_time`](crate::commands::sync_system_time) |
| Direct rename and copy frames | The library sends the desktop application's single File UPDATE (rename) and single File COPY (copy) frame shapes, with the request id stamped when the frame is sent. The shapes come from a static audit of the desktop binary, not from a USB capture. | - | Whether the unit applies either frame as sent. No run of either exists since the library began stamping request ids on file mutations; the failed runs that exist are earlier. What a COPY does to the working preset. | [`commands::rename_preset`](crate::commands::rename_preset), [`commands::copy_preset`](crate::commands::copy_preset), [`plan_preset_mutation`](crate::device::request::plan_preset_mutation) |
| Tuner and device-name writes: no readback | The desktop application sends each tuner frame (input, mute, reference, meter) once and never reads it back; the library does the same and reports `accepted_unverified`. | A tuner write engages the tuner invisibly (an undated measurement, not a static finding). The device-name frame is derived from the wire schema; the desktop application has no sender for it. | Whether the unit applies each frame as sent, whether it pushes an update afterwards, and whether a device name survives a restart. Hardware verification of the no-readback path is pending. | [`commands::set_tuner_input`](crate::commands::set_tuner_input), [`commands::set_device_name`](crate::commands::set_device_name), [Sending commands safely](crate::guide::send_commands) |
| Undo and redo: state after the frame | The frames are the desktop application's single UndoRedo UPDATEs (`08 01 28 01`, `08 01 30 01`). | - | Whether the unit pushes preset state after an undo or redo. The library sends no read afterwards and does not refresh its cached preset. Hardware verification is pending. | [`commands::undo`](crate::commands::undo), [`commands::redo`](crate::commands::redo) |
| Preset recall without a request id | The frame is the desktop application's single SetlistPosition UPDATE (folder key, position, is_factory), with no request id, from a static audit of the desktop binary. Completion is observed as a newer position revision, so nothing depends on an echoed id; that observation names the address only, never the preset body. | - | Whether the unit applies the frame and pushes a newer position revision as expected. Hardware verification pending: a recall without a request id completes and verifies. Bank navigation and reload are host arithmetic over the same one frame (current position plus or minus 8 from the cached snapshot; the current address); that a re-recall discards unsaved edits is believed, not shown. | [`commands::setlist_position`](crate::commands::setlist_position) |
| Global bypass: single-family frame | The desktop application sends one complete type-9 family frame per interaction (Cab in outer tag 7, IR in tag 8), from a static audit of its binary. The library sends exactly one frame for a family supplied alone. | - | Whether the unit applies a single-family frame as sent. When both families are supplied the library sends two frames, Cab then IR, which are not atomic. Hardware verification pending: a single-family frame. | [`commands::set_global_bypass`](crate::commands::set_global_bypass) |
| Global tempo write in PRESET mode | The frame is the desktop application's one type-33 parameter 0 UPDATE, which has no mode precondition. The library no longer refuses the write outside GLOBAL mode. | - | Whether the unit applies a write to parameter 0 while parameter 1 is PRESET: no measurement in this project shows it ignoring the write. Hardware verification pending: a scratch test in PRESET mode, reading back type-33 index 0. | [`commands::set_global_tempo`](crate::commands::set_global_tempo) |
| Tempo and metronome fields: where they are stored | Each supplied field is one type-33 GlobalTempo UPDATE frame, in a fixed order (time signature, LED, volume, running, pan, subdivision, sound, routing, then beat cells). | - | Whether the unit stores these fields in the preset tempo block that a readback of the loaded preset reads; a failure between frames leaves a partial state. Hardware verification pending. | [`commands::set_tempo_parameters`](crate::commands::set_tempo_parameters) |
| Block replacement on an occupied cell | Replacing sends the same single Grid UPDATE frame as adding a block; only the host guard differs. | - | What the unit does with that frame on an occupied cell, and what the desktop application's occupied-cell drop controller sends (a static audit is pending). Hardware verification pending. | [`commands::set_block`](crate::commands::set_block) |
| Instrument values | `0` untagged, `1` guitar and `2` bass are stated plainly. Value `3` has not been seen in listings. | `3`, `4` and `5` are Synth, Vocal and Other. | Whether those three names are right, and whether `3` appears in listings at all. | [`PresetEntry::instrument`](crate::device::PresetEntry::instrument), [`LibraryEntry::instrument`](crate::protocol::responses::LibraryEntry::instrument), [`DeviceOperation::SavePreset`](crate::commands::DeviceOperation::SavePreset) |
| Expression bypass options | Observed on a CorOS 4.1.0 unit, 2026-10-07 (an assignment written to a scratch preset, the unit's screen read): mode `0` is Stop, `1` is Switch and `2` is Heel-Toe (the unit's SWITCH ON label); `invert` shows as INVERT RANGE On or Off with no inversion of the value; `delay_ms` is raw milliseconds (250, 1000, 2500 and 5000 displayed exactly, and `0` shows as OFF); `latch_emulation` shows as LATCH EMULATION On or Off. A new assignment shows Heel-Toe selected and 600 ms for the delay, and the screen greys out latch emulation under Heel-Toe (CorOS 4.1.0). | The behaviours: Heel-Toe bypasses at the heel end of the travel (the toe end when inverted), Switch bypasses while the toe switch is pressed, Stop bypasses when the pedal is held still for the delay; latch emulation is available only for Switch. | What the unit does with each behaviour. **Experiment that would settle it:** with a pedal connected, assign each mode to a block on a scratch preset and move the pedal through its travel, noting when the block bypasses. | [`DeviceOperation::SetExpressionBypass`](crate::commands::DeviceOperation::SetExpressionBypass) |
| Input-control bypass assignment | - | - | Why the assignment frame for the input control carries a fixed column value of `1` that its info frame omits. | [`DeviceOperation::SetExpressionBypass`](crate::commands::DeviceOperation::SetExpressionBypass) |
| MIDI Out sources and message types | Observed on a CorOS 4.1.0 unit, 2026-10-06 (raw frames written to a scratch preset, then the unit's own Preset MIDI Out screen read): type `0` is None (an empty row), `1` is Control Change (CC number `param1`, value `param2`; no third value shown), `2` is Control Change Toggle (CC number `param1`, minimum `param2`, maximum `param3`) and `3` is Program Change (bank CC#0 `param1`, bank CC#32 `param2`, program number `param3`). Sources `0` to `7` are footswitches A to H, `8` is EXP 1 and `9` is EXP 2, and the preset-load list is a separate group that accepts types `1`, `2` and `3` with the same labels, and an empty list clears it. A source holds up to 12 messages, shown as three pages of four rows in the order sent, and the unit preserves the position of each message as sent, so the library writes the rows in order with each empty row as a type `0` placeholder, reads a list back by position and drops only the empty rows after the last active one; an empty list clears the source, and replacing a longer list with a shorter one leaves only the shorter list. Observed on the same unit on 2026-10-07 (live read, then a save and recall of a scratch slot): a stored preset keeps the general messages in a 120-slot list where source `N` occupies slots `N * 12` to `N * 12 + 11`, a shorter legacy list mirrors only the first message of each source, the preset-load messages are stored in a separate twelve-slot list that preserves positions (a type `0` row between two active rows stays at its index, live and stored), and a type `1` message keeps `param3` in both the live and the stored bytes, which were identical. The writes took effect on screen at once and did not mark the preset modified. | - | Which jack an expression port id belongs to is separate (see Expression port ids). | [`DeviceOperation::SetMidiOut`](crate::commands::DeviceOperation::SetMidiOut), [`DeviceOperation::SetPresetLoadMidiOut`](crate::commands::DeviceOperation::SetPresetLoadMidiOut), [`commands::set_midi_out_selector`](crate::commands::set_midi_out_selector) |
| MIDI Out undefined type (hazard) | Once, on the same unit and date, writing an undefined type (type `4`, channel `5`, parameters `21`, `42`, `63`) to source `3` was followed within seconds by the USB device vanishing for about 25 seconds and re-enumerating with the unit restarted on its saved preset. | Undefined types are dangerous and must not be written. The crate accepts only types `1` to `3` through the operation, and the action schemas accept only `1` to `3`. | Whether the write caused the restart or something else did; one occurrence does not prove it. The low-level `set_midi_out_selector` does not check the type. **Experiment that would settle it:** none is recommended on a unit holding unsaved work; if repeated, do it only on a scratch preset with the host's state saved first. | [`commands::set_midi_out_selector`](crate::commands::set_midi_out_selector) |
| Device setting meanings | Observed on a CorOS 4.1.0 unit, 2026-10-06 (raw values written, the unit's Settings pages read): `midiChannel` `1`, `2` and `16` show as channels 1, 2 and 16 and `17` shows as OMNI; `holdTiming` `0`, `3` and `5` show as 500, 800 and 1000 ms, so index `n` is `500 + 100 * n` ms. Both are sent and read back as the raw wire number; `holdTimingMs` is derived by the library. | - | Indices `1`, `2` and `4` of `holdTiming` were not read, and the values between `2` and `16` of `midiChannel` were not read; the pattern is assumed. | [`commands::is_general_integer_value`](crate::commands::is_general_integer_value) |
| Tempo clock: bar counter | The bar counter advances once per bar at the active tempo (a bar lasted 2000 ms at 120 BPM in 4/4) and keeps counting across separate host sessions, so a new session does not restart it. Values in the thousands and as low as the tens have been seen. | The count starts when the unit starts. | Whether the first bar is `0` or `1`. | [`TempoClock::current_bar`](crate::protocol::responses::TempoClock::current_bar) |
| Tempo clock: ticks | Observed on a CorOS 4.1.0 unit, 2026-10-07, with the metronome not enabled: status pushes arrive continuously, one beat message per beat with tick `0` and a second message with tick `1` about 23 ms later, so `1` is a pulse flag and not a subdivision count. Earlier captures saw values `0..=21`, and other values only beside gaps in delivery to the host. The delay from tick `0` to tick `1` scaled with the beat length, about 23 ms at 120 BPM and about 14 ms at 212 BPM. | - | - | [`TempoClock::current_tick`](crate::protocol::responses::TempoClock::current_tick) |
| Tempo clock: count-in and pre-roll | With no count-in running, the beats-remaining value counts down over the bar from the bar's last beat index to `0` (`3`, `2`, `1`, `0` in four-beat bars, observed again 2026-10-07), and the bars-remaining value is `-1` (metronome and pre-roll off, at two tempos). Pre-roll was never enabled in the captures examined. | The pre-roll flag and the two count-in fields drive the unit's pre-roll indicator (beats left in the bar, bars left in the count-in), and relate to the Looper X count-in of 1, 2 or 4 bars. | The values of all three fields while a pre-roll is running. | [`TempoClock`](crate::protocol::responses::TempoClock) |
| The graphics tree | - | - | General device behaviour, request-id semantics, and behaviour across firmware versions. It is an audited host extension outside the confirmed protocol surface. | [`decode_graphics_tree`](crate::protocol::responses::decode_graphics_tree), [`commands::read_graphics_tree`](crate::commands::read_graphics_tree) |
| Plugin ownership | - | - | Whether a plugin model is licensed on the unit. The library admits plugin models and does not check; the device enforces ownership. | [`StateDecoder::assert_model_available`](crate::state::StateDecoder::assert_model_available), [`StateDecoder::license_observation`](crate::state::StateDecoder::license_observation) |
| Global EQ preset list: completeness | From a static audit of the desktop application's receiver for the model preset message (type 71), not from a USB capture: a create, or an update that lists two or more presets, appends entries with no clearing or de-duplication; an update that lists exactly one is a keyed rename or default-flag change; a delete removes by identity, and one with no entries empties the list; a read is ignored; no message marks the end of the list. The library folds the messages the same way for each state session and publishes only the entries with the Global EQ model hash (4004). | The unit sends its list at the start of a session as one large update, followed by single creates, renames and deletes. | Whether the list the library holds is complete. It is complete only relative to the messages this session received, and the library never claims more. It marks the list unusable for the rest of the session on a possible loss (a message that did not decode, an entry without a value, hash or factory flag, or a transport loss that could have cost a model preset message) and on more than 256 Global EQ entries, repeats counted, or more than 20000 retained entries in all; those two limits are the library's, not the unit's. Whether a preset that is absent from the list exists on the unit. Whether a preset requested with `create_model_preset` is created or ever appears in the list; the library looks nothing up after that write. What the unit does with a recall of an identity that was not announced; the library never sends one. | [`ModelPresetCatalog`](crate::state::ModelPresetCatalog), [`PUBLISHED_ENTRY_LIMIT`](crate::state::PUBLISHED_ENTRY_LIMIT), [`RETAINED_ENTRY_LIMIT`](crate::state::RETAINED_ENTRY_LIMIT) |
| Suspension reasons | Only the BACKUP reason has been observed on hardware. | - | The other reasons; they are handled defensively. | [`device::owner`](crate::device::owner) suspension types; [Sessions](crate::guide::sessions) |
| Secondary stomp assignments | Only primary assignments have been written to hardware. No recorded hardware output shows a unit carrying a secondary one, and the unit's own assign dialog offers only a latching or momentary choice and an unassign button, with no primary or secondary choice (CorOS 4.1.0). | A secondary assignment binds a footswitch to a model's second stomp control (for example a momentary trigger), where a primary one binds it to the block's bypass; only blocks whose model defines such a control are expected to have one. | Whether that reading is right, and what a secondary assignment looks like on a unit. | [`StompAssignmentType`](crate::commands::StompAssignmentType) |
| Looper X stomp assignment indices | Observed on a CorOS 4.1.0 unit, 2026-10-06 (raw assignments written, the Looper X editor read): writing `[0, 1, 2, 3, 4, 5, 6, 7]` showed footswitches A to H as Record, Play, Undo, Duplicate, One Shot, Half Speed, Punch In and Reverse. So index `0` is Record, `1` Play, `2` Undo, `3` Duplicate, `4` One Shot, `5` Half Speed, `6` Punch In and `7` Reverse. The unit's own arrangement, `[3, 4, 5, 6, 0, 1, 7, 2]`, shows A to H as Duplicate, One Shot, Half Speed, Punch In, Record, Play, Reverse and Undo. `assignments[i]` is the action index on footswitch `A + i`, and the library sends the indices unchanged. | The unit's labels for the secondary states, Overdub, Redo, Re-Loop and Punch Out, are what the same controls show while a loop is playing, after an undo, while Duplicate is active and while punched in. | Those secondary labels were not observed. Whether the read-back arrangement is the unit's factory arrangement (the library does not call it the default). **Experiment that would settle it:** record a short loop and watch the Record, Undo, Duplicate and Punch In controls change their labels through each state. | [`DeviceOperation::SetLooperStompAssignments`](crate::commands::DeviceOperation::SetLooperStompAssignments), [`commands::set_looper_stomp_assignments`](crate::commands::set_looper_stomp_assignments) |
| Device type values | A Quad Cortex reports device type `0`. The crate treats `1` as unsupported and rejects it at startup, which is a policy choice. | A device type of `1` is a compact variant of the unit, with four footswitches (two pages of four scenes), a smaller set of I/O ports and a power-on mode setting. | Whether any unit answers with `1`, and whether the library would work on one. Nothing here has been observed against such a device. | [`DeviceIdentity::device_type`](crate::protocol::responses::DeviceIdentity::device_type); [Sessions](crate::guide::sessions) |
| Brightness scale | Observed on a CorOS 4.1.0 unit, 2026-10-06 (raw values written, the Settings brightness page read): the wire field is a percentage-style value from `0` to `100`, and the unit shows `round(raw * 32 / 100)` steps of 32. Screen raw `25`, `50` and `75` showed `8`, `16` and `24`; LED raw `50` and `100` showed `16` and `32`. The library sends a step `s` as `round(s * 100 / 32)` and publishes the raw wire values beside its derived steps. Earlier, one raw `100` written to the dimmed-LED field was echoed as `97`. | - | The dimmed-LED field's scale was not tried and is assumed to match; whether the unit clamps the dimmed field below the LED field. | [`commands::brightness_step_to_wire`](crate::commands::brightness_step_to_wire), [`responses::decode_general_settings`](crate::protocol::responses::decode_general_settings) |
| Master volume level | The wire value is a float from `0.0` to `1.0`; the unit's own display reads `0` to `100`. The library reports a whole-number percentage (`round(value * 100)`) and sends `value / 100`. | - | What level, in decibels or any other physical measure, a given value gives at the outputs. The library does not describe the scale beyond the number. **Experiment that would settle it:** with a test tone and a meter on the assigned outputs, write `0`, `25`, `50`, `75` and `100` and record the output level for each. | [`commands::set_master_volume`](crate::commands::set_master_volume) |
| Tempo normalisation | Observed on a CorOS 4.1.0 unit, 2026-10-07: the preset tempo value `0.86` showed 212 BPM and `0.4` showed 120, so BPM is `40 + 200 * value` (the unit's tempo program data, parameter `0`), and the device-wide tempo uses the same mapping. | - | What the plain integer `tempo` field of a preset holds; when only it is present the library reads it as a BPM value, which no observation supports. **Experiment that would settle it:** read the integer `tempo` field of a preset whose tempo is known and compare. | [`commands::set_tempo`](crate::commands::set_tempo), [`commands::set_global_tempo`](crate::commands::set_global_tempo) |
| Global EQ band enabled value | Observed on a CorOS 4.1.0 unit, 2026-10-06 (raw global EQ parameters written, the Global EQ screen read): the fifth parameter of a band (band 1 is parameter index `4`) at `1.0` shows the band enabled (blue dot, BYPASS button off), and at `0.0` shows it bypassed (grey dot, dimmed controls, BYPASS button lit). The host action writes `enabled` as `1.0` or `0.0` there with no inversion, and the crate-level command takes raw index and value pairs; the raw value reads back in the parameter list. | - | - | [`DeviceOperation::SetGlobalEqParameters`](crate::commands::DeviceOperation::SetGlobalEqParameters) |
| Looper X status values | Observed on a CorOS 4.1.0 unit, 2026-10-07: `free_samples` `27131904` showed as AVAILABLE 4:43 on the screen, which is audio samples at 96 kHz (282.6 s); status `state` `1` is IDLE and `5` is ARMED, and a Record press toggles between them. The flag fields are unsigned integers on the wire, and the library reports each as true when it is above `0`. The raw `loop_length` value is published unchanged. | `loopLength` is in the same unit as `freeSamples`. | The unit of `loopLength`; the other `state` values; flag values other than `0` and `1`. **Experiment that would settle it:** record a loop of known length and read the status before and after, comparing the change in `loopLength` and `freeSamples` with the elapsed time. | [`responses::decode_looper_status`](crate::protocol::responses::decode_looper_status) |
| Looper X MIDI controls | Observed on a CorOS 4.1.0 unit, 2026-10-07 (channel 1): CC `48` values `0` and `63` open the Looper X view and `127` closes it, so the manual's polarity holds; CC `53` (Record) value `127` toggles the status state between IDLE and ARMED and value `0` is ignored; CC `51` (Half Speed) value `127` toggles the half-speed flag; CC `57` is Duplicate Mode (`0` Free, `1` Sync); CC `58` is quantize (index `0` to `9` sets parameter 18 to index/9); CC `59` is MIDI clock start (`0` or `1`); CC `61` is routing mode (index `0` to `13` sets parameter 17 to index/13). | - | The effect of CC `49`, `50`, `52`, `55` and `56` (One Shot, Punch, Reverse, Undo/Redo and Duplicate), which changed nothing with no loop recorded, so only Record and Half Speed are verified as press toggles; the effect of CC `60` (perform mode), which changed no Looper X parameter. **Experiment that would settle it:** repeat those presses with a loop recorded and read the status. | [`device::request::plan_host_midi`](crate::device::request::plan_host_midi) |
| Expression port ids | Hardware runs reported exactly two expression ports with ids `1` and `2`, and the unit's screens name two jacks. | Id `1` is the jack printed EXP 1 and id `2` is EXP 2. | Which physical jack each id belongs to; no run had a pedal plugged in. **Experiment that would settle it:** plug one pedal into one jack at a time and read which id reports it as plugged. | [`state::IoPortState`](crate::state::IoPortState) |
| Footswitch order and lamp | The unit lists footswitch assignments with a position, and the library publishes that position as `footswitchOrder`. The library derives each footswitch's `active` and `assigned` values itself; the unit reports neither. | With several blocks on one footswitch, the lowest order is the block whose state the lamp follows. | Which block the lamp follows. **Experiment that would settle it:** assign two blocks to one footswitch with different bypass states, and watch the lamp as each is toggled from the unit. | [`state::GridBlock`](crate::state::GridBlock) |
| Performance mode numbers | Observed on a CorOS 4.1.0 unit, 2026-10-07, with its available modes listed as `[1, 0, 2]` (cycle order Scene, Preset, Stomp): writing the mode field `0`, `1` and `2` showed PRESET, SCENE and STOMP on the screen. So the field is the mode enum value (`0` Preset, `1` Scene, `2` Stomp) and not an index into the cycle, and the available-modes list carries enum values in cycle order. An earlier report that writing mode `0` was ignored did not reproduce. | The hybrid values `3` to `8` are the pairings Preset/Scene, Preset/Stomp, Scene/Preset, Scene/Stomp, Stomp/Preset and Stomp/Scene, and an undefined list entry is accepted by the unit and then leaves it unusable, so the library accepts only `0` to `8` for a mode-cycle list entry. | The order of the hybrid values, which have not been written. Until it is measured the library selects only `0` to `2`: a mode slot configured with a hybrid is refused before anything is sent, and the checked encoder rejects `3` to `8`. **Experiment that would settle it:** on a scratch unit state, write each of `3` to `8` and read the mode the screen shows. | [`DeviceCommand::SelectMode`](crate::commands::DeviceCommand::SelectMode), [`DeviceOperation::SetModeCycle`](crate::commands::DeviceOperation::SetModeCycle) |
| Show tuner message | Observed on a CorOS 4.1.0 unit, 2026-10-07: the show-tuner message does nothing; no tuner screen appears and the unit has no tuner view class. | - | What the meter-enable and tuner-read frames that follow it in the library's workflow do on their own. | [`commands::show_tuner`](crate::commands::show_tuner) |
| Gig View hide frame | Observed on a CorOS 4.1.0 unit, 2026-10-07: Gig View hides on both the bare frame `08 01` and the explicit frame `08 01 18 00`, because the field is a plain proto3 boolean. The library sends the explicit form. | - | - | [`commands::show_gig_view`](crate::commands::show_gig_view) |
| Bare read of the recall-preset message | Observed on a CorOS 4.1.0 unit, 2026-10-07: a bare type-15 read with no request id is answered. | - | What the answer carries beyond that it arrives. | [`commands::read`](crate::commands::read) |
| Looper X quantize and routing values | The unit's Looper X parameter list gives quantize `0` to `9` as OFF, `1` to `8` Beats and `16` Beats, and routing mode `0` to `13` as Grid, In 1, In 2, Ret 1, Ret 2, In 1/2, Ret 1/2, Out1, Out2, Out3, Out4, Out1/2, Out3/4 and Multi. Observed on a CorOS 4.1.0 unit, 2026-10-07: `5` showed 5 Beats and `7` showed Out1 for parameter writes, and the MIDI route (CC `58` and CC `61`) set the same parameters to index/9 and index/13. | - | - | [`device::request::plan_host_midi`](crate::device::request::plan_host_midi) |
| Footswitch MIDI press | Observed on a CorOS 4.1.0 unit, 2026-10-07: footswitch CC `35` (footswitch A) toggles the assigned block on every message whatever the value (`0`, `1`, `63`, `64`, `100` and `127` all toggled), also with the footswitch set momentary. So a host sends one message per press and never a value-`0` release, which would toggle twice. Tap tempo CC `44`: five messages 0.512 s apart set the preset tempo (`0.86` to `0.385`) and the tempo clock followed at 117.07 BPM; the unit measures the interval itself. | - | - | [`device::request::plan_host_midi`](crate::device::request::plan_host_midi) |
| Unsaved-changes flag after a host edit (OPEN known gap) | After a parameter write the unit pushes PresetDirty about 0.6 s later (see the push timings below). `recall_preset`, `navigate_bank` and a PRESET-mode `press_footswitch` are refused while the retained flag reads dirty, and, until the unit has reported the flag at all in the session, refused as `UNSAVED_STATE_NOT_YET_OBSERVED` (retryable). | - | Whether a preset change planned in the window between a host edit and its PresetDirty push is admitted on the earlier clean flag and so discards the edit; a PresetDirty message carries no sequence, so the host cannot tell a stale clean flag from a current one. Also whether a reload discards edits, and whether a stale preset body or clean flag can arrive after a position change. This gap is open. **Experiment that would settle it:** capture the dirty-flag lag after an edit, whether a reload discards edits, and any stale publication after a position change. | [`device::request::plan_preset_recall`](crate::device::request::plan_preset_recall) |
| Hybrid mode footswitch split | - | In a hybrid mode `A/B`, the first mode applies to footswitches A-D and the second to E-H. | Whether that split is right on the unit. | [`DeviceCommand::SelectMode`](crate::commands::DeviceCommand::SelectMode) |
| Footswitch mode values | A unit's mode message carried `mode = 2` with the configured list `[1, 0, 2]`. A write of mode `1` changed the active mode; an explicit write of `0` did not change active mode `2` within four seconds. | The wire schema declares no mode enum. The names follow the numbering the vendor's desktop application uses for its own mode enum: `0` Preset, `1` Scene, `2` Stomp, and `3` to `8` the hybrids Preset/Scene, Preset/Stomp, Scene/Preset, Scene/Stomp, Stomp/Preset, Stomp/Scene. | Why the write of `0` had no effect. What a value above `8` would mean: the library reports it as `UNKNOWN` with the number in `raw_mode`, and names no per-row modes for it. | [`StateDecoder`](crate::state::StateDecoder), [`GatewaySnapshot`](crate::device::GatewaySnapshot) |
| MIDI Out source numbering in a preset | A preset read from a unit holds twelve preset-load rows in one field, 120 rows in the flat list of per-source rows, and ten legacy rows. The vendor application's own flat model holds 132 rows, which is the preset-load group and ten sources of twelve. A preset-load edit has been pushed back as one group of twelve rows with no source on it. Observed on a CorOS 4.1.0 unit, 2026-10-07 (live read and a save-then-recall of a scratch slot): group `s` of the 120 rows is source `s` (footswitches A to H, then the two expression inputs), so source `s` occupies flat rows `s * 12` to `s * 12 + 11`. Each source's published list is positional: a message sits at the index of its row, an empty row between messages is a type `0` entry and trailing empties are dropped. `presetFlatOffset` reports where each group began. | - | Whether the vendor application's own flat model, which puts source `s` at row `(s + 1) * 12`, is the same layout seen from the other side of the preset-load group. | [`MidiOutSource`](crate::state::MidiOutSource), [`StateDecoder`](crate::state::StateDecoder) |
| Block bypass records | Full presets read from a unit carry a bypass list of four rows of eight entries that names no row and no column, with eight identical per-scene flags per entry. The scene-mode flag was present and false on every occupied block and absent on empty cells. | A record that names its own row or column is addressed by it, and one that names neither is addressed by its place in the list. A record's value for the active scene is the block's bypass state. | What a scene-mode flag of false means for a record's eight per-scene flags, and how a unit addresses the records of a Grid push beyond the one shape seen. Both are reported (`bypass_scene_mode`, `bypass_address`) rather than assumed. | [`GridBlock`](crate::state::GridBlock) |
| Undo and redo availability | The undo list a unit pushed held twenty entries, mostly zero, and the redo list was absent. | A list with any entries means something can be undone. | Whether that is true: a list of twenty zeros reads as available. What the entries mean. The raw lists are reported (`undo_frames`, `redo_frames`). | [`StateDecoder`](crate::state::StateDecoder) |
| Route ids on a Grid row | A preset read from a unit wrote ids `1` and `19` on one row and an explicit `0` on its unrouted rows. | The wire enums for input and output ports, which are declared on the gain-calibration messages, also number a Grid row's input and output. The unrouted id `0` is the enum's `EMPTY`, shown as `Internal` by the library and as Not In Use on the unit. | Whether the numbering carries over from the gain-calibration messages, and what an absent id means (it is reported as `None`, not `0`). | [`INPUT_ROUTE_WIRE_NAMES`](crate::domain::INPUT_ROUTE_WIRE_NAMES), [`OUTPUT_ROUTE_WIRE_NAMES`](crate::domain::OUTPUT_ROUTE_WIRE_NAMES) |
| Parameter index when absent | A full preset's tempo block of 24 parameters named no index on any of them. | In a full record the position is the index. In a sparse push an absent index is `0`, as the vendor application's receiver reads it. | Whether every other model in a full preset is addressed by position too. | [`StateDecoder`](crate::state::StateDecoder) |
| Persistence of device-wide settings | - | - | Whether each is stored immediately. The library sends them as global writes. | [Sending commands safely](crate::guide::send_commands) |
| Firmware compatibility | - | - | Any per-firmware matrix. The profile names one host-protocol version and the USB identifiers; the repository records no firmware test matrix. | [`protocol::profile`](crate::protocol::profile); crate README |
| Why a keepalive is needed during a backup | - | - | The device's side of the requirement. | [`BackupAction::Keepalive`](crate::backup::BackupAction::Keepalive); [Backups](crate::guide::backups) |
| Interaction of the two keepalives | - | - | How the connection's deferrable keepalive and the backup's non-deferrable one interact on a unit. | [Backups](crate::guide::backups) |
| OS write results | - | - | Which native USB result corresponds to an *expected status stall*. | [`ReportOutcome`](crate::device::delivery::ReportOutcome); [Transports](crate::guide::transports) |
| Payload of a backup | - | - | What is inside the base64 `payload`. The crate checks the wrapper, not the contents. | [`finalize_device_backup`](crate::device::request::finalize_device_backup) |

## Host policy and unverified behaviour

Some of what the library does is not a frame the desktop application sends. It is
host policy: a read, guard, ladder, timeout or refusal the library adds around the
frames. Each item below says what it is, where it lives, whether it protects the
user's work or only observes, and its tier (**established**: a retained
measurement or the desktop application's own behaviour from a static audit;
**believed**: stated in a comment or note with no retained measurement;
**unknown**: nothing retained settles it). The constants are in
[`profile`](crate::protocol::profile) and the planners in
[`device::request`](crate::device::request).

| Host policy | Where | Protects work or only observes | Tier |
| --- | --- | --- | --- |
| Passive write confirmation. After a write the host waits for the QC's own pushed state to confirm it (3 s, 15 s for a preset transition) and sends NOTHING while it waits: the type-15 verification READs it used to send at 0, 250 and 1500 ms (and on the preset-transition ladders), the settle reads and the quarantine resync reads are removed, because the desktop application sends a type-15 READ only at boot. When no confirming state is pushed the result is `accepted_unverified`: not an error, and it quarantines nothing. | `device::request::gateway_write_verification_policy`, the broker's write path | Only observes | Believed: the 3 s and 15 s deadlines. Whether the unit pushes confirming state for every write family is unknown (experiments pending). |
| Write quarantine, kept only for genuine delivery ambiguity (a lost USB link, a native delivery that timed out, a session that restarted under the write, a persistent File workflow whose frames were attempted but not all delivered). It clears only on a new session epoch or `reset_device_session`; the broker sends no read to clear it. | The broker's write-quarantine module | Protects an unverified live preset from being saved over | Policy. |
| Retained-publication poll (0, 100, 250, 500, 1000 ms) while waiting for the QC's own sparse echo of an I/O or general-settings write. It reads a cache and sends nothing. Every other write that used to poll a READ (Global EQ, mode cycle, tempo and metronome, favorite and pinned-model writes) is `accepted_unverified` after its frames are delivered. | `device::request::gateway_write_readback_method` | Only observes | Believed (the timings). Measured 2026-10-07 on CorOS 4.1.0: a Global EQ or Mode update is echoed by a push within about 10 to 50 ms, and a global-tempo update in PRESET tempo mode is answered by UndoRedo, Grid and PresetDirty pushes, never a type-33 echo. |
| Published-echo wait (3 s) after I/O and general-settings writes. Sends nothing; no echo gives `accepted_unverified`. | The broker's write path | Only observes | Believed: that the unit answers no on-demand READ of those two types is recorded without a date or artifact. |
| Stale-state guards from decoded state, with no read: scene selection and tempo mode compare against what the session already holds and skip a write that already matches (the desktop application skips those two too); `setGlobalTempo`, `setFavorite` and `setModelPinned` only check the caller's `expected_*` value and always send the frame, as the desktop application does. Pinning a model (not unpinning it) also needs the session's model catalog to list the model and not hide it, as adding or replacing a block does; with no catalog synchronized the write is refused with `STATE_UNAVAILABLE`. A guard whose state has not been published fails closed and sends nothing (for favorites, call `list_favorites` first; for pinned models, `list_pinned_models`). | `device::request::gateway_write_preflight_method`, the broker's write path | Protects the user's work from stale input | Policy. |
| Expected-state guards, operand fence, dirty-preset refusals (recall, bank navigation, reload, a footswitch press in PRESET mode), confirmation flags, no-replay of uncertain writes. | `device::request`, the MCP layer | Protects the user's work | Policy. The premise that a recall or press discards unsaved edits is believed, not shown. |
| Read timeouts per method and ONE same-session re-send of a read after a timeout-class error (not for diagnostics or looper status), then a refusal that does not reset the session. The re-send shares the first attempt's budget, so for a single-frame read it is practically unreachable. | The broker's gateway read path | Only observes | Believed; no test exercises the re-send. |
| Session timing: reconnect delay 750 ms doubling to 10 s, handshake 30 s with 5 s per attempt, 2 s initial sync, 35 s ready wait, 15 s preset sync, startup rebuild cap of 3, 60 s startup-stall teardown, 250 ms hold before the first write. | `profile`, `device::transport`, `device::initialization`, the broker worker | Keeps the session usable | Believed; no measurement retained. |
| File verification after a library write: passive first (the unit's own File publication, including anything pushed while the host was still settling), then at most three `readFolders(false)` reads after a 3 s passive window, then passive again; the result is `accepted_unverified` when no listing confirms it (a stale first listing is not a contradiction). The desktop application does not poll. | `device::catalog`, `device::preset_catalog`, the broker's preset workflow | Only observes | Timings believed. Established on CorOS 4.1.0 (direct USB, 2026-10-07): the unit answers a bare directory READ on demand every time within seconds, and the first listing after a mutation can lag. |
| The explicit-refresh refusal for `list_presets`, `list_preset_folders` (`PRESET_CATALOG_REFRESH_UNAVAILABLE`). | `device::request` | Avoids a session rebuild | Policy: a refresh is not implemented. The unit does answer a directory read (established 2026-10-07), so this is a missing feature, not a device limit. |
| Refusing a numeric write to a time or rate control the device reports disabled behind an engaged SYNC switch. | `device::request` | Stops a write the desktop application's UI never offers | Policy. What the unit does with such a write is believed (a 2026-09-11 measurement recorded without an artifact), not established. |
| Backup budgets (180 s total, 60 s first chunk, 15 s stall, 32 MiB) and the unconditional 1 s keepalive clock during a backup. | `profile`, the broker worker | Protects the transfer | The 1 s keepalive interval is the desktop application's; the budgets and the need for an unconditional clock are believed. |
| Touch, MIDI and gesture pacing. | `profile`, the broker worker | Pacing only | Out of scope here; see the touch commands' own documentation. |

### Established by direct USB runs (CorOS 4.1.0, 2026-10-07)

- Direct File COPY and File UPDATE take effect with or without a request id. COPY
  de-duplicates names (a copy of `QC MIDI SCRATCH` into an empty slot of the same
  folder was created as `QC MIDI SCRATCH_1`). UPDATE does NOT: renaming onto a name
  another slot holds leaves both slots listing the same name and the same key, so
  `rename_current_preset` refuses such a target. The first listing after a COPY or
  UPDATE can lag.
- A bare File directory READ (`08 03 58 00`), a bare type-15 READ and a recall
  without a request id are all answered or applied.
- After a parameter write the unit pushes UndoRedo and a Grid echo within about
  0.1 s and PresetDirty about 0.6 s later; after an undo it pushes a full preset,
  Scene, SetlistPosition, UndoRedo and Grid within about 0.25 s. Block and lane details
  are therefore served from pushed state.
- A Global EQ or Mode update is echoed by a push within about 10 to 50 ms.
- A global-tempo update (type 33, parameter 0) in PRESET tempo mode is not ignored: it
  edits the loaded preset's tempo and dirties the preset (UndoRedo, Grid and PresetDirty
  pushes, no type-33 echo).

### Hardware verification pending

These behaviours were changed or specified to follow the desktop application's
frames and have **no run on a unit since the change**. Treat a failure of any of
them as a result to investigate, never to retry blindly.

- Tuner and device-name writes without a readback.
- Undo and redo without a following refresh read (undo's pushed state is
  established above; redo is not measured).
- `set_global_bypass` with one family per call.
- `move_block` without the desktop application's field-3 sidecar. Established as a
  dated 2026-10-06 measurement whose raw result files are not retained: the bare
  frame applied four of four times and the earlier all-rows sidecar form was
  ignored two of four times. Why it was ignored is unknown, and a sidecar shaped
  exactly like the desktop application's has not been tested.
- Brightness and output-level wire values (step-to-percent scaling and the
  `(dB+40)/52` output scale are inferred; some of the evidence is undated).
- `press_footswitch` momentary polarity (what a momentary stomp does with a
  press and no release).
- The two-frame form of `set_expression_bypass`, and whether the desktop
  application also sends the info frame when clearing.

## What the library does not implement

- **Restoring a backup.** There is no request that sends one to a unit.
- **The encrypted families.** Cloud login, product forwarding, backup forwarding,
  log forwarding, cloud backup and license messages. They are rejected by name.
- **Opening a device.** No USB, HID or MIDI handle, ever.
- **Retrying a persistent write.** By design; see
  [Sending commands safely](crate::guide::send_commands).

## FAQ

### Is this an official SDK?

No. It is independent and unofficial, and is not affiliated with, authorized,
sponsored, endorsed or supported by Neural DSP Technologies Oy. See the
[start of this guide](crate::guide).

### Can I depend on it from my own project?

By path or by git revision. The manifest sets `publish = false`, so
`cargo publish` is refused and there is no crates.io release. The crate README
shows both forms. The crate is pre-1.0; pin a revision.

### Which Rust version do I need?

1.87 or newer. See [section 1](crate::guide::overview) for how that was verified and
what it does not cover.

### Which operating systems does it work on?

The crate has no platform-specific code or dependencies, and the repository's CI
builds and tests it on Windows. Whether it builds and behaves on other platforms
has not been checked by this guide. The *hosts* in the repository exist for
Windows and Android only.

### Do I need `serde_json`?

For the guarded layer, yes. The planners
([`plan_gateway_read`](crate::device::request::plan_gateway_read),
[`plan_gateway_write`](crate::device::request::plan_gateway_write),
[`plan_preset_mutation`](crate::device::request::plan_preset_mutation)) take
`serde_json::Value` parameters, and state updates serialize to a camelCase JSON
contract. The guide's snippets that build protobuf messages also use `prost`
0.14. Add both to your own `Cargo.toml` when you do the same.

### Where do the method names (`device.setTempo`) come from?

They are the shared gateway vocabulary generated from the project's contracts.
[`generated_gateway::API_VERSION`](crate::device::generated_gateway::API_VERSION),
[`CAPABILITIES`](crate::device::generated_gateway::CAPABILITIES) and
[`METHODS`](crate::device::generated_gateway::METHODS) are the generated lists.
The generators emit documentation for them; the crate README says which
generated modules that covers. Propose changes through the project's schema
rather than editing generated files.

### How do I read a block's parameters?

[`DeviceStateRuntime::block_details(row, column)`](crate::device::state_runtime::DeviceStateRuntime::block_details)
returns the parameters and values of the block at a grid cell, as far as the
installed model catalog describes them. The grid is
[`domain::GRID_ROWS`](crate::domain::GRID_ROWS) rows by
[`domain::GRID_COLUMNS`](crate::domain::GRID_COLUMNS) columns.

### Why can't I refresh the preset list on demand?

The library says: the unit publishes its preset directory at the start of each USB
session and pushes File updates as the library changes. Reads of the directory are
served from what the session already holds; a new session pulls a new one. The unit
does answer a bare directory read when asked (established on CorOS 4.1.0, 2026-10-07),
so an explicit refresh is possible but is not implemented here; it is a missing
feature, not a device limit. The library's own post-write verification is passive first
and then sends at most a few such reads (see "Host policy and unverified behaviour"
above). See
[`PRESET_CATALOG_REFRESH_UNAVAILABLE`](crate::device::request::PRESET_CATALOG_REFRESH_UNAVAILABLE).
The general and I/O settings are handled the same way: the state runtime retains
what the unit publishes and merges later sparse echoes
([`DeviceStateRuntime::published_settings`](crate::device::state_runtime::DeviceStateRuntime::published_settings)).

### Can I send any message type I like?

At the framing level, yes: [`framing::encode`](crate::protocol::framing::encode)
takes a type and bytes. At the command level the crate deliberately offers named
operations; the generic read builder is `#[doc(hidden)]` with a note that callers
must not present an arbitrary message type as a sanctioned device operation. Be
conservative: sending arbitrary protocol messages to a device can mutate
persistent state.

### Does it collect or transmit anything?

Not by itself. The crate opens no device, file or socket, and does not send or
store anything. (It reads the system clock to timestamp suspension diagnostics and
state publications.) What a host does with the data is the host's business, and a
capture of a unit can include personal data; see [Testing](crate::guide::testing).

### How do I report a problem or propose a change?

Open an issue or pull request in the repository. `CONTRIBUTING.md` describes the
sign-off the project asks for. For a wrong or missing statement in this guide,
include the page and the item that contradicts it.

## What to check before you rely on any of this

1. **Run your own loop against a real unit, in a controlled setup, before you
   trust it.** The guide's authors ran none of it against hardware.
2. **Guard every write with freshly observed state**, and re-read after any
   uncertain outcome. Delivery is not acknowledgement.
3. **Never replay a persistent write on a guess.**
4. **Keep your reader lossless.** The wire format has no checksum and no sequence
   numbers.
5. **Pin the crate revision** and read the changelog entries for the library when
   you update.
6. **Check the "not established" register above** for any behaviour you assume.
