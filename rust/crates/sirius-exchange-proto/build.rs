fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = std::path::Path::new("../../../proto");
    let schema = proto_root.join("sirius/exchange/v1/exchange.proto");
    println!("cargo:rerun-if-changed={}", schema.display());
    let descriptors = protox::compile([schema], [proto_root])?;
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}
