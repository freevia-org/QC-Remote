// Compile the exact renderer used by the Windows broker, without a second
// implementation or a build of the entire broker just to export its artwork.
#[path = "../../../services/device-broker/src/tray/icon.rs"]
mod icon;

fn main() -> std::io::Result<()> {
    let output = std::path::PathBuf::from(std::env::args_os().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    for (name, tone) in [
        ("ready", icon::IconTone::Ready),
        ("attention", icon::IconTone::Attention),
        ("error", icon::IconTone::Error),
        ("inactive", icon::IconTone::Inactive),
    ] {
        std::fs::write(
            output.join(format!("{name}.rgba")),
            icon::tone_rgba(tone, 0),
        )?;
    }
    for frame in 0..icon::SYNC_FRAME_COUNT {
        std::fs::write(
            output.join(format!("syncing-{frame}.rgba")),
            icon::tone_rgba(icon::IconTone::Syncing, frame),
        )?;
    }
    Ok(())
}
