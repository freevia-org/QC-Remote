// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
/// USB vendor ID of the supported devices.
pub const VENDOR_ID: u16 = 5418;
/// USB product IDs of the supported devices, in ascending order. [`supports_product_id`] searches it.
pub const SUPPORTED_PRODUCT_IDS: &[u16] = &[34826, 35119];
/// Version string the host announces to the device in its opening Version message.
pub const CORTEX_CONTROL_VERSION: &str = "4.1.0";
/// Largest message payload, in bytes, that the host sends or accepts in one frame.
pub const MAX_FRAME_BYTES: usize = 1048576;
/// Ceiling, in bytes, on the size a gzip-compressed payload may expand to.
pub const MAX_INFLATED_BYTES: usize = 16777216;
/// Interval, in milliseconds, between the keepalive messages the host sends while a session is open.
pub const KEEPALIVE_INTERVAL_MS: u64 = 1000;
/// Number of HID reports sent before the host pauses for [`HID_REPORT_BATCH_DELAY_MS`].
pub const HID_REPORT_BATCH_SIZE: usize = 10;
/// Pause, in milliseconds, after each full batch of [`HID_REPORT_BATCH_SIZE`] HID reports.
pub const HID_REPORT_BATCH_DELAY_MS: u64 = 6;
/// Base delay, in milliseconds, between connection attempts. It doubles with each consecutive failure up to a fixed cap. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const RECONNECT_INTERVAL_MS: u64 = 750;
/// Minimum gap, in milliseconds, between two consecutive performance MIDI messages. Pacing only; host policy, no measurement in the repository.
pub const PERFORMANCE_MIDI_GAP_MS: u64 = 8;
/// Interval, in milliseconds, that the former two-frame screen tap left between its frames. No gesture uses it now: a tap is a single TAP frame and a swipe a single DRAG frame. It is kept as the floor the hold and stepped-drag intervals are checked against. Observed on CorOS 4.1.0 (2026-10-06): a touch-down then a touch-up this close together toggled the preset-menu button 0 times in 10 at 20 ms and 1 in 10 at 80 ms (3 in 3 at 900 ms), so it is too short for the unit.
pub const REMOTE_GESTURE_INTERVAL_MS: u64 = 20;
/// Interval, in milliseconds, between the two frames of a screen hold. It is the hold's duration, and is longer than [`REMOTE_GESTURE_INTERVAL_MS`].
pub const REMOTE_HOLD_INTERVAL_MS: u64 = 1200;
/// Interval, in milliseconds, between every frame of a stepped screen drag: the touch-down, each move and the touch-up. It is longer than [`REMOTE_GESTURE_INTERVAL_MS`].
pub const REMOTE_DRAG_INTERVAL_MS: u64 = 50;
/// Time, in milliseconds, after which a handshake that has not completed is abandoned. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const HANDSHAKE_TIMEOUT_MS: u64 = 30000;
/// Time, in milliseconds, to wait for a reply to one handshake attempt before the next attempt is made. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const HANDSHAKE_ATTEMPT_TIMEOUT_MS: u64 = 5000;
/// Time, in milliseconds, allowed for the first preset read after startup. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const INITIAL_SYNC_TIMEOUT_MS: u64 = 2000;
/// Delay, in milliseconds, before the first host write after initialization. The device can report its initial preset before it is ready to accept a mutation. Host policy (not a desktop-application frame); the stated reason is BELIEVED, no measurement in the repository.
pub const POST_INITIALIZATION_WRITE_DELAY_MS: u64 = 250;
/// Time, in milliseconds, that startup may take to reach the ready state. Each startup rebuild starts the wait again. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const READY_WAIT_TIMEOUT_MS: u64 = 35000;
/// Time, in milliseconds, allowed for preset synchronization, including preset transitions such as recall, bank navigation and reload. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const PRESET_SYNC_TIMEOUT_MS: u64 = 15000;
/// Time, in milliseconds, that an ordinary command waits for the device to confirm it. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const COMMAND_CONFIRMATION_TIMEOUT_MS: u64 = 3000;
/// Intervals, in milliseconds, between polls of a retained settings publication while waiting for the QC's own sparse echo of a type-9 or type-3 write. The first is zero, meaning an immediate poll, and they increase. Host policy (not a desktop-application frame): the desktop application never reads a write back, and a poll sends no frame; it only reads a cache. BELIEVED, no measurement in the repository.
pub const CORRELATED_WRITE_READBACK_RETRY_INTERVALS_MS: &[u64] = &[0, 100, 250, 500, 1000];
/// Upper bound, in milliseconds, on the retry delay of the default preset-library verification policy. It is also the wait imposed after a stale listing. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const PRESET_LIBRARY_REFRESH_COALESCE_MS: u64 = 8000;
/// Time, in milliseconds, that the default policy waits for the preset library to reflect a change before giving up. Host policy (not a desktop-application frame). The verification waits passively first and then sends at most a small bounded number of directory reads; the unit answers an on-demand directory read (ESTABLISHED, CorOS 4.1.0, 2026-10-07) but its first listing after a mutation can lag.
pub const PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS: u64 = 45000;
/// Initial retry delay, in milliseconds, of the default preset-library verification policy. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const PRESET_LIBRARY_VERIFICATION_RETRY_MS: u64 = 500;
/// Host-side safety budget for one non-replayable persistent File mutation.
/// There is no operation acknowledgement or timeout here, and a save or
/// catalog mutation can remain unsettled for tens of seconds before
/// converging, so persistent verification uses a wider, bounded window than
/// an ordinary catalog read. Host policy (not a desktop-application frame);
/// the 120 s figure is BELIEVED, no measurement in the repository. The window
/// is passive-first: the verification sends at most a small bounded number of
/// directory reads after a passive window, and the rest of the budget only
/// observes pushed File publications. The unit answers an on-demand directory
/// read (ESTABLISHED, CorOS 4.1.0, 2026-10-07).
pub const PERSISTENT_FILE_VERIFICATION_TIMEOUT_MS: u64 = 120000;
/// Initial retry delay, in milliseconds, when verifying a persistent File mutation. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const PERSISTENT_FILE_VERIFICATION_RETRY_MS: u64 = 2000;
/// Upper bound, in milliseconds, on the retry delay when verifying a persistent File mutation. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const PERSISTENT_FILE_VERIFICATION_MAXIMUM_RETRY_MS: u64 = 15000;
/// Base readback timeout, in milliseconds, for a duplicated setlist, before the per-preset allowance is added. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const DUPLICATE_SETLIST_VERIFICATION_BASE_TIMEOUT_MS: u64 = 90000;
/// Readback timeout, in milliseconds, added for each occupied preset in a duplicated setlist. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const DUPLICATE_SETLIST_VERIFICATION_PER_PRESET_MS: u64 = 2000;
/// Upper bound, in milliseconds, on the readback timeout for a duplicated setlist. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const DUPLICATE_SETLIST_VERIFICATION_MAXIMUM_TIMEOUT_MS: u64 = 600000;
/// Initial retry delay, in milliseconds, when verifying a duplicated setlist. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const DUPLICATE_SETLIST_VERIFICATION_RETRY_MS: u64 = 2000;
/// Upper bound, in milliseconds, on the retry delay when verifying a duplicated setlist. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const DUPLICATE_SETLIST_VERIFICATION_MAXIMUM_RETRY_MS: u64 = 30000;
/// Quiet time, in milliseconds, after the last File message of a full directory read before the directory is treated as complete. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const PRESET_LIBRARY_SETTLEMENT_QUIET_MS: u64 = 250;
/// Time limit, in milliseconds, for a whole device backup. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const BACKUP_TOTAL_TIMEOUT_MS: u64 = 180000;
/// Time, in milliseconds, to wait for the first chunk of a backup transfer. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const BACKUP_FIRST_CHUNK_TIMEOUT_MS: u64 = 60000;
/// Time, in milliseconds, a backup transfer may go without a new chunk before it is treated as stalled. Host policy (not a desktop-application frame); BELIEVED, no measurement in the repository.
pub const BACKUP_STREAM_STALL_TIMEOUT_MS: u64 = 15000;
/// Largest assembled backup document, in bytes. A larger document is rejected. Host policy (not a desktop-application frame); a size ceiling, no measurement in the repository.
pub const BACKUP_MAXIMUM_DOCUMENT_BYTES: usize = 33554432;
/// MIDI status byte for a control change on channel 1.
pub const MIDI_CONTROL_CHANGE_STATUS: u8 = 176;
/// USB-MIDI event packet header byte for a control change on cable 0.
pub const MIDI_USB_EVENT_PACKET_HEADER: u8 = 11;
/// Controller number of the first footswitch. A footswitch with zero-based index `n` uses this plus `n`.
pub const FOOTSWITCH_BASE_CONTROLLER: u8 = 35;
/// Controller number of the tap-tempo message.
pub const TAP_TEMPO_CONTROLLER: u8 = 44;
// TUNER_CONTROLLER, GIG_VIEW_CONTROLLER, MODE_SLOT_CONTROLLER,
// MIDI_FEATURE_OFF_VALUE, and MIDI_FEATURE_ON_VALUE below have zero call sites
// anywhere in quadcortex or quadcortex::device: device.showTuner,
// device.showGigView, and device.selectModeSlot dispatch through the generated
// gateway path, never host MIDI. The constants remain part of the shared
// protocol contract because other generated clients use them.
// Do not treat their presence here as evidence that a live host-MIDI path
// exists in this crate.
/// Controller number that the shared protocol contract assigns to the tuner.
pub const TUNER_CONTROLLER: u8 = 45;
/// Controller number that the shared protocol contract assigns to Gig View.
pub const GIG_VIEW_CONTROLLER: u8 = 46;
/// Controller number that the shared protocol contract assigns to mode-slot selection.
pub const MODE_SLOT_CONTROLLER: u8 = 47;
/// Controller value that signals a press.
pub const MIDI_PRESSED_VALUE: u8 = 127;
/// Controller value that switches a feature off.
pub const MIDI_FEATURE_OFF_VALUE: u8 = 0;
/// Controller value that switches a feature on.
pub const MIDI_FEATURE_ON_VALUE: u8 = 127;
/// Controller number of the Looper X editor. The host sends 0 to open it and [`MIDI_PRESSED_VALUE`] to close it. Observed on a CorOS 4.1.0 unit, 2026-10-07: 0 and 63 open the editor and 127 closes it.
pub const LOOPER_MENU_CONTROLLER: u8 = 48;
/// Controller number of the Looper X duplicate command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_DUPLICATE_CONTROLLER: u8 = 49;
/// Controller number of the Looper X one-shot command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_ONE_SHOT_CONTROLLER: u8 = 50;
/// Controller number of the Looper X half-speed command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_HALF_SPEED_CONTROLLER: u8 = 51;
/// Controller number of the Looper X punch command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_PUNCH_CONTROLLER: u8 = 52;
/// Controller number of the Looper X record command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_RECORD_CONTROLLER: u8 = 53;
/// Controller number of the Looper X play command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_PLAY_CONTROLLER: u8 = 54;
/// Controller number of the Looper X reverse command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_REVERSE_CONTROLLER: u8 = 55;
/// Controller number of the Looper X undo/redo command, triggered by [`MIDI_PRESSED_VALUE`].
pub const LOOPER_UNDO_REDO_CONTROLLER: u8 = 56;
/// Controller number of the Looper X duplicate-mode setting, which takes a value from 0 to 1.
pub const LOOPER_DUPLICATE_MODE_CONTROLLER: u8 = 57;
/// Controller number of the Looper X quantize setting, which takes a value from 0 to 9.
pub const LOOPER_QUANTIZE_CONTROLLER: u8 = 58;
/// Controller number of the Looper X MIDI-clock-start setting, which takes a value from 0 to 1.
pub const LOOPER_MIDI_CLOCK_START_CONTROLLER: u8 = 59;
/// Controller number of the Looper X perform-mode setting, which takes a value from 0 to 1.
pub const LOOPER_PERFORM_MODE_CONTROLLER: u8 = 60;
/// Controller number of the Looper X routing-mode setting, which takes a value from 0 to 13.
pub const LOOPER_ROUTING_MODE_CONTROLLER: u8 = 61;
/// Wire type number of the `Grid` message (1).
pub const MESSAGE_TYPE_GRID: u16 = 1;
/// Wire type number of the `SetlistPosition` message (2).
pub const MESSAGE_TYPE_SETLIST_POSITION: u16 = 2;
/// Wire type number of the `IOSettings` message (3).
pub const MESSAGE_TYPE_IO_SETTINGS: u16 = 3;
/// Wire type number of the `File` message (4).
pub const MESSAGE_TYPE_FILE: u16 = 4;
/// Wire type number of the `IOMeter` message (5).
pub const MESSAGE_TYPE_IO_METER: u16 = 5;
/// Wire type number of the `Tuner` message (6).
pub const MESSAGE_TYPE_TUNER: u16 = 6;
/// Wire type number of the `Diagnostics` message (7).
pub const MESSAGE_TYPE_DIAGNOSTICS: u16 = 7;
/// Wire type number of the `MIDISettings` message (8).
pub const MESSAGE_TYPE_MIDI_SETTINGS: u16 = 8;
/// Wire type number of the `GeneralSettings` message (9).
pub const MESSAGE_TYPE_GENERAL_SETTINGS: u16 = 9;
/// Wire type number of the `Version` message (10).
pub const MESSAGE_TYPE_VERSION: u16 = 10;
/// Wire type number of the `ProductionAutomationMode` message (11).
pub const MESSAGE_TYPE_PRODUCTION_AUTOMATION_MODE: u16 = 11;
/// Wire type number of the `GridMove` message (12).
pub const MESSAGE_TYPE_GRID_MOVE: u16 = 12;
/// Wire type number of the `Scene` message (13).
pub const MESSAGE_TYPE_SCENE: u16 = 13;
/// Wire type number of the `Mode` message (14).
pub const MESSAGE_TYPE_MODE: u16 = 14;
/// Wire type number of the `RecallPreset` message (15).
pub const MESSAGE_TYPE_RECALL_PRESET: u16 = 15;
/// Wire type number of the `EnableCaptureOut` message (16).
pub const MESSAGE_TYPE_ENABLE_CAPTURE_OUT: u16 = 16;
/// Wire type number of the `MasterVolume` message (17).
pub const MESSAGE_TYPE_MASTER_VOLUME: u16 = 17;
/// Wire type number of the `CloudLogin` message (18).
pub const MESSAGE_TYPE_CLOUD_LOGIN: u16 = 18;
/// Wire type number of the `DefaultParameters` message (19).
pub const MESSAGE_TYPE_DEFAULT_PARAMETERS: u16 = 19;
/// Wire type number of the `RecentsFavorites` message (20).
pub const MESSAGE_TYPE_RECENTS_FAVORITES: u16 = 20;
/// Wire type number of the `UndoRedo` message (21).
pub const MESSAGE_TYPE_UNDO_REDO: u16 = 21;
/// Wire type number of the `SceneCopy` message (22).
pub const MESSAGE_TYPE_SCENE_COPY: u16 = 22;
/// Wire type number of the `SceneLabel` message (23).
pub const MESSAGE_TYPE_SCENE_LABEL: u16 = 23;
/// Wire type number of the `ShowGigView` message (24).
pub const MESSAGE_TYPE_SHOW_GIG_VIEW: u16 = 24;
/// Wire type number of the `Screenshot` message (25).
pub const MESSAGE_TYPE_SCREENSHOT: u16 = 25;
/// Wire type number of the `CPULoad` message (26).
pub const MESSAGE_TYPE_CPU_LOAD: u16 = 26;
/// Wire type number of the `ShowTuner` message (27).
pub const MESSAGE_TYPE_SHOW_TUNER: u16 = 27;
/// Wire type number of the `Looper` message (28).
pub const MESSAGE_TYPE_LOOPER: u16 = 28;
/// Wire type number of the `ProductForward` message (29).
pub const MESSAGE_TYPE_PRODUCT_FORWARD: u16 = 29;
/// Wire type number of the `BackupsForward` message (30).
pub const MESSAGE_TYPE_BACKUPS_FORWARD: u16 = 30;
/// Wire type number of the `LogsForward` message (31).
pub const MESSAGE_TYPE_LOGS_FORWARD: u16 = 31;
/// Wire type number of the `KeepAlive` message (32).
pub const MESSAGE_TYPE_KEEP_ALIVE: u16 = 32;
/// Wire type number of the `GlobalTempo` message (33).
pub const MESSAGE_TYPE_GLOBAL_TEMPO: u16 = 33;
/// Wire type number of the `PresetDirty` message (34).
pub const MESSAGE_TYPE_PRESET_DIRTY: u16 = 34;
/// Wire type number of the `ModuleStats` message (35).
pub const MESSAGE_TYPE_MODULE_STATS: u16 = 35;
/// Wire type number of the `NeuralCapture` message (36).
pub const MESSAGE_TYPE_NEURAL_CAPTURE: u16 = 36;
/// Wire type number of the `GridModelMeter` message (37).
pub const MESSAGE_TYPE_GRID_MODEL_METER: u16 = 37;
/// Wire type number of the `GlobalEQ` message (38).
pub const MESSAGE_TYPE_GLOBAL_EQ: u16 = 38;
/// Wire type number of the `RecentSearches` message (39).
pub const MESSAGE_TYPE_RECENT_SEARCHES: u16 = 39;
/// Wire type number of the `LocalBackup` message (40).
pub const MESSAGE_TYPE_LOCAL_BACKUP: u16 = 40;
/// Wire type number of the `CloudBackup` message (41).
pub const MESSAGE_TYPE_CLOUD_BACKUP: u16 = 41;
/// Wire type number of the `CompilerInhibitedModules` message (42).
pub const MESSAGE_TYPE_COMPILER_INHIBITED_MODULES: u16 = 42;
/// Wire type number of the `SystemTimeSync` message (43).
pub const MESSAGE_TYPE_SYSTEM_TIME_SYNC: u16 = 43;
/// Wire type number of the `Logs` message (44).
pub const MESSAGE_TYPE_LOGS: u16 = 44;
/// Wire type number of the `ProcessDownloadsQueue` message (45).
pub const MESSAGE_TYPE_PROCESS_DOWNLOADS_QUEUE: u16 = 45;
/// Wire type number of the `CloudProduct` message (46).
pub const MESSAGE_TYPE_CLOUD_PRODUCT: u16 = 46;
/// Wire type number of the `Confirmation` message (47).
pub const MESSAGE_TYPE_CONFIRMATION: u16 = 47;
/// Wire type number of the `SceneColor` message (48).
pub const MESSAGE_TYPE_SCENE_COLOR: u16 = 48;
/// Wire type number of the `Connection` message (49).
pub const MESSAGE_TYPE_CONNECTION: u16 = 49;
/// Wire type number of the `NewModels` message (50).
pub const MESSAGE_TYPE_NEW_MODELS: u16 = 50;
/// Wire type number of the `ModelRepo` message (51).
pub const MESSAGE_TYPE_MODEL_REPO: u16 = 51;
/// Wire type number of the `ResetCommsBuffers` message (52).
pub const MESSAGE_TYPE_RESET_COMMS_BUFFERS: u16 = 52;
/// Wire type number of the `SuspendConnection` message (53).
pub const MESSAGE_TYPE_SUSPEND_CONNECTION: u16 = 53;
/// Wire type number of the `PinnedModels` message (54).
pub const MESSAGE_TYPE_PINNED_MODELS: u16 = 54;
/// Wire type number of the `GigViewButton` message (55).
pub const MESSAGE_TYPE_GIG_VIEW_BUTTON: u16 = 55;
/// Wire type number of the `GenericError` message (56).
pub const MESSAGE_TYPE_GENERIC_ERROR: u16 = 56;
/// Wire type number of the `BulkOperation` message (57).
pub const MESSAGE_TYPE_BULK_OPERATION: u16 = 57;
/// Wire type number of the `License` message (58).
pub const MESSAGE_TYPE_LICENSE: u16 = 58;
/// Wire type number of the `PresetSpeedTest` message (59).
pub const MESSAGE_TYPE_PRESET_SPEED_TEST: u16 = 59;
/// Wire type number of the `Updater` message (60).
pub const MESSAGE_TYPE_UPDATER: u16 = 60;
/// Wire type number of the `UpdaterForward` message (61).
pub const MESSAGE_TYPE_UPDATER_FORWARD: u16 = 61;
/// Wire type number of the `GainCalibration` message (62).
pub const MESSAGE_TYPE_GAIN_CALIBRATION: u16 = 62;
/// Wire type number of the `NeuralCapture2` message (63).
pub const MESSAGE_TYPE_NEURAL_CAPTURE2: u16 = 63;
/// Wire type number of the `Serialization` message (64).
pub const MESSAGE_TYPE_SERIALIZATION: u16 = 64;
/// Wire type number of the `TestFarm` message (65).
pub const MESSAGE_TYPE_TEST_FARM: u16 = 65;
/// Wire type number of the `ProductionTest` message (66).
pub const MESSAGE_TYPE_PRODUCTION_TEST: u16 = 66;
/// Wire type number of the `LoadAutomatedTestPreset` message (67).
pub const MESSAGE_TYPE_LOAD_AUTOMATED_TEST_PRESET: u16 = 67;
/// Wire type number of the `SetTestPresetInputOutputPorts` message (68).
pub const MESSAGE_TYPE_SET_TEST_PRESET_INPUT_OUTPUT_PORTS: u16 = 68;
/// Wire type number of the `SetTestPresetSplitMixPoints` message (69).
pub const MESSAGE_TYPE_SET_TEST_PRESET_SPLIT_MIX_POINTS: u16 = 69;
/// Wire type number of the `GenerateTestPreset` message (70).
pub const MESSAGE_TYPE_GENERATE_TEST_PRESET: u16 = 70;
/// Wire type number of the `ModelPreset` message (71).
pub const MESSAGE_TYPE_MODEL_PRESET: u16 = 71;
/// Wire type number of the `RemoteControl` message (72).
pub const MESSAGE_TYPE_REMOTE_CONTROL: u16 = 72;

/// Whether `product_id` is one of the [`SUPPORTED_PRODUCT_IDS`].
pub fn supports_product_id(product_id: u16) -> bool {
    SUPPORTED_PRODUCT_IDS.binary_search(&product_id).is_ok()
}
