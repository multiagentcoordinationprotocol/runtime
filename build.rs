fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .compile(&["macp/v1/macp.proto"], &["proto"])?;
    Ok(())
}
