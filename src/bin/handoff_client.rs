use macp_runtime::handoff_pb::{HandoffAcceptPayload, HandoffContextPayload, HandoffOfferPayload};
use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{Envelope, GetSessionRequest, SendRequest, SessionStartPayload};
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

    println!("=== Handoff Mode Demo ===\n");

    // 1) SessionStart
    let start_payload = SessionStartPayload {
        intent: "escalate support ticket".into(),
        ttl_ms: 60000,
        participants: vec!["owner".into(), "target".into()],
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
            mode: "macp.mode.handoff.v1".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "handoff-1".into(),
            sender: "owner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        },
    )
    .await;

    // 2) HandoffOffer
    let offer = HandoffOfferPayload {
        handoff_id: "h1".into(),
        target_participant: "target".into(),
        scope: "customer-support".into(),
        reason: "need specialist help".into(),
    };
    send(
        &mut client,
        "handoff_offer",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.handoff.v1".into(),
            message_type: "HandoffOffer".into(),
            message_id: "m1".into(),
            session_id: "handoff-1".into(),
            sender: "owner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: offer.encode_to_vec(),
        },
    )
    .await;

    // 3) HandoffContext
    let context = HandoffContextPayload {
        handoff_id: "h1".into(),
        content_type: "text/plain".into(),
        context: b"Customer issue: billing discrepancy on invoice #1234".to_vec(),
    };
    send(
        &mut client,
        "handoff_context",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.handoff.v1".into(),
            message_type: "HandoffContext".into(),
            message_id: "m2".into(),
            session_id: "handoff-1".into(),
            sender: "owner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: context.encode_to_vec(),
        },
    )
    .await;

    // 4) HandoffAccept
    let accept = HandoffAcceptPayload {
        handoff_id: "h1".into(),
        accepted_by: "target".into(),
        reason: "ready to assist".into(),
    };
    send(
        &mut client,
        "handoff_accept",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.handoff.v1".into(),
            message_type: "HandoffAccept".into(),
            message_id: "m3".into(),
            session_id: "handoff-1".into(),
            sender: "target".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: accept.encode_to_vec(),
        },
    )
    .await;

    // 5) Commitment
    let commitment = macp_runtime::pb::CommitmentPayload {
        commitment_id: "c1".into(),
        action: "handoff.accepted".into(),
        authority_scope: "support".into(),
        reason: "handoff complete".into(),
        mode_version: "1.0.0".into(),
        policy_version: String::new(),
        configuration_version: String::new(),
    };
    send(
        &mut client,
        "commitment",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.handoff.v1".into(),
            message_type: "Commitment".into(),
            message_id: "m4".into(),
            session_id: "handoff-1".into(),
            sender: "owner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: commitment.encode_to_vec(),
        },
    )
    .await;

    // 6) GetSession
    match client
        .get_session(GetSessionRequest {
            session_id: "handoff-1".into(),
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
