// Shared synthetic QC input for the real Windows and Android workflow adapters.
// This is deterministic protocol-shaped test data, not a captured device session.
use prost::Message;
use quadcortex::device::owner::SessionRuntime;
use quadcortex::device::transport::IngressTime;
use quadcortex::{
    profile,
    proto::{binary_preset, cortex_protobuf_v2 as pa, BinaryPreset},
};
use serde_json::{json, Value};

const ORIGINAL: &str = "/media/p4/Presets/Original";
const SOURCE: &str = "/media/p4/Presets/Source";
const DESTINATION: &str = "/media/p4/Presets/Destination";

/// How long after the frame the unit's own File publication arrives, in the host's clock.
const PUSH_DELAY_MS: u64 = 40;

pub struct Device {
    caller_identity: String,
    copy: bool,
    sent: bool,
    sent_at: u64,
    pushed: bool,
    pub actions: Vec<&'static str>,
}
impl Device {
    pub fn new(copy: bool, owner: &mut SessionRuntime) -> Self {
        navigate(owner, ORIGINAL, 1, "Original", 0);
        // Startup publishes once. Preparation must never need another READ.
        folder(owner, ORIGINAL, &[(1, "Original")], 0);
        folder(owner, SOURCE, &[(4, "Source")], 0);
        folder(owner, DESTINATION, &[], 0);
        Self {
            caller_identity: owner.caller_preset_identity().unwrap(),
            copy,
            sent: false,
            sent_at: 0,
            pushed: false,
            actions: vec![],
        }
    }
    /// Copy is one File COPY frame; Save is one File CREATE frame. Neither recalls anything
    /// and neither is followed by a directory READ: verification is passive first, so the
    /// unit's own File publication (see [`Device::tick`]) confirms the write well inside
    /// the passive window.
    pub fn command_count(&self) -> usize {
        1
    }

    /// The unit's File publication after the frame: pushed once, [`PUSH_DELAY_MS`] after the
    /// frame was handled, and only then. A host calls this as its clock advances.
    pub fn tick(&mut self, owner: &mut SessionRuntime, now: u64) {
        if !self.sent || self.pushed || now < self.sent_at.saturating_add(PUSH_DELAY_MS) {
            return;
        }
        self.pushed = true;
        let mut original = vec![(1, "Original")];
        if !self.copy {
            original.push((2, "Saved"));
        }
        folder(owner, ORIGINAL, &original, now);
        folder(owner, SOURCE, &[(4, "Source")], now);
        folder(
            owner,
            DESTINATION,
            if self.copy { &[(7, "Source")] } else { &[] },
            now,
        );
    }
    pub fn method(&self) -> &'static str {
        if self.copy {
            "device.copyPreset"
        } else {
            "device.savePresetAs"
        }
    }
    pub fn params(&self) -> Value {
        if self.copy {
            // Copy acts on no active preset: it carries no expected-state inputs.
            json!({"sourceSetlistKey":SOURCE,"sourcePosition":4,"sourceName":"Source",
                   "destinationSetlistKey":DESTINATION,"destinationPosition":7,
                   "confirmOverwrite":true})
        } else {
            json!({"setlistKey":ORIGINAL,"position":2,"name":"Saved",
                   "expectedPresetName":"Original","expectedPresetIdentity":self.caller_identity,"expectedPosition":1,"confirmOverwrite":false})
        }
    }
    pub fn respond(&mut self, owner: &mut SessionRuntime, kind: u16, payload: &[u8], now: u64) {
        assert_ne!(
            kind,
            profile::MESSAGE_TYPE_SETLIST_POSITION,
            "neither Save nor the direct File COPY frame may navigate"
        );
        assert_eq!(
            kind,
            profile::MESSAGE_TYPE_FILE,
            "unexpected workflow message: {}",
            kind
        );
        let message = pa::FileMessage::decode(payload).unwrap();
        if message.action == pa::message_action::Enum::Read as i32 {
            panic!("no directory READ is expected: the startup catalog suffices for preparation and the verification is passive inside its window");
        } else if self.copy {
            assert_eq!(message.action, pa::message_action::Enum::Copy as i32);
            assert!(!self.sent, "persistent COPY must never replay");
            let pa::file_message::Folder::Folder(source) = message.folder.unwrap();
            let pa::folder_info::Key::Key(source_key) = source.key.unwrap();
            assert_eq!(source_key, SOURCE);
            assert_eq!(source.files.len(), 1);
            let pa::product_data::Key::Key(product_key) = source.files[0].key.clone().unwrap();
            assert_eq!(product_key, format!("{SOURCE}/4"));
            let pa::file_message::ToFolder::ToFolder(destination) = message.to_folder.unwrap();
            let pa::folder_info::Key::Key(destination_key) = destination.key.unwrap();
            assert_eq!(destination_key, DESTINATION);
            assert_eq!(destination.files.len(), 1);
            let pa::product_data::Index::Index(index) = destination.files[0].index.unwrap();
            assert_eq!(index, 7);
            // The active preset is untouched by the frame.
            assert_eq!(owner.state.snapshot().preset_name, "Original");
            self.actions.push("copy");
            self.sent = true;
            self.sent_at = now;
        } else {
            assert_eq!(message.action, pa::message_action::Enum::Create as i32);
            assert!(!self.sent, "persistent Save must never replay");
            let pa::file_message::Folder::Folder(folder) = message.folder.unwrap();
            let pa::folder_info::Key::Key(key) = folder.key.unwrap();
            assert_eq!(folder.files.len(), 1);
            let file = &folder.files[0];
            let position = match file.index.as_ref().unwrap() {
                pa::product_data::Index::Index(index) => *index,
            };
            let name = match file.name.as_ref().unwrap() {
                pa::product_data::Name::Name(name) => name.as_str(),
            };
            assert_eq!((key.as_str(), position, name), (ORIGINAL, 2, "Saved"));
            assert_eq!(owner.state.snapshot().preset_name, "Original");
            self.actions.push("save");
            self.sent = true;
            self.sent_at = now;
            // Save selects its destination.
            navigate(owner, ORIGINAL, 2, "Saved", now);
        }
    }
    pub fn assert_result(&self, result: &Value, owner: &SessionRuntime) {
        assert_eq!(
            self.actions,
            if self.copy {
                vec!["copy"]
            } else {
                vec!["save"]
            }
        );
        assert_eq!(result["verified"], true, "{result}");
        assert_eq!(
            result["savedName"],
            if self.copy { "Source" } else { "Saved" }
        );
        assert_eq!(
            result["setlistKey"],
            if self.copy { DESTINATION } else { ORIGINAL }
        );
        // Neither operation has anything to restore.
        assert_eq!(result["restoration"]["status"], "not_required");
        assert!(result.get("changed").is_none());
        assert_eq!(owner.state.snapshot().setlist_key, ORIGINAL);
        assert_eq!(
            owner.state.snapshot().preset_position,
            if self.copy { 1 } else { 2 }
        );
        quadcortex::device::generated_gateway::validate_result(self.method(), result).unwrap();
    }
}
fn publish(owner: &mut SessionRuntime, kind: u16, payload: Vec<u8>, now: u64) {
    let ticket = owner
        .capture_observation_with_ingress(
            1,
            Some(IngressTime {
                first_report_at_ms: now,
                last_report_at_ms: now,
            }),
        )
        .unwrap();
    owner
        .observe_state(&ticket, kind, &payload)
        .unwrap()
        .unwrap();
}
fn navigate(owner: &mut SessionRuntime, key: &str, position: u32, name: &str, now: u64) {
    publish(
        owner,
        profile::MESSAGE_TYPE_SETLIST_POSITION,
        pa::SetlistPositionMessage {
            action: pa::message_action::Enum::Update as i32,
            folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
                key.into(),
            )),
            position: Some(pa::setlist_position_message::Position::Position(position)),
            ..Default::default()
        }
        .encode_to_vec(),
        now,
    );
    publish(
        owner,
        profile::MESSAGE_TYPE_RECALL_PRESET,
        pa::RecallPresetMessage {
            action: pa::message_action::Enum::Update as i32,
            preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
                name: Some(binary_preset::Name::Name(name.into())),
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec(),
        now,
    );
}
fn folder(owner: &mut SessionRuntime, key: &str, entries: &[(i32, &str)], now: u64) {
    publish(
        owner,
        profile::MESSAGE_TYPE_FILE,
        pa::FileMessage {
            action: pa::message_action::Enum::Create as i32,
            folder: Some(pa::file_message::Folder::Folder(pa::FolderInfo {
                key: Some(pa::folder_info::Key::Key(key.into())),
                name: Some(pa::folder_info::Name::Name(
                    key.rsplit('/').next().unwrap().into(),
                )),
                files: (0..256)
                    .map(|index| {
                        let name = entries
                            .iter()
                            .find(|(position, _)| *position == index)
                            .map(|(_, name)| *name);
                        pa::ProductData {
                            index: Some(pa::product_data::Index::Index(index)),
                            name: name.map(|name| pa::product_data::Name::Name(name.into())),
                            key: name.map(|_| pa::product_data::Key::Key(format!("{key}/{index}"))),
                            instrument: Some(pa::product_data::Instrument::Instrument(0)),
                            ..Default::default()
                        }
                    })
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        }
        .encode_to_vec(),
        now,
    );
}
