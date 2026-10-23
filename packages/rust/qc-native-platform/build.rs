use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=src/app_store.m");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || env::var_os("CARGO_FEATURE_MACOS_APP_STORE").is_none()
    {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let arch = if target.starts_with("aarch64-") {
        "arm64"
    } else if target.starts_with("x86_64-") {
        "x86_64"
    } else {
        panic!("Unsupported Mac App Store target: {target}");
    };
    assert!(Command::new("xcrun")
        .args([
            "clang",
            "-fobjc-arc",
            "-fblocks",
            "-mmacosx-version-min=13.0",
            "-arch",
            arch,
            "-c",
            "src/app_store.m",
            "-o"
        ])
        .arg(out.join("app_store.o"))
        .status()
        .unwrap()
        .success());
    assert!(Command::new("xcrun")
        .arg("ar")
        .arg("crs")
        .arg(out.join("libqc_app_store.a"))
        .arg(out.join("app_store.o"))
        .status()
        .unwrap()
        .success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=qc_app_store");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=ServiceManagement");
}
