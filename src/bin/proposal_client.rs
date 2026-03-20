#[path = "support/common.rs"]
mod common;

use common::{
    canonical_commitment_payload, canonical_start_payload, envelope, get_session_as, print_ack,
    send_as,
};
use macp_runtime::proposal_pb::{AcceptPayload, CounterProposalPayload, ProposalPayload};
use prost::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = common::connect_client().await?;
    let session_id = common::new_session_id();

    println!("=== Proposal Mode Demo ===\n");

    let ack = send_as(
        &mut client,
        "buyer",
        envelope(
            "macp.mode.proposal.v1",
            "SessionStart",
            "m0",
            &session_id,
            "buyer",
            canonical_start_payload("negotiate price", &["buyer", "seller"], 60_000),
        ),
    )
    .await?;
    print_ack("session_start", &ack);

    let proposal = ProposalPayload {
        proposal_id: "offer-1".into(),
        title: "Initial offer".into(),
        summary: "1200 USD".into(),
        details: vec![],
        tags: vec![],
    };
    let ack = send_as(
        &mut client,
        "seller",
        envelope(
            "macp.mode.proposal.v1",
            "Proposal",
            "m1",
            &session_id,
            "seller",
            proposal.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("proposal", &ack);

    let counter = CounterProposalPayload {
        proposal_id: "offer-2".into(),
        supersedes_proposal_id: "offer-1".into(),
        title: "Counter offer".into(),
        summary: "1000 USD".into(),
        details: vec![],
    };
    let ack = send_as(
        &mut client,
        "buyer",
        envelope(
            "macp.mode.proposal.v1",
            "CounterProposal",
            "m2",
            &session_id,
            "buyer",
            counter.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("counter_proposal", &ack);

    let accept = AcceptPayload {
        proposal_id: "offer-2".into(),
        reason: "agreed".into(),
    };
    let ack = send_as(
        &mut client,
        "buyer",
        envelope(
            "macp.mode.proposal.v1",
            "Accept",
            "m3",
            &session_id,
            "buyer",
            accept.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("buyer_accept", &ack);

    let accept = AcceptPayload {
        proposal_id: "offer-2".into(),
        reason: "confirmed".into(),
    };
    let ack = send_as(
        &mut client,
        "seller",
        envelope(
            "macp.mode.proposal.v1",
            "Accept",
            "m4",
            &session_id,
            "seller",
            accept.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("seller_accept", &ack);

    let ack = send_as(
        &mut client,
        "buyer",
        envelope(
            "macp.mode.proposal.v1",
            "Commitment",
            "m5",
            &session_id,
            "buyer",
            canonical_commitment_payload(
                "c1",
                "proposal.accepted",
                "commercial",
                "all required participants accepted offer-2",
            ),
        ),
    )
    .await?;
    print_ack("commitment", &ack);

    let session = get_session_as(&mut client, "seller", &session_id).await?;
    let meta = session.metadata.expect("metadata");
    println!("[get_session] state={} mode={}", meta.state, meta.mode);

    Ok(())
}
