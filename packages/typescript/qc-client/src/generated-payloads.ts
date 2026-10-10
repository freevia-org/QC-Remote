// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.

export interface TunerSettings {
  inputPortId: 1 | 2 | 3 | 4 | 5 | 8 | 9;
  referenceOffsetHz: number;
  referenceHz: number;
  muted: boolean;
  meterEnabled?: boolean;
  meter?: number;
}

export interface GlobalBypassRows {
  row1: boolean;
  row2: boolean;
  row3: boolean;
  row4: boolean;
}

export interface MasterVolumeAssignment {
  out12: boolean;
  out34: boolean;
  send12: boolean;
  headphones: boolean;
}

export interface GeneralSettings {
  screenBrightness?: number;
  ledBrightness?: number;
  dimmedLedBrightness?: number;
  lockScreenAndVolumeKnob?: boolean;
  globalBypassCab?: GlobalBypassRows;
  globalBypassIr?: GlobalBypassRows;
  sceneBypassBehavior?: "alwaysOverwrite" | "nonstompOverwrite" | "neverOverwrite";
  midiOverUsb?: boolean;
  midiChannel?: number;
  ignoreDuplicatePc?: boolean;
  availableDiskSpace?: number;
  totalDiskSpace?: number;
  internalMidiClockEnabled?: boolean;
  masterVolumeAssignment?: MasterVolumeAssignment;
  stompModeAutoAssign?: boolean;
  swapTempoTunerAccess?: boolean;
  looperStompAssignments?: number[];
  midiClockOut?: "off" | "midiDinOnly" | "usbMidiOnly" | "bothUsbAndDinMidi";
  disableInternetConnectionCheck?: boolean;
  dynamicDelayCompensation?: boolean;
  presetDimmed?: boolean;
  sceneDimmed?: boolean;
  stompDimmed?: boolean;
  midiClockIn?: boolean;
  gigViewStompAccess?: boolean;
  holdTimingIndex?: number;
  holdTimingMs?: number;
  screenBrightnessWire?: number;
  ledBrightnessWire?: number;
  dimmedLedBrightnessWire?: number;
}

export interface InputPortSettings {
  inputPortId: number;
  level?: number;
  levelDb?: number;
  impedance?: number;
  inputType?: number;
  groundLift?: number;
  plugged?: boolean;
}

export interface OutputPortSettings {
  outputPortId: number;
  level?: number;
  levelDb?: number;
  groundLift?: number;
  muted?: boolean;
  plugged?: boolean;
}

export interface HeadphonesFeed {
  outputPortId: number;
  level: number;
}

export interface HeadphonesSettings {
  feeds: HeadphonesFeed[];
  level?: number;
  plugged?: boolean;
}

export interface UsbPortSettings {
  level?: number;
  headphonesSource?: number;
  plugged?: boolean;
  dryWet?: number;
}

export interface ExpressionPortSettings {
  expressionPortId: number;
  plugged?: boolean;
  level?: number;
  calibrating?: boolean;
}

export interface MidiPortSettings {
  thru?: number;
}

export interface IoSettings {
  inputs: InputPortSettings[];
  outputs: OutputPortSettings[];
  expressionPorts: ExpressionPortSettings[];
  headphones?: HeadphonesSettings;
  usb?: UsbPortSettings;
  midi?: MidiPortSettings;
  xlr12Linked?: boolean;
  out34Linked?: boolean;
}

export interface GlobalEqParameter {
  parameterIndex: number;
  value: number;
}

export interface GlobalEqSettings {
  parameters: GlobalEqParameter[];
  bypassed?: boolean;
}

export interface ModeCycle {
  slots: number[];
}

export interface TempoSettings {
  mode?: "PRESET" | "GLOBAL";
  bpm?: number;
  globalBpm?: number;
  ledEnabled?: boolean;
  volumeDb?: number;
  running?: boolean;
  pan?: number;
  timeSignature?: "2/4" | "3/4" | "4/4" | "5/4" | "6/4" | "7/4" | "8/4" | "9/4" | "10/4" | "11/4" | "12/4" | "13/4" | "3/8" | "6/8" | "9/8" | "12/8" | "5/8 (3+2)" | "5/8 (2+3)" | "7/8 (3+2+2)" | "7/8 (2+3+2)" | "7/8 (2+2+3)";
  subdivision?: "1/4" | "1/8" | "1/8T" | "1/16" | "1/16T" | "1/32";
  sound?: "BLIP" | "BLOCK" | "COWBELL" | "DIGITAL" | "DRUM KIT" | "SOFT KIT";
  routing?: "MULTI" | "HP" | "OUT 1/2" | "OUT 3/4" | "SEND 1/2";
  beats: ("NORMAL" | "MUTE" | "ACCENT 1" | "ACCENT 2")[];
}

export interface TempoModeSettings {
  mode: "PRESET" | "GLOBAL";
}

export interface LooperStatus {
  state?: number;
  progress?: number;
  undoProgress?: number;
  duplicateCycle?: number;
  numDuplicateCycles?: number;
  oneShotStopped?: boolean;
  redoAvailable?: boolean;
  loopLength?: number;
  freeSamples?: number;
  inReverse?: boolean;
  oneShot?: boolean;
  halfSpeed?: boolean;
  fixedDuplicateCycles?: boolean;
  armed?: boolean;
  waitingForCycle?: boolean;
  undoCount?: number;
  maxWriteDisplacement?: number;
  minWriteDisplacement?: number;
  eventsWaitingForQuantize?: number;
  currentClock?: number;
  transition?: number;
  action?: number;
  oneShotPlay?: boolean;
  syncStartWaiting?: boolean;
  quantizeEnabled?: boolean;
  updateType?: 0 | 1;
}

export interface BypassExpression {
  pedal: number;
  minimum: number;
  maximum: number;
  mode: number;
  invert: boolean;
  delayMs: number;
  latchEmulation: boolean;
}

export interface GridBlock {
  id: string;
  modelId?: number;
  categoryId?: number;
  name: string;
  kind: "input" | "utility" | "capture" | "amp" | "cab" | "mod" | "delay" | "reverb" | "output";
  category?: string;
  plugin?: boolean;
  pluginId?: string;
  row: number;
  column: number;
  bypassed?: boolean;
  bypassExpression?: BypassExpression;
  color?: string;
  glyph?: string;
  footswitch?: number;
  footswitchOrder?: number;
  footswitchAssignmentType?: "primary" | "secondary";
  catalogCategory?: string;
  bypassSceneMode?: boolean;
  bypassAddress?: "explicit" | "positional" | "mixed";
}

export interface FootswitchState {
  index: number;
  active: boolean;
  assigned: boolean;
  color: string;
  momentary?: boolean;
  label?: string;
}

export interface MidiOutMessage {
  type: number;
  channel: number;
  param1: number;
  param2: number;
  param3: number;
}

export interface MidiOutSource {
  source: number;
  messages: MidiOutMessage[];
  presetFlatOffset?: number;
}

export interface GridRoute {
  row: number;
  inputId?: number;
  outputId?: number;
  input: string;
  output: string;
  splitColumn?: number;
  mixColumn?: number;
  splitterModelId?: number;
  splitMuted: boolean;
  inputWire?: string;
  outputWire?: string;
}

export interface IoPortState {
  kind: "input" | "output" | "headphones" | "usb" | "expression";
  id: number;
  label: string;
  plugged: boolean;
}

export interface ModeSlot {
  slot: 0 | 1 | 2;
  label: string;
  mode: "PRESET" | "SCENE" | "STOMP" | "HYBRID" | "UNKNOWN";
  rawMode?: number;
}

export interface BypassUpdate {
  row: number;
  column: number;
  bypassed: boolean;
}

export interface NativeStateVersion {
  session: string;
  revision: number;
}

export interface CpuColumnLoad {
  load: number;
  core: number;
}

export interface ModelPresetSummary {
  id: string;
  name: string;
  isFactory: boolean;
  isDefault: boolean;
  modelHash?: number;
}

export interface ModelPresetCatalogMarker {
  id?: string;
  isFactory?: boolean;
  modelHash?: number;
}

export interface ModelPresetCatalogCounters {
  messages: number;
  creates: number;
  updates: number;
  renames: number;
  renameMisses: number;
  deletes: number;
  deleteMisses: number;
  wipes: number;
  readsIgnored: number;
  otherActionsIgnored: number;
  markerUpdates: number;
  ambiguousMarkers: number;
  loadedIgnored: number;
  malformedIdentities: number;
  decodeFailures: number;
  inboundLoss: number;
  duplicateTuples: number;
  ambiguousTuples: number;
  retainedEntries: number;
  retainedOtherHashEntries: number;
}

export interface ModelPresetCatalogState {
  state: "unavailable" | "folding" | "invalidated";
  session: string;
  generation: number;
  scopeHash: number;
  entries?: ModelPresetSummary[];
  activeMarker?: ModelPresetCatalogMarker;
  invalidReason?: "decode-failure" | "malformed-identity" | "inbound-loss" | "retention-limit" | "publication-limit";
  counters: ModelPresetCatalogCounters;
}

export interface BulkOperationState {
  message?: string;
  blocking?: boolean;
  cancellable?: boolean;
  progress?: number;
  finished?: boolean;
}

export interface QcStateUpdate {
  kind: "scene" | "dirty" | "master" | "sceneLabel" | "sceneColor" | "position" | "preset" | "blockRemoved" | "bypass" | "bypassBatch" | "mode" | "tempo" | "parameter" | "ioPorts" | "midiOut" | "defaultParameters" | "history" | "refresh" | "gigView" | "cpuLoad" | "catalog" | "suspend" | "bulkOperation" | "modelPresetCatalog" | "gridTopology";
  activeScene?: number;
  dirty?: boolean;
  masterVolume?: number;
  index?: number;
  label?: string;
  color?: string;
  setlistKey?: string;
  position?: number;
  isFactory?: boolean;
  presetName?: string;
  tempo?: number;
  tempoLedEnabled?: boolean;
  scenes?: string[];
  sceneColors?: string[];
  footswitchStates?: FootswitchState[];
  midiOut?: MidiOutSource[];
  presetLoadMidiOut?: MidiOutMessage[];
  blocks?: GridBlock[];
  routes?: GridRoute[];
  ioPorts?: IoPortState[];
  mode?: "PRESET" | "SCENE" | "STOMP" | "HYBRID" | "UNKNOWN";
  modeSlots?: ModeSlot[];
  footswitchModes?: ("PRESET" | "SCENE" | "STOMP")[];
  row?: number;
  column?: number;
  bypassed?: boolean;
  bypassUpdates?: BypassUpdate[];
  parameterIndex?: number;
  normalizedValue?: number;
  catalogRefresh?: boolean;
  gigViewShown?: boolean;
  cpuTotalLoad?: number;
  cpuColumns?: CpuColumnLoad[];
  undoAvailable?: boolean;
  redoAvailable?: boolean;
  refreshRequired?: boolean;
  suspended?: boolean;
  suspendReason?: string;
  bulkOperation?: BulkOperationState;
  modelPresetCatalog?: ModelPresetCatalogState;
  gridLayoutComplete?: boolean;
  defaultParameterCount?: number;
  newModelIds?: number[];
  observedAt?: number;
  presetIdentity?: string;
  stateVersion?: NativeStateVersion;
  rawMode?: number;
  rawColor?: number;
  sceneColorsArgb?: number[];
  sceneLabelsRaw?: string[];
  undoFrames?: number[];
  redoFrames?: number[];
}

export interface ScalePoint {
  normalized: number;
  real: number;
}

export interface BlockParameter {
  index: number;
  displayPosition: number;
  name: string;
  normalizedValue: number | null;
  displayValue: string;
  units: string;
  type: string;
  minimum: number;
  maximum: number;
  valueScale: string;
  scaleExponent?: number | null;
  scalePoints: ScalePoint[];
  scaleKnown: boolean;
  displayPrecision?: number | null;
  minimumLabel?: string | null;
  midpointLabel?: string | null;
  maximumLabel?: string | null;
  showAsInteger: boolean;
  hasOffPosition: boolean;
  floorDisplay: number | null;
  floorNormalized: number | null;
  steps?: number | null;
  sceneMode: boolean;
  options: string[];
  writable: boolean;
  enabled: boolean;
  expressionAssignable: boolean;
  linkedSceneMode?: number | null;
  expression?: number | null;
  expressionMinimum?: number | null;
  expressionMaximum?: number | null;
  ledValue?: number | null;
  wireValueKind: string;
}

export interface BlockDetails {
  row: number;
  column: number;
  modelId: number;
  name: string;
  category: string;
  scene: number;
  bypassExpression?: BypassExpression;
  parameters: BlockParameter[];
}

export interface PresetSnapshot {
  deviceName: string;
  presetName: string;
  presetLocation: string;
  presetPosition: number;
  setlistKey: string;
  setlistName: string;
  mode: "PRESET" | "SCENE" | "STOMP" | "HYBRID" | "UNKNOWN";
  modeSlots?: ModeSlot[];
  footswitchModes?: ("PRESET" | "SCENE" | "STOMP")[];
  activeScene: number;
  scenes: string[];
  sceneColors?: string[];
  footswitchStates?: FootswitchState[];
  midiOut?: MidiOutSource[];
  presetLoadMidiOut?: MidiOutMessage[];
  blocks: GridBlock[];
  routes: GridRoute[];
  ioPorts?: IoPortState[];
  tempo: number;
  tempoLedEnabled?: boolean;
  tempoPulseEpochMs?: number;
  masterVolume: number;
  dirty: boolean;
  presetIdentity?: string;
  stateVersion?: NativeStateVersion;
  rawMode?: number;
  sceneColorsArgb?: number[];
  sceneLabelsRaw?: string[];
  unobserved?: ("mode" | "tempo" | "tempoLedEnabled" | "masterVolume" | "dirty" | "presetLocation")[];
  gridLayoutComplete?: boolean;
  modelPresetCatalog?: ModelPresetCatalogState;
}

export interface PresetRestorationResult {
  status: "not_required" | "completed" | "skipped" | "failed";
  detail?: string;
}

export interface PluginBlockReadback {
  plugin: boolean;
  bypassed?: boolean;
  bypassRequested: boolean;
  note?: string;
}

export interface DeviceActionResult {
  detail: string;
  accepted?: boolean;
  verified?: boolean;
  verification?: "authoritative_readback" | "authoritative_reconnect_readback" | "authoritative_preflight" | "accepted_unverified";
  changed?: boolean;
  readback?: Record<string, unknown>;
  snapshot?: PresetSnapshot;
  setlistKey?: string;
  savedName?: string;
  restoration?: PresetRestorationResult;
  pluginBlockReadback?: PluginBlockReadback;
  observation?: Record<string, unknown>;
}
