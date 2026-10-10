// The complete CortexMessageType table for the Quad Cortex wire protocol.
// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
//
// Every type the device declares is decodable and encodable here, so the
// transport can round-trip anything on the link. Whether the application
// exposes a type is a separate decision; see
// references/public/cortex-protocol/message-type-plan.json.

use crate::proto::cortex_protobuf_v2 as pa;
use prost::Message;

/// Every `CortexMessageType` value the shipped binary declares, excluding
/// the `Undefined` and `NumberOfMessageTypes` sentinels.
pub const MESSAGE_TYPES: [(u16, &str); 72] = [
    (1, "Grid"),
    (2, "SetlistPosition"),
    (3, "IOSettings"),
    (4, "File"),
    (5, "IOMeter"),
    (6, "Tuner"),
    (7, "Diagnostics"),
    (8, "MIDISettings"),
    (9, "GeneralSettings"),
    (10, "Version"),
    (11, "ProductionAutomationMode"),
    (12, "GridMove"),
    (13, "Scene"),
    (14, "Mode"),
    (15, "RecallPreset"),
    (16, "EnableCaptureOut"),
    (17, "MasterVolume"),
    (18, "CloudLogin"),
    (19, "DefaultParameters"),
    (20, "RecentsFavorites"),
    (21, "UndoRedo"),
    (22, "SceneCopy"),
    (23, "SceneLabel"),
    (24, "ShowGigView"),
    (25, "Screenshot"),
    (26, "CPULoad"),
    (27, "ShowTuner"),
    (28, "Looper"),
    (29, "ProductForward"),
    (30, "BackupsForward"),
    (31, "LogsForward"),
    (32, "KeepAlive"),
    (33, "GlobalTempo"),
    (34, "PresetDirty"),
    (35, "ModuleStats"),
    (36, "NeuralCapture"),
    (37, "GridModelMeter"),
    (38, "GlobalEQ"),
    (39, "RecentSearches"),
    (40, "LocalBackup"),
    (41, "CloudBackup"),
    (42, "CompilerInhibitedModules"),
    (43, "SystemTimeSync"),
    (44, "Logs"),
    (45, "ProcessDownloadsQueue"),
    (46, "CloudProduct"),
    (47, "Confirmation"),
    (48, "SceneColor"),
    (49, "Connection"),
    (50, "NewModels"),
    (51, "ModelRepo"),
    (52, "ResetCommsBuffers"),
    (53, "SuspendConnection"),
    (54, "PinnedModels"),
    (55, "GigViewButton"),
    (56, "GenericError"),
    (57, "BulkOperation"),
    (58, "License"),
    (59, "PresetSpeedTest"),
    (60, "Updater"),
    (61, "UpdaterForward"),
    (62, "GainCalibration"),
    (63, "NeuralCapture2"),
    (64, "Serialization"),
    (65, "TestFarm"),
    (66, "ProductionTest"),
    (67, "LoadAutomatedTestPreset"),
    (68, "SetTestPresetInputOutputPorts"),
    (69, "SetTestPresetSplitMixPoints"),
    (70, "GenerateTestPreset"),
    (71, "ModelPreset"),
    (72, "RemoteControl"),
];

/// One decoded message of any declared type.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedMessage {
    /// Message type 1 (`Grid`), carrying a [`pa::GridMessage`].
    Grid(Box<pa::GridMessage>),
    /// Message type 2 (`SetlistPosition`), carrying a [`pa::SetlistPositionMessage`].
    SetlistPosition(Box<pa::SetlistPositionMessage>),
    /// Message type 3 (`IOSettings`), carrying a [`pa::IoSettingsMessage`].
    IoSettings(Box<pa::IoSettingsMessage>),
    /// Message type 4 (`File`), carrying a [`pa::FileMessage`].
    File(Box<pa::FileMessage>),
    /// Message type 5 (`IOMeter`), carrying a [`pa::IoMeterMessage`].
    IoMeter(Box<pa::IoMeterMessage>),
    /// Message type 6 (`Tuner`), carrying a [`pa::TunerMessage`].
    Tuner(Box<pa::TunerMessage>),
    /// Message type 7 (`Diagnostics`), carrying a [`pa::DiagnosticsMessage`].
    Diagnostics(Box<pa::DiagnosticsMessage>),
    /// Message type 8 (`MIDISettings`), carrying a [`pa::MidiSettingsMessage`].
    MidiSettings(Box<pa::MidiSettingsMessage>),
    /// Message type 9 (`GeneralSettings`), carrying a [`pa::GeneralSettingsMessage`].
    GeneralSettings(Box<pa::GeneralSettingsMessage>),
    /// Message type 10 (`Version`), carrying a [`pa::VersionMessage`].
    Version(Box<pa::VersionMessage>),
    /// Message type 11 (`ProductionAutomationMode`), carrying a [`pa::ProductionAutomationModeMessage`].
    ProductionAutomationMode(Box<pa::ProductionAutomationModeMessage>),
    /// Message type 12 (`GridMove`), carrying a [`pa::GridMoveMessage`].
    GridMove(Box<pa::GridMoveMessage>),
    /// Message type 13 (`Scene`), carrying a [`pa::SceneMessage`].
    Scene(Box<pa::SceneMessage>),
    /// Message type 14 (`Mode`), carrying a [`pa::ModeMessage`].
    Mode(Box<pa::ModeMessage>),
    /// Message type 15 (`RecallPreset`), carrying a [`pa::RecallPresetMessage`].
    RecallPreset(Box<pa::RecallPresetMessage>),
    /// Message type 16 (`EnableCaptureOut`), carrying a [`pa::EnableCaptureOutMessage`].
    EnableCaptureOut(Box<pa::EnableCaptureOutMessage>),
    /// Message type 17 (`MasterVolume`), carrying a [`pa::MasterVolumeMessage`].
    MasterVolume(Box<pa::MasterVolumeMessage>),
    /// Message type 18 (`CloudLogin`), carrying a [`pa::CloudLoginMessage`].
    CloudLogin(Box<pa::CloudLoginMessage>),
    /// Message type 19 (`DefaultParameters`), carrying a [`pa::DefaultParametersMessage`].
    DefaultParameters(Box<pa::DefaultParametersMessage>),
    /// Message type 20 (`RecentsFavorites`), carrying a [`pa::RecentsFavoritesMessage`].
    RecentsFavorites(Box<pa::RecentsFavoritesMessage>),
    /// Message type 21 (`UndoRedo`), carrying a [`pa::UndoRedoMessage`].
    UndoRedo(Box<pa::UndoRedoMessage>),
    /// Message type 22 (`SceneCopy`), carrying a [`pa::SceneCopyMessage`].
    SceneCopy(Box<pa::SceneCopyMessage>),
    /// Message type 23 (`SceneLabel`), carrying a [`pa::SceneLabelMessage`].
    SceneLabel(Box<pa::SceneLabelMessage>),
    /// Message type 24 (`ShowGigView`), carrying a [`pa::ShowGigViewMessage`].
    ShowGigView(Box<pa::ShowGigViewMessage>),
    /// Message type 25 (`Screenshot`), carrying a [`pa::ScreenshotMessage`].
    Screenshot(Box<pa::ScreenshotMessage>),
    /// Message type 26 (`CPULoad`), carrying a [`pa::CpuLoadMessage`].
    CpuLoad(Box<pa::CpuLoadMessage>),
    /// Message type 27 (`ShowTuner`), carrying a [`pa::ShowTunerMessage`].
    ShowTuner(Box<pa::ShowTunerMessage>),
    /// Message type 28 (`Looper`), carrying a [`pa::LooperMessage`].
    Looper(Box<pa::LooperMessage>),
    /// Message type 29 (`ProductForward`), carrying a [`pa::ProductForwardMessage`].
    ProductForward(Box<pa::ProductForwardMessage>),
    /// Message type 30 (`BackupsForward`), carrying a [`pa::BackupsForwardMessage`].
    BackupsForward(Box<pa::BackupsForwardMessage>),
    /// Message type 31 (`LogsForward`), carrying a [`pa::LogsForwardMessage`].
    LogsForward(Box<pa::LogsForwardMessage>),
    /// Message type 32 (`KeepAlive`), carrying a [`pa::KeepAliveMessage`].
    KeepAlive(Box<pa::KeepAliveMessage>),
    /// Message type 33 (`GlobalTempo`), carrying a [`pa::GlobalTempoMessage`].
    GlobalTempo(Box<pa::GlobalTempoMessage>),
    /// Message type 34 (`PresetDirty`), carrying a [`pa::PresetDirtyMessage`].
    PresetDirty(Box<pa::PresetDirtyMessage>),
    /// Message type 35 (`ModuleStats`), carrying a [`pa::ModuleStatsMessage`].
    ModuleStats(Box<pa::ModuleStatsMessage>),
    /// Message type 36 (`NeuralCapture`), carrying a [`pa::NeuralCaptureMessage`].
    NeuralCapture(Box<pa::NeuralCaptureMessage>),
    /// Message type 37 (`GridModelMeter`), carrying a [`pa::GridModelMeterMessage`].
    GridModelMeter(Box<pa::GridModelMeterMessage>),
    /// Message type 38 (`GlobalEQ`), carrying a [`pa::GlobalEqMessage`].
    GlobalEq(Box<pa::GlobalEqMessage>),
    /// Message type 39 (`RecentSearches`), carrying a [`pa::RecentSearchesMessage`].
    RecentSearches(Box<pa::RecentSearchesMessage>),
    /// Message type 40 (`LocalBackup`), carrying a [`pa::LocalBackupMessage`].
    LocalBackup(Box<pa::LocalBackupMessage>),
    /// Message type 41 (`CloudBackup`), carrying a [`pa::CloudBackupMessage`].
    CloudBackup(Box<pa::CloudBackupMessage>),
    /// Message type 42 (`CompilerInhibitedModules`), carrying a [`pa::CompilerInhibitedModulesMessage`].
    CompilerInhibitedModules(Box<pa::CompilerInhibitedModulesMessage>),
    /// Message type 43 (`SystemTimeSync`), carrying a [`pa::SystemTimeSyncMessage`].
    SystemTimeSync(Box<pa::SystemTimeSyncMessage>),
    /// Message type 44 (`Logs`), carrying a [`pa::LogsMessage`].
    Logs(Box<pa::LogsMessage>),
    /// Message type 45 (`ProcessDownloadsQueue`), carrying a [`pa::ProcessDownloadsQueueMessage`].
    ProcessDownloadsQueue(Box<pa::ProcessDownloadsQueueMessage>),
    /// Message type 46 (`CloudProduct`), carrying a [`pa::CloudProductMessage`].
    CloudProduct(Box<pa::CloudProductMessage>),
    /// Message type 47 (`Confirmation`), carrying a [`pa::ConfirmationMessage`].
    Confirmation(Box<pa::ConfirmationMessage>),
    /// Message type 48 (`SceneColor`), carrying a [`pa::SceneColorMessage`].
    SceneColor(Box<pa::SceneColorMessage>),
    /// Message type 49 (`Connection`), carrying a [`pa::ConnectionMessage`].
    Connection(Box<pa::ConnectionMessage>),
    /// Message type 50 (`NewModels`), carrying a [`pa::NewModelsMessage`].
    NewModels(Box<pa::NewModelsMessage>),
    /// Message type 51 (`ModelRepo`), carrying a [`pa::ModelRepoMessage`].
    ModelRepo(Box<pa::ModelRepoMessage>),
    /// Message type 52 (`ResetCommsBuffers`), carrying a [`pa::ResetCommsBuffersMessage`].
    ResetCommsBuffers(Box<pa::ResetCommsBuffersMessage>),
    /// Message type 53 (`SuspendConnection`), carrying a [`pa::SuspendConnectionMessage`].
    SuspendConnection(Box<pa::SuspendConnectionMessage>),
    /// Message type 54 (`PinnedModels`), carrying a [`pa::PinnedModelsMessage`].
    PinnedModels(Box<pa::PinnedModelsMessage>),
    /// Message type 55 (`GigViewButton`), carrying a [`pa::GigViewButtonMessage`].
    GigViewButton(Box<pa::GigViewButtonMessage>),
    /// Message type 56 (`GenericError`), carrying a [`pa::GenericErrorMessage`].
    GenericError(Box<pa::GenericErrorMessage>),
    /// Message type 57 (`BulkOperation`), carrying a [`pa::BulkOperationMessage`].
    BulkOperation(Box<pa::BulkOperationMessage>),
    /// Message type 58 (`License`), carrying a [`pa::LicenseMessage`].
    License(Box<pa::LicenseMessage>),
    /// Message type 59 (`PresetSpeedTest`), carrying a [`pa::PresetSpeedTestMessage`].
    PresetSpeedTest(Box<pa::PresetSpeedTestMessage>),
    /// Message type 60 (`Updater`), carrying a [`pa::UpdaterMessage`].
    Updater(Box<pa::UpdaterMessage>),
    /// Message type 61 (`UpdaterForward`), carrying a [`pa::UpdaterForwardMessage`].
    UpdaterForward(Box<pa::UpdaterForwardMessage>),
    /// Message type 62 (`GainCalibration`), carrying a [`pa::GainCalibrationMessage`].
    GainCalibration(Box<pa::GainCalibrationMessage>),
    /// Message type 63 (`NeuralCapture2`), carrying a [`pa::NeuralCapture2Message`].
    NeuralCapture2(Box<pa::NeuralCapture2Message>),
    /// Message type 64 (`Serialization`), carrying a [`pa::SerializationMessage`].
    Serialization(Box<pa::SerializationMessage>),
    /// Message type 65 (`TestFarm`), carrying a [`pa::TestFarmMessage`].
    TestFarm(Box<pa::TestFarmMessage>),
    /// Message type 66 (`ProductionTest`), carrying a [`pa::ProductionTestMessage`].
    ProductionTest(Box<pa::ProductionTestMessage>),
    /// Message type 67 (`LoadAutomatedTestPreset`), carrying a [`pa::LoadAutomatedTestPresetMessage`].
    LoadAutomatedTestPreset(Box<pa::LoadAutomatedTestPresetMessage>),
    /// Message type 68 (`SetTestPresetInputOutputPorts`), carrying a [`pa::SetTestPresetInputOutputPortsMessage`].
    SetTestPresetInputOutputPorts(Box<pa::SetTestPresetInputOutputPortsMessage>),
    /// Message type 69 (`SetTestPresetSplitMixPoints`), carrying a [`pa::SetTestPresetSplitMixPointsMessage`].
    SetTestPresetSplitMixPoints(Box<pa::SetTestPresetSplitMixPointsMessage>),
    /// Message type 70 (`GenerateTestPreset`), carrying a [`pa::GenerateTestPresetMessage`].
    GenerateTestPreset(Box<pa::GenerateTestPresetMessage>),
    /// Message type 71 (`ModelPreset`), carrying a [`pa::ModelPresetMessage`].
    ModelPreset(Box<pa::ModelPresetMessage>),
    /// Message type 72 (`RemoteControl`), carrying a [`pa::RemoteControlMessage`].
    RemoteControl(Box<pa::RemoteControlMessage>),
}

impl DecodedMessage {
    /// The wire type number this message is sent under.
    pub fn message_type(&self) -> u16 {
        match self {
            Self::Grid(_) => 1,
            Self::SetlistPosition(_) => 2,
            Self::IoSettings(_) => 3,
            Self::File(_) => 4,
            Self::IoMeter(_) => 5,
            Self::Tuner(_) => 6,
            Self::Diagnostics(_) => 7,
            Self::MidiSettings(_) => 8,
            Self::GeneralSettings(_) => 9,
            Self::Version(_) => 10,
            Self::ProductionAutomationMode(_) => 11,
            Self::GridMove(_) => 12,
            Self::Scene(_) => 13,
            Self::Mode(_) => 14,
            Self::RecallPreset(_) => 15,
            Self::EnableCaptureOut(_) => 16,
            Self::MasterVolume(_) => 17,
            Self::CloudLogin(_) => 18,
            Self::DefaultParameters(_) => 19,
            Self::RecentsFavorites(_) => 20,
            Self::UndoRedo(_) => 21,
            Self::SceneCopy(_) => 22,
            Self::SceneLabel(_) => 23,
            Self::ShowGigView(_) => 24,
            Self::Screenshot(_) => 25,
            Self::CpuLoad(_) => 26,
            Self::ShowTuner(_) => 27,
            Self::Looper(_) => 28,
            Self::ProductForward(_) => 29,
            Self::BackupsForward(_) => 30,
            Self::LogsForward(_) => 31,
            Self::KeepAlive(_) => 32,
            Self::GlobalTempo(_) => 33,
            Self::PresetDirty(_) => 34,
            Self::ModuleStats(_) => 35,
            Self::NeuralCapture(_) => 36,
            Self::GridModelMeter(_) => 37,
            Self::GlobalEq(_) => 38,
            Self::RecentSearches(_) => 39,
            Self::LocalBackup(_) => 40,
            Self::CloudBackup(_) => 41,
            Self::CompilerInhibitedModules(_) => 42,
            Self::SystemTimeSync(_) => 43,
            Self::Logs(_) => 44,
            Self::ProcessDownloadsQueue(_) => 45,
            Self::CloudProduct(_) => 46,
            Self::Confirmation(_) => 47,
            Self::SceneColor(_) => 48,
            Self::Connection(_) => 49,
            Self::NewModels(_) => 50,
            Self::ModelRepo(_) => 51,
            Self::ResetCommsBuffers(_) => 52,
            Self::SuspendConnection(_) => 53,
            Self::PinnedModels(_) => 54,
            Self::GigViewButton(_) => 55,
            Self::GenericError(_) => 56,
            Self::BulkOperation(_) => 57,
            Self::License(_) => 58,
            Self::PresetSpeedTest(_) => 59,
            Self::Updater(_) => 60,
            Self::UpdaterForward(_) => 61,
            Self::GainCalibration(_) => 62,
            Self::NeuralCapture2(_) => 63,
            Self::Serialization(_) => 64,
            Self::TestFarm(_) => 65,
            Self::ProductionTest(_) => 66,
            Self::LoadAutomatedTestPreset(_) => 67,
            Self::SetTestPresetInputOutputPorts(_) => 68,
            Self::SetTestPresetSplitMixPoints(_) => 69,
            Self::GenerateTestPreset(_) => 70,
            Self::ModelPreset(_) => 71,
            Self::RemoteControl(_) => 72,
        }
    }

    /// The protobuf body, ready for the framing layer.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Grid(message) => message.encode_to_vec(),
            Self::SetlistPosition(message) => message.encode_to_vec(),
            Self::IoSettings(message) => message.encode_to_vec(),
            Self::File(message) => message.encode_to_vec(),
            Self::IoMeter(message) => message.encode_to_vec(),
            Self::Tuner(message) => message.encode_to_vec(),
            Self::Diagnostics(message) => message.encode_to_vec(),
            Self::MidiSettings(message) => message.encode_to_vec(),
            Self::GeneralSettings(message) => message.encode_to_vec(),
            Self::Version(message) => message.encode_to_vec(),
            Self::ProductionAutomationMode(message) => message.encode_to_vec(),
            Self::GridMove(message) => message.encode_to_vec(),
            Self::Scene(message) => message.encode_to_vec(),
            Self::Mode(message) => message.encode_to_vec(),
            Self::RecallPreset(message) => message.encode_to_vec(),
            Self::EnableCaptureOut(message) => message.encode_to_vec(),
            Self::MasterVolume(message) => message.encode_to_vec(),
            Self::CloudLogin(message) => message.encode_to_vec(),
            Self::DefaultParameters(message) => message.encode_to_vec(),
            Self::RecentsFavorites(message) => message.encode_to_vec(),
            Self::UndoRedo(message) => message.encode_to_vec(),
            Self::SceneCopy(message) => message.encode_to_vec(),
            Self::SceneLabel(message) => message.encode_to_vec(),
            Self::ShowGigView(message) => message.encode_to_vec(),
            Self::Screenshot(message) => message.encode_to_vec(),
            Self::CpuLoad(message) => message.encode_to_vec(),
            Self::ShowTuner(message) => message.encode_to_vec(),
            Self::Looper(message) => message.encode_to_vec(),
            Self::ProductForward(message) => message.encode_to_vec(),
            Self::BackupsForward(message) => message.encode_to_vec(),
            Self::LogsForward(message) => message.encode_to_vec(),
            Self::KeepAlive(message) => message.encode_to_vec(),
            Self::GlobalTempo(message) => message.encode_to_vec(),
            Self::PresetDirty(message) => message.encode_to_vec(),
            Self::ModuleStats(message) => message.encode_to_vec(),
            Self::NeuralCapture(message) => message.encode_to_vec(),
            Self::GridModelMeter(message) => message.encode_to_vec(),
            Self::GlobalEq(message) => message.encode_to_vec(),
            Self::RecentSearches(message) => message.encode_to_vec(),
            Self::LocalBackup(message) => message.encode_to_vec(),
            Self::CloudBackup(message) => message.encode_to_vec(),
            Self::CompilerInhibitedModules(message) => message.encode_to_vec(),
            Self::SystemTimeSync(message) => message.encode_to_vec(),
            Self::Logs(message) => message.encode_to_vec(),
            Self::ProcessDownloadsQueue(message) => message.encode_to_vec(),
            Self::CloudProduct(message) => message.encode_to_vec(),
            Self::Confirmation(message) => message.encode_to_vec(),
            Self::SceneColor(message) => message.encode_to_vec(),
            Self::Connection(message) => message.encode_to_vec(),
            Self::NewModels(message) => message.encode_to_vec(),
            Self::ModelRepo(message) => message.encode_to_vec(),
            Self::ResetCommsBuffers(message) => message.encode_to_vec(),
            Self::SuspendConnection(message) => message.encode_to_vec(),
            Self::PinnedModels(message) => message.encode_to_vec(),
            Self::GigViewButton(message) => message.encode_to_vec(),
            Self::GenericError(message) => message.encode_to_vec(),
            Self::BulkOperation(message) => message.encode_to_vec(),
            Self::License(message) => message.encode_to_vec(),
            Self::PresetSpeedTest(message) => message.encode_to_vec(),
            Self::Updater(message) => message.encode_to_vec(),
            Self::UpdaterForward(message) => message.encode_to_vec(),
            Self::GainCalibration(message) => message.encode_to_vec(),
            Self::NeuralCapture2(message) => message.encode_to_vec(),
            Self::Serialization(message) => message.encode_to_vec(),
            Self::TestFarm(message) => message.encode_to_vec(),
            Self::ProductionTest(message) => message.encode_to_vec(),
            Self::LoadAutomatedTestPreset(message) => message.encode_to_vec(),
            Self::SetTestPresetInputOutputPorts(message) => message.encode_to_vec(),
            Self::SetTestPresetSplitMixPoints(message) => message.encode_to_vec(),
            Self::GenerateTestPreset(message) => message.encode_to_vec(),
            Self::ModelPreset(message) => message.encode_to_vec(),
            Self::RemoteControl(message) => message.encode_to_vec(),
        }
    }
}

/// The declared name of a message type, for logging and diagnostics.
pub fn message_name(message_type: u16) -> Option<&'static str> {
    MESSAGE_TYPES
        .iter()
        .find(|(number, _)| *number == message_type)
        .map(|(_, name)| *name)
}

/// Decode any declared type. `None` for a type the binary does not
/// declare, which is how an unknown push is told apart from a corrupt one.
pub fn decode_any(
    message_type: u16,
    payload: &[u8],
) -> Option<Result<DecodedMessage, prost::DecodeError>> {
    let decoded =
        match message_type {
            1 => pa::GridMessage::decode(payload)
                .map(|message| DecodedMessage::Grid(Box::new(message))),
            2 => pa::SetlistPositionMessage::decode(payload)
                .map(|message| DecodedMessage::SetlistPosition(Box::new(message))),
            3 => pa::IoSettingsMessage::decode(payload)
                .map(|message| DecodedMessage::IoSettings(Box::new(message))),
            4 => pa::FileMessage::decode(payload)
                .map(|message| DecodedMessage::File(Box::new(message))),
            5 => pa::IoMeterMessage::decode(payload)
                .map(|message| DecodedMessage::IoMeter(Box::new(message))),
            6 => pa::TunerMessage::decode(payload)
                .map(|message| DecodedMessage::Tuner(Box::new(message))),
            7 => pa::DiagnosticsMessage::decode(payload)
                .map(|message| DecodedMessage::Diagnostics(Box::new(message))),
            8 => pa::MidiSettingsMessage::decode(payload)
                .map(|message| DecodedMessage::MidiSettings(Box::new(message))),
            9 => pa::GeneralSettingsMessage::decode(payload)
                .map(|message| DecodedMessage::GeneralSettings(Box::new(message))),
            10 => pa::VersionMessage::decode(payload)
                .map(|message| DecodedMessage::Version(Box::new(message))),
            11 => pa::ProductionAutomationModeMessage::decode(payload)
                .map(|message| DecodedMessage::ProductionAutomationMode(Box::new(message))),
            12 => pa::GridMoveMessage::decode(payload)
                .map(|message| DecodedMessage::GridMove(Box::new(message))),
            13 => pa::SceneMessage::decode(payload)
                .map(|message| DecodedMessage::Scene(Box::new(message))),
            14 => pa::ModeMessage::decode(payload)
                .map(|message| DecodedMessage::Mode(Box::new(message))),
            15 => pa::RecallPresetMessage::decode(payload)
                .map(|message| DecodedMessage::RecallPreset(Box::new(message))),
            16 => pa::EnableCaptureOutMessage::decode(payload)
                .map(|message| DecodedMessage::EnableCaptureOut(Box::new(message))),
            17 => pa::MasterVolumeMessage::decode(payload)
                .map(|message| DecodedMessage::MasterVolume(Box::new(message))),
            18 => pa::CloudLoginMessage::decode(payload)
                .map(|message| DecodedMessage::CloudLogin(Box::new(message))),
            19 => pa::DefaultParametersMessage::decode(payload)
                .map(|message| DecodedMessage::DefaultParameters(Box::new(message))),
            20 => pa::RecentsFavoritesMessage::decode(payload)
                .map(|message| DecodedMessage::RecentsFavorites(Box::new(message))),
            21 => pa::UndoRedoMessage::decode(payload)
                .map(|message| DecodedMessage::UndoRedo(Box::new(message))),
            22 => pa::SceneCopyMessage::decode(payload)
                .map(|message| DecodedMessage::SceneCopy(Box::new(message))),
            23 => pa::SceneLabelMessage::decode(payload)
                .map(|message| DecodedMessage::SceneLabel(Box::new(message))),
            24 => pa::ShowGigViewMessage::decode(payload)
                .map(|message| DecodedMessage::ShowGigView(Box::new(message))),
            25 => pa::ScreenshotMessage::decode(payload)
                .map(|message| DecodedMessage::Screenshot(Box::new(message))),
            26 => pa::CpuLoadMessage::decode(payload)
                .map(|message| DecodedMessage::CpuLoad(Box::new(message))),
            27 => pa::ShowTunerMessage::decode(payload)
                .map(|message| DecodedMessage::ShowTuner(Box::new(message))),
            28 => pa::LooperMessage::decode(payload)
                .map(|message| DecodedMessage::Looper(Box::new(message))),
            29 => pa::ProductForwardMessage::decode(payload)
                .map(|message| DecodedMessage::ProductForward(Box::new(message))),
            30 => pa::BackupsForwardMessage::decode(payload)
                .map(|message| DecodedMessage::BackupsForward(Box::new(message))),
            31 => pa::LogsForwardMessage::decode(payload)
                .map(|message| DecodedMessage::LogsForward(Box::new(message))),
            32 => pa::KeepAliveMessage::decode(payload)
                .map(|message| DecodedMessage::KeepAlive(Box::new(message))),
            33 => pa::GlobalTempoMessage::decode(payload)
                .map(|message| DecodedMessage::GlobalTempo(Box::new(message))),
            34 => pa::PresetDirtyMessage::decode(payload)
                .map(|message| DecodedMessage::PresetDirty(Box::new(message))),
            35 => pa::ModuleStatsMessage::decode(payload)
                .map(|message| DecodedMessage::ModuleStats(Box::new(message))),
            36 => pa::NeuralCaptureMessage::decode(payload)
                .map(|message| DecodedMessage::NeuralCapture(Box::new(message))),
            37 => pa::GridModelMeterMessage::decode(payload)
                .map(|message| DecodedMessage::GridModelMeter(Box::new(message))),
            38 => pa::GlobalEqMessage::decode(payload)
                .map(|message| DecodedMessage::GlobalEq(Box::new(message))),
            39 => pa::RecentSearchesMessage::decode(payload)
                .map(|message| DecodedMessage::RecentSearches(Box::new(message))),
            40 => pa::LocalBackupMessage::decode(payload)
                .map(|message| DecodedMessage::LocalBackup(Box::new(message))),
            41 => pa::CloudBackupMessage::decode(payload)
                .map(|message| DecodedMessage::CloudBackup(Box::new(message))),
            42 => pa::CompilerInhibitedModulesMessage::decode(payload)
                .map(|message| DecodedMessage::CompilerInhibitedModules(Box::new(message))),
            43 => pa::SystemTimeSyncMessage::decode(payload)
                .map(|message| DecodedMessage::SystemTimeSync(Box::new(message))),
            44 => pa::LogsMessage::decode(payload)
                .map(|message| DecodedMessage::Logs(Box::new(message))),
            45 => pa::ProcessDownloadsQueueMessage::decode(payload)
                .map(|message| DecodedMessage::ProcessDownloadsQueue(Box::new(message))),
            46 => pa::CloudProductMessage::decode(payload)
                .map(|message| DecodedMessage::CloudProduct(Box::new(message))),
            47 => pa::ConfirmationMessage::decode(payload)
                .map(|message| DecodedMessage::Confirmation(Box::new(message))),
            48 => pa::SceneColorMessage::decode(payload)
                .map(|message| DecodedMessage::SceneColor(Box::new(message))),
            49 => pa::ConnectionMessage::decode(payload)
                .map(|message| DecodedMessage::Connection(Box::new(message))),
            50 => pa::NewModelsMessage::decode(payload)
                .map(|message| DecodedMessage::NewModels(Box::new(message))),
            51 => pa::ModelRepoMessage::decode(payload)
                .map(|message| DecodedMessage::ModelRepo(Box::new(message))),
            52 => pa::ResetCommsBuffersMessage::decode(payload)
                .map(|message| DecodedMessage::ResetCommsBuffers(Box::new(message))),
            53 => pa::SuspendConnectionMessage::decode(payload)
                .map(|message| DecodedMessage::SuspendConnection(Box::new(message))),
            54 => pa::PinnedModelsMessage::decode(payload)
                .map(|message| DecodedMessage::PinnedModels(Box::new(message))),
            55 => pa::GigViewButtonMessage::decode(payload)
                .map(|message| DecodedMessage::GigViewButton(Box::new(message))),
            56 => pa::GenericErrorMessage::decode(payload)
                .map(|message| DecodedMessage::GenericError(Box::new(message))),
            57 => pa::BulkOperationMessage::decode(payload)
                .map(|message| DecodedMessage::BulkOperation(Box::new(message))),
            58 => pa::LicenseMessage::decode(payload)
                .map(|message| DecodedMessage::License(Box::new(message))),
            59 => pa::PresetSpeedTestMessage::decode(payload)
                .map(|message| DecodedMessage::PresetSpeedTest(Box::new(message))),
            60 => pa::UpdaterMessage::decode(payload)
                .map(|message| DecodedMessage::Updater(Box::new(message))),
            61 => pa::UpdaterForwardMessage::decode(payload)
                .map(|message| DecodedMessage::UpdaterForward(Box::new(message))),
            62 => pa::GainCalibrationMessage::decode(payload)
                .map(|message| DecodedMessage::GainCalibration(Box::new(message))),
            63 => pa::NeuralCapture2Message::decode(payload)
                .map(|message| DecodedMessage::NeuralCapture2(Box::new(message))),
            64 => pa::SerializationMessage::decode(payload)
                .map(|message| DecodedMessage::Serialization(Box::new(message))),
            65 => pa::TestFarmMessage::decode(payload)
                .map(|message| DecodedMessage::TestFarm(Box::new(message))),
            66 => pa::ProductionTestMessage::decode(payload)
                .map(|message| DecodedMessage::ProductionTest(Box::new(message))),
            67 => pa::LoadAutomatedTestPresetMessage::decode(payload)
                .map(|message| DecodedMessage::LoadAutomatedTestPreset(Box::new(message))),
            68 => pa::SetTestPresetInputOutputPortsMessage::decode(payload)
                .map(|message| DecodedMessage::SetTestPresetInputOutputPorts(Box::new(message))),
            69 => pa::SetTestPresetSplitMixPointsMessage::decode(payload)
                .map(|message| DecodedMessage::SetTestPresetSplitMixPoints(Box::new(message))),
            70 => pa::GenerateTestPresetMessage::decode(payload)
                .map(|message| DecodedMessage::GenerateTestPreset(Box::new(message))),
            71 => pa::ModelPresetMessage::decode(payload)
                .map(|message| DecodedMessage::ModelPreset(Box::new(message))),
            72 => pa::RemoteControlMessage::decode(payload)
                .map(|message| DecodedMessage::RemoteControl(Box::new(message))),
            _ => return None,
        };
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_type_round_trips() {
        for (number, name) in MESSAGE_TYPES {
            let decoded = decode_any(number, &[])
                .unwrap_or_else(|| panic!("{number} {name} is not decodable"))
                .unwrap_or_else(|error| panic!("{number} {name}: {error}"));
            assert_eq!(decoded.message_type(), number, "{name}");
            assert!(decoded.encode().is_empty(), "{name} encodes an empty body");
            assert_eq!(message_name(number), Some(name));
        }
    }

    #[test]
    fn an_undeclared_type_is_reported_rather_than_guessed() {
        assert!(decode_any(0, &[]).is_none());
        assert!(decode_any(73, &[]).is_none());
        assert!(message_name(73).is_none());
    }
}
