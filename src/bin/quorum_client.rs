#[path = "support/common.rs"]
mod common;

use common::{
    canonical_commitment_payload, canonical_start_payload, envelope, get_session_as, print_ack,
    send_as,
};
use macp_runtime::quorum_pb::{ApprovalRequestPayload, ApprovePayload};
use prost::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = common::connect_client().await?;

    println!("=== Quorum Mode Demo ===\n");

    let ack = send_as(
        &mut client,
        "coordinator",
        envelope(
            "macp.mode.quorum.v1",
            "SessionStart",
            "m0",
            "quorum-demo-1",
            "coordinator",
            canonical_start_payload(
                "approve production deploy",
                &["alice", "bob", "carol"],
                60_000,
            ),
        ),
    )
    .await?;
    print_ack("session_start", &ack);

    let request = ApprovalRequestPayload {
        request_id: "deploy-v2".into(),
        action: "deploy.production".into(),
        summary: "Deploy v2.1 to production".into(),
        details: vec![],
        required_approvals: 2,
    };
    let ack = send_as(
        &mut client,
        "coordinator",
        envelope(
            "macp.mode.quorum.v1",
            "ApprovalRequest",
            "m1",
            "quorum-demo-1",
            "coordinator",
            request.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("approval_request", &ack);

    let approve = ApprovePayload {
        request_id: "deploy-v2".into(),
        reason: "tests pass".into(),
    };
    let ack = send_as(
        &mut client,
        "alice",
        envelope(
            "macp.mode.quorum.v1",
            "Approve",
            "m2",
            "quorum-demo-1",
            "alice",
            approve.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("alice_approve", &ack);

    let approve = ApprovePayload {
        request_id: "deploy-v2".into(),
        reason: "staging looks good".into(),
    };
    let ack = send_as(
        &mut client,
        "bob",
        envelope(
            "macp.mode.quorum.v1",
            "Approve",
            "m3",
            "quorum-demo-1",
            "bob",
            approve.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("bob_approve", &ack);

    let ack = send_as(
        &mut client,
        "coordinator",
        envelope(
            "macp.mode.quorum.v1",
            "Commitment",
            "m4",
            "quorum-demo-1",
            "coordinator",
            canonical_commitment_payload(
                "c1",
                "quorum.approved",
                "release-management",
                "required approvals reached for deploy-v2",
            ),
        ),
    )
    .await?;
    print_ack("commitment", &ack);

    let session = get_session_as(&mut client, "carol", "quorum-demo-1").await?;
    let meta = session.metadata.expect("metadata");
    println!("[get_session] state={} mode={}", meta.state, meta.mode);

    Ok(())
}
