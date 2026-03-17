use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{Envelope, GetSessionRequest, SendRequest, SessionStartPayload};
use macp_runtime::quorum_pb::{ApprovalRequestPayload, ApprovePayload};
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

    println!("=== Quorum Mode Demo ===\n");

    // 1) SessionStart
    let start_payload = SessionStartPayload {
        intent: "approve production deploy".into(),
        ttl_ms: 60000,
        participants: vec!["alice".into(), "bob".into(), "carol".into()],
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
            mode: "macp.mode.quorum.v1".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "quorum-1".into(),
            sender: "coordinator".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        },
    )
    .await;

    // 2) ApprovalRequest
    let req = ApprovalRequestPayload {
        request_id: "deploy-v2".into(),
        action: "deploy.production".into(),
        summary: "Deploy v2.1 to production".into(),
        details: vec![],
        required_approvals: 2,
    };
    send(
        &mut client,
        "approval_request",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.quorum.v1".into(),
            message_type: "ApprovalRequest".into(),
            message_id: "m1".into(),
            session_id: "quorum-1".into(),
            sender: "coordinator".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: req.encode_to_vec(),
        },
    )
    .await;

    // 3) Alice approves
    let approve1 = ApprovePayload {
        request_id: "deploy-v2".into(),
        reason: "tests pass".into(),
    };
    send(
        &mut client,
        "alice_approve",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.quorum.v1".into(),
            message_type: "Approve".into(),
            message_id: "m2".into(),
            session_id: "quorum-1".into(),
            sender: "alice".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: approve1.encode_to_vec(),
        },
    )
    .await;

    // 4) Bob approves
    let approve2 = ApprovePayload {
        request_id: "deploy-v2".into(),
        reason: "staging looks good".into(),
    };
    send(
        &mut client,
        "bob_approve",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.quorum.v1".into(),
            message_type: "Approve".into(),
            message_id: "m3".into(),
            session_id: "quorum-1".into(),
            sender: "bob".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: approve2.encode_to_vec(),
        },
    )
    .await;

    // 5) Commitment (threshold met: 2 of 3)
    let commitment = macp_runtime::pb::CommitmentPayload {
        commitment_id: "c1".into(),
        action: "quorum.approved".into(),
        authority_scope: "deploy".into(),
        reason: "2 of 3 approved".into(),
        mode_version: "1.0.0".into(),
        policy_version: String::new(),
        configuration_version: String::new(),
    };
    send(
        &mut client,
        "commitment",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.quorum.v1".into(),
            message_type: "Commitment".into(),
            message_id: "m4".into(),
            session_id: "quorum-1".into(),
            sender: "coordinator".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: commitment.encode_to_vec(),
        },
    )
    .await;

    // 6) GetSession
    match client
        .get_session(GetSessionRequest {
            session_id: "quorum-1".into(),
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
