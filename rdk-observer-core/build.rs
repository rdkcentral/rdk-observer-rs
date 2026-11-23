fn main() -> std::io::Result<()> {
    let schema = "proto/observation.proto";

    println!("cargo::rerun-if-changed={schema}");
    println!("cargo::rerun-if-env-changed=PROTOC");
    println!("cargo::rerun-if-env-changed=PROTOC_INCLUDE");

    prost_build::compile_protos(&[schema], &["proto"])
}
