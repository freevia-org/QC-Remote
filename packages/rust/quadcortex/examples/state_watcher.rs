//! Follow a unit's state as messages arrive, and print each update as JSON.
//!
//! A host feeds every assembled message to a [`DeviceStateRuntime`]. It returns
//! the normalized updates the message produced and keeps a snapshot of the
//! current state. This example builds a small, synthetic message stream (a model
//! catalog, a preset with two blocks, a setlist position, then some live
//! changes), feeds it in, and prints what comes out. Nothing is read from or
//! sent to a device, and the messages are built here, not captured.
//!
//! Run with `cargo run -p quadcortex --example state_watcher`.

use prost::Message;
use quadcortex::device::state_runtime::DeviceStateRuntime;
use quadcortex::protocol::profile::{
    MESSAGE_TYPE_MASTER_VOLUME, MESSAGE_TYPE_PRESET_DIRTY, MESSAGE_TYPE_RECALL_PRESET,
    MESSAGE_TYPE_SCENE, MESSAGE_TYPE_SETLIST_POSITION,
};
use quadcortex::protocol::proto::{
    binary_preset, chain, cortex_protobuf_v2 as pa, model, BinaryPreset, Chain, Model,
};
use quadcortex::state::parse_model_repo;

const UPDATE: i32 = pa::message_action::Enum::Update as i32;

/// A block of model `id` in `column` of its chain.
fn block(id: u32, column: u32) -> Model {
    Model {
        hash: Some(model::Hash::Hash(id)),
        column: Some(model::Column::Column(column)),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut runtime = DeviceStateRuntime::new();

    // The model catalog gives blocks their names. `ingest` refuses it on
    // purpose: parsing is expensive, so a host parses it off the receive path
    // and installs the result.
    let catalog = pa::ModelRepoMessage {
        action: UPDATE,
        model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
            br#"<Models>
                  <Category name="Drive"><Model id="3" name="Boost" /></Category>
                  <Category name="Delay"><Model id="7" name="Echo" /></Category>
                </Models>"#
                .to_vec(),
        )),
        ..Default::default()
    }
    .encode_to_vec();
    runtime.install_model_catalog(parse_model_repo(&catalog)?);
    println!("catalog installed: {} model(s)", runtime.model_count());

    // A preset with two blocks on row 0, at 100 BPM.
    let preset = pa::RecallPresetMessage {
        action: UPDATE,
        preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
            name: Some(binary_preset::Name::Name("Watch Me".into())),
            tempo: Some(binary_preset::Tempo::Tempo(100)),
            chains: vec![Chain {
                row: Some(chain::Row::Row(0)),
                models: vec![block(3, 1), block(7, 4)],
                ..Default::default()
            }],
            ..Default::default()
        })),
        ..Default::default()
    }
    .encode_to_vec();
    let position = pa::SetlistPositionMessage {
        action: UPDATE,
        folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
            "/media/p4/Presets/Example Setlist".into(),
        )),
        position: Some(pa::setlist_position_message::Position::Position(17)),
        ..Default::default()
    }
    .encode_to_vec();
    let scene = pa::SceneMessage {
        action: UPDATE,
        selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(2)),
        ..Default::default()
    }
    .encode_to_vec();
    let dirty = pa::PresetDirtyMessage {
        action: UPDATE,
        is_dirty: true,
        ..Default::default()
    }
    .encode_to_vec();
    let volume = pa::MasterVolumeMessage {
        action: UPDATE,
        volume: Some(pa::master_volume_message::Volume::Volume(0.42)),
        ..Default::default()
    }
    .encode_to_vec();

    let stream: [(&str, u16, &[u8]); 5] = [
        ("preset recalled", MESSAGE_TYPE_RECALL_PRESET, &preset),
        ("setlist position", MESSAGE_TYPE_SETLIST_POSITION, &position),
        ("scene selected", MESSAGE_TYPE_SCENE, &scene),
        ("edited since load", MESSAGE_TYPE_PRESET_DIRTY, &dirty),
        ("master volume", MESSAGE_TYPE_MASTER_VOLUME, &volume),
    ];

    for (label, message_type, payload) in stream {
        let observation = runtime.ingest(message_type, payload)?;
        println!("\n{label}: {} update(s)", observation.states.len());
        for update in &observation.states {
            // The camelCase JSON contract; empty fields are omitted. The preset
            // update is long, so show only the start of it.
            let json = serde_json::to_string(update)?;
            let mut shown: String = json.chars().take(150).collect();
            if json.chars().count() > 150 {
                shown.push_str("...");
            }
            println!("  {shown}");
        }
    }

    let snapshot = runtime.snapshot();
    println!(
        "\nsnapshot: {:?} at {} ({}), scene {}, {} BPM, volume {}, edited: {}",
        snapshot.preset_name,
        snapshot.preset_location,
        snapshot.setlist_name,
        snapshot.active_scene,
        snapshot.tempo,
        snapshot.master_volume,
        snapshot.dirty,
    );
    for block in &snapshot.blocks {
        println!(
            "  block {} at row {}, column {}",
            block.name, block.row, block.column
        );
    }
    Ok(())
}
