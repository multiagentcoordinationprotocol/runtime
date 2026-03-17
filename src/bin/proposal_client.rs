use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{Envelope, GetSessionRequest, SendRequest, SessionStartPayload};
use macp_runtime::proposal_pb::{AcceptPayload, CounterProposalPayload, ProposalPayload};
use prost::Message;

async fn send(
    client: &mut MacpRuntimeServiceClient<tonic::transport::Channel>,
    label: &str,
    e: Envelope,
) {
    match client.send(SendRequest { envelope: Some(e) }).await {
        Ok(resp) => {
            let ack = resp.into_inner().ack.unwrap();
            let err_code = ack.error.as_ref().map(|e| e.code.as_str()).unwrap_or("");
            println!("[{label}] ok={} error='{}'", ack.ok, err_code);
        }
        Err(status) => {
            println!("[{label}] grpc error: {status}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = MacpRuntimeServiceClient::connect("http://127.0.0.1:50051").await?;

    println!("=== Proposal Mode Demo ===\n");

    // 1) SessionStart
    let start_payload = SessionStartPayload {
        intent: "negotiate price".into(),
        ttl_ms: 60000,
        participants: vec!["buyer".into(), "seller".into()],
        mode_version: String::new(),
        configuration_version: String::new(),
        policy_version: String::new(),
        context: vec![],
        roots: vec![],
    };
    send(
        &mut client,
        "session_start",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.proposal.v1".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "proposal-1".into(),
            sender: "buyer".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        },
    )
    .await;

    // 2) Seller makes a proposal
    let proposal = ProposalPayload {
        proposal_id: "offer-1".into(),
        title: "Initial offer".into(),
        summary: "1200 USD".into(),
        details: vec![],
        tags: vec![],
    };
    send(
        &mut client,
        "proposal",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.proposal.v1".into(),
            message_type: "Proposal".into(),
            message_id: "m1".into(),
            session_id: "proposal-1".into(),
            sender: "seller".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: proposal.encode_to_vec(),
        },
    )
    .await;

    // 3) Buyer counter-proposes
    let counter = CounterProposalPayload {
        proposal_id: "offer-2".into(),
        supersedes_proposal_id: "offer-1".into(),
        title: "Counter offer".into(),
        summary: "1000 USD".into(),
        details: vec![],
    };
    send(
        &mut client,
        "counter_proposal",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.proposal.v1".into(),
            message_type: "CounterProposal".into(),
            message_id: "m2".into(),
            session_id: "proposal-1".into(),
            sender: "buyer".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: counter.encode_to_vec(),
        },
    )
    .await;

    // 4) Seller accepts
    let accept = AcceptPayload {
        proposal_id: "offer-2".into(),
        reason: "agreed".into(),
    };
    send(
        &mut client,
        "accept",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.proposal.v1".into(),
            message_type: "Accept".into(),
            message_id: "m3".into(),
            session_id: "proposal-1".into(),
            sender: "seller".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: accept.encode_to_vec(),
        },
    )
    .await;

    // 5) Commitment
    let commitment = macp_runtime::pb::CommitmentPayload {
        commitment_id: "c1".into(),
        action: "proposal.accepted".into(),
        authority_scope: "commercial".into(),
        reason: "deal at 1000 USD".into(),
        mode_version: "1.0.0".into(),
        policy_version: String::new(),
        configuration_version: String::new(),
    };
    send(
        &mut client,
        "commitment",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.proposal.v1".into(),
            message_type: "Commitment".into(),
            message_id: "m4".into(),
            session_id: "proposal-1".into(),
            sender: "buyer".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: commitment.encode_to_vec(),
        },
    )
    .await;

    // 6) GetSession — verify resolved
    match client
        .get_session(GetSessionRequest {
            session_id: "proposal-1".into(),
        })
        .await
    {
        Ok(resp) => {
            let meta = resp.into_inner().metadata.unwrap();
            println!("[get_session] state={} mode={}", meta.state, meta.mode);
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}
