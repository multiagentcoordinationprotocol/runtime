fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir =
        std::env::var("DEP_MACP_PROTO_PROTO_DIR").expect("macp-proto crate must set proto_dir");
    tonic_prost_build::configure()
        .build_server(true)
        .compile_protos(
            &[
                "macp/v1/envelope.proto",
                "macp/v1/core.proto",
                "macp/modes/decision/v1/decision.proto",
                "macp/modes/proposal/v1/proposal.proto",
                "macp/modes/task/v1/task.proto",
                "macp/modes/handoff/v1/handoff.proto",
                "macp/modes/quorum/v1/quorum.proto",
            ],
            &[&proto_dir],
        )?;
    Ok(())
}
