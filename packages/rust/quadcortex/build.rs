fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc is available");
    std::env::set_var("PROTOC", protoc);
    let mut config = prost_build::Config::new();
    config.include_file("quadcortex.rs");
    config
        .compile_protos(
            &["proto/Preset.proto", "proto/ProductionAutomation.proto"],
            &["proto"],
        )
        .expect("Quad Cortex protobuf schemas compile");
    println!("cargo:rerun-if-changed=proto/Preset.proto");
    println!("cargo:rerun-if-changed=proto/ProductionAutomation.proto");
}
