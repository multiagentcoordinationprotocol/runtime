#[path = "support/common.rs"]
mod common;

use common::{envelope, get_session_as, print_ack, send_as};
use macp_runtime::pb::SessionStartPayload;
use prost::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = common::connect_client().await?;
    let session_id = common::new_session_id();

    println!("=== Multi-Round Convergence Demo ===\n");

    let start_payload = SessionStartPayload {
        intent: "convergence test".into(),
        ttl_ms: 60_000,
        participants: vec!["alice".into(), "bob".into()],
        mode_version: "experimental".into(),
        configuration_version: "legacy".into(),
        policy_version: String::new(),
        context: vec![],
        roots: vec![],
    };
    let ack = send_as(
        &mut client,
        "coordinator",
        envelope(
            "macp.mode.multi_round.v1",
            "SessionStart",
            "m0",
            &session_id,
            "coordinator",
            start_payload.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("session_start", &ack);

    let ack = send_as(
        &mut client,
        "alice",
        envelope(
            "macp.mode.multi_round.v1",
            "Contribute",
            "m1",
            &session_id,
            "alice",
            br#"{"value":"option_a"}"#.to_vec(),
        ),
    )
    .await?;
    print_ack("alice_contributes", &ack);

    let ack = send_as(
        &mut client,
        "bob",
        envelope(
            "macp.mode.multi_round.v1",
            "Contribute",
            "m2",
            &session_id,
            "bob",
            br#"{"value":"option_b"}"#.to_vec(),
        ),
    )
    .await?;
    print_ack("bob_contributes_b", &ack);

    let session = get_session_as(&mut client, "alice", &session_id).await?;
    let meta = session.metadata.expect("metadata");
    println!("[get_session] state={} mode={}", meta.state, meta.mode);

    let ack = send_as(
        &mut client,
        "bob",
        envelope(
            "macp.mode.multi_round.v1",
            "Contribute",
            "m3",
            &session_id,
            "bob",
            br#"{"value":"option_a"}"#.to_vec(),
        ),
    )
    .await?;
    print_ack("bob_revises", &ack);

    let session = get_session_as(&mut client, "alice", &session_id).await?;
    let meta = session.metadata.expect("metadata");
    println!("[get_session] state={} mode={}", meta.state, meta.mode);

    Ok(())
}
