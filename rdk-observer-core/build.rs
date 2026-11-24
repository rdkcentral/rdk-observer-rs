use std::path::PathBuf;

////////////////////////////////////////////////////////////////////////////////
fn main() -> anyhow::Result<()> {
    compile_proto_buf()?;
    build_ebpf_binary()?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
fn compile_proto_buf() -> std::io::Result<()> {
    let schema = "proto/observation.proto";

    println!("cargo::rerun-if-changed={schema}");
    println!("cargo::rerun-if-env-changed=PROTOC");
    println!("cargo::rerun-if-env-changed=PROTOC_INCLUDE");

    prost_build::compile_protos(&[schema], &["proto"])
}

////////////////////////////////////////////////////////////////////////////////
fn build_ebpf_binary() -> anyhow::Result<()> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let root_dir = manifest_dir.join("../rdk-observer-ebpf").canonicalize()?;
    let root_dir = root_dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path: {}", root_dir.display()))?;

    let package = aya_build::Package {
        name: "rdk-observer-ebpf",
        root_dir,
        ..Default::default()
    };
    aya_build::build_ebpf([package], aya_build::Toolchain::default())
}
