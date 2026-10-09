//! Walk the whole staged startup, from the first handshake message to a
//! synchronized session, against replies this example makes up.
//!
//! [`SessionDriver`] decides what to send at each stage; the host only writes
//! each identified batch and reports that the write was accepted. Here the "host"
//! is a loop that prints each batch, and the "device" is a set of synthetic
//! replies built from the generated protobuf types. Nothing is sent to a device,
//! and the replies are not captured traffic: they show the library's startup
//! order and what it waits for at each stage, not how any firmware behaves.
//!
//! Run with `cargo run -p quadcortex --example startup_walkthrough`.

use prost::Message;
use quadcortex::protocol::message_registry::message_name;
use quadcortex::protocol::proto::{binary_preset, cortex_protobuf_v2 as pa, BinaryPreset};
use quadcortex::session::{DispatchOutcome, SessionDriver};

const SESSION_ID: &str = "walkthrough-session";
const UPDATE: i32 = pa::message_action::Enum::Update as i32;

/// Pull every batch the driver has ready and "write" it, printing each one.
fn write_ready_batches(driver: &mut SessionDriver, now_ms: u64) {
    while let Some(batch) = driver.next_batch(now_ms, 1_700_000_000_000) {
        let names: Vec<String> = batch
            .messages
            .iter()
            .map(|message| {
                format!(
                    "{} ({})",
                    message.message_type,
                    message_name(message.message_type).unwrap_or("?")
                )
            })
            .collect();
        println!(
            "  t={now_ms:>3} ms  send batch: {}   [phase {}]",
            names.join(", "),
            driver.phase().as_str()
        );
        // A real host writes each message's reports here, one at a time, and
        // checks `driver.is_current(&batch.ticket)` between messages.
        driver.dispatched(&batch.ticket, DispatchOutcome::Accepted);
    }
}

/// Offer one synthetic reply to the driver.
fn reply(driver: &mut SessionDriver, now_ms: u64, message_type: u16, payload: Vec<u8>) {
    println!(
        "  t={now_ms:>3} ms  device sends {} ({})",
        message_type,
        message_name(message_type).unwrap_or("?")
    );
    driver.observe(message_type, &payload, now_ms);
}

fn main() {
    let mut driver = SessionDriver::start(1, SESSION_ID.into(), 0);

    println!("handshake");
    write_ready_batches(&mut driver, 0);
    let echo = pa::ResetCommsBuffersMessage {
        session_id: Some(pa::reset_comms_buffers_message::SessionId::SessionId(
            SESSION_ID.into(),
        )),
        ..Default::default()
    };
    reply(&mut driver, 10, 52, echo.encode_to_vec());

    println!("version check");
    write_ready_batches(&mut driver, 10);
    let version = pa::VersionMessage {
        action: UPDATE,
        device_type: Some(pa::version_message::DeviceTypeOneOf::DeviceType(
            pa::version_message::DeviceType::Qc as i32,
        )),
        cortex_control_version_valid: Some(
            pa::version_message::CortexControlVersionValid::CortexControlVersionValid(true),
        ),
        ..Default::default()
    };
    reply(&mut driver, 20, 10, version.encode_to_vec());

    println!("disconnect, then build (the driver waits for the model catalog)");
    write_ready_batches(&mut driver, 20);
    let catalog = pa::ModelRepoMessage {
        action: UPDATE,
        model_repo_payload: Some(pa::model_repo_message::ModelRepoPayload::ModelRepoPayload(
            br#"<Models><Category name="Drive"><Model id="1" name="Test" /></Category></Models>"#
                .to_vec(),
        )),
        ..Default::default()
    };
    reply(&mut driver, 30, 51, catalog.encode_to_vec());

    println!("initialize (waits for module statistics)");
    write_ready_batches(&mut driver, 30);
    let stats = pa::ModuleStatsMessage {
        action: UPDATE,
        stats: vec![pa::ModuleStatsItem::default()],
        ..Default::default()
    };
    reply(&mut driver, 40, 35, stats.encode_to_vec());

    println!("boot burst (the driver waits for the updater message)");
    write_ready_batches(&mut driver, 40);
    // While booting, the unit publishes the state the library needs to see
    // before it calls the session synchronized. These are made up here.
    let preset = pa::RecallPresetMessage {
        action: UPDATE,
        preset: Some(pa::recall_preset_message::Preset::Preset(BinaryPreset {
            name: Some(binary_preset::Name::Name("Walkthrough".into())),
            ..Default::default()
        })),
        ..Default::default()
    };
    let position = pa::SetlistPositionMessage {
        action: UPDATE,
        folder_key: Some(pa::setlist_position_message::FolderKey::FolderKey(
            "/media/p4/Presets/Example Setlist".into(),
        )),
        position: Some(pa::setlist_position_message::Position::Position(0)),
        ..Default::default()
    };
    let scene = pa::SceneMessage {
        action: UPDATE,
        selected_scene: Some(pa::scene_message::SelectedScene::SelectedScene(0)),
        ..Default::default()
    };
    let mode = pa::ModeMessage {
        action: UPDATE,
        mode: Some(pa::mode_message::Mode::Mode(2)),
        ..Default::default()
    };
    let volume = pa::MasterVolumeMessage {
        action: UPDATE,
        volume: Some(pa::master_volume_message::Volume::Volume(0.5)),
        ..Default::default()
    };
    let dirty = pa::PresetDirtyMessage {
        action: UPDATE,
        is_dirty: false,
        ..Default::default()
    };
    reply(&mut driver, 41, 15, preset.encode_to_vec());
    reply(&mut driver, 42, 2, position.encode_to_vec());
    reply(&mut driver, 43, 13, scene.encode_to_vec());
    reply(&mut driver, 44, 14, mode.encode_to_vec());
    reply(&mut driver, 45, 17, volume.encode_to_vec());
    reply(&mut driver, 46, 34, dirty.encode_to_vec());
    let updater = pa::UpdaterMessage {
        action: UPDATE,
        ..Default::default()
    };
    reply(&mut driver, 50, 60, updater.encode_to_vec());

    println!("seed (system time first, then any seed reads still missing)");
    write_ready_batches(&mut driver, 50);
    // `next_batch` also advances the seed's timers; one more pull settles it.
    write_ready_batches(&mut driver, 60);

    println!("\nphase: {}", driver.phase().as_str());
    println!("connected: {}", driver.is_connected());
    println!("synchronization: {:?}", driver.synchronization());
    println!("failure: {:?}", driver.failure());
}
