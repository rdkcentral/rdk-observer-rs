fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/observation.proto");
    println!("cargo:rerun-if-env-changed=PROTOC");

    let protoc = match std::env::var_os("PROTOC") {
        Some(path) => std::path::PathBuf::from(path),
        None => protoc_bin_vendored::protoc_bin_path()?,
    };
    prost_build::Config::new()
        .protoc_executable(protoc)
        .compile_protos(&["proto/observation.proto"], &["proto"])?;
    Ok(())
}
