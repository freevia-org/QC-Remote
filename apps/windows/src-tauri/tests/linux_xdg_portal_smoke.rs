#![cfg(target_os = "linux")]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

const OPEN_FILE: &str = "open-fixture.txt";
const SAVE_FILE: &str = "saved-by-portal.txt";

fn smoke_dir() -> PathBuf {
    PathBuf::from(
        env::var_os("QC_XDG_PORTAL_SMOKE_DIR")
            .expect("run this ignored test with scripts/run_linux_xdg_portal_smoke.py"),
    )
}

fn publish_stage(dir: &Path, stage: &str) {
    fs::write(dir.join("stage"), stage).expect("publish portal smoke stage");
}

#[test]
#[ignore = "interactive Linux XDG portal smoke; run through the supervised workflow helper"]
fn xdg_portal_open_save_cancel_smoke() {
    let dir = smoke_dir();
    let open_path = dir.join(OPEN_FILE);
    assert_eq!(
        fs::read(&open_path).expect("read open fixture"),
        b"qc-remote-portal-open-fixture\n"
    );

    publish_stage(&dir, "open-dialog");
    let selected = rfd::FileDialog::new()
        .set_title("QC Remote Portal Open Smoke")
        .set_directory(&dir)
        .pick_file();
    let selected = selected.expect("open portal should return the selected fixture");
    assert_eq!(
        selected
            .canonicalize()
            .expect("canonicalize open selection"),
        open_path.canonicalize().expect("canonicalize open fixture")
    );

    publish_stage(&dir, "save-dialog");
    let save_path = rfd::FileDialog::new()
        .set_title("QC Remote Portal Save Smoke")
        .set_directory(&dir)
        .set_file_name(SAVE_FILE)
        .save_file()
        .expect("save portal should return a destination");
    assert_eq!(save_path, dir.join(SAVE_FILE));
    fs::write(&save_path, b"qc-remote-portal-save-result\n").expect("write selected destination");

    publish_stage(&dir, "cancel-dialog");
    let cancelled = rfd::FileDialog::new()
        .set_title("QC Remote Portal Cancel Smoke")
        .set_directory(&dir)
        .pick_file();
    assert!(
        cancelled.is_none(),
        "Escape should cancel the portal request"
    );

    publish_stage(&dir, "complete");
}
