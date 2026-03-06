fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_server(true).compile(
        &[
            "macp/v1/envelope.proto",
            "macp/v1/core.proto",
            "macp/modes/decision/v1/decision.proto",
        ],
        &["proto"],
    )?;
    Ok(())
}
