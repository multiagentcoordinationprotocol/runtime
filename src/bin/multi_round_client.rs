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

    println!("=== Multi-Round Convergence Demo ===\n");

    // 1) SessionStart with multi_round mode
    let start_payload = SessionStartPayload {
        intent: "convergence test".into(),
        ttl_ms: 60000,
        participants: vec!["alice".into(), "bob".into()],
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
            mode: "multi_round".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "mr1".into(),
            sender: "coordinator".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        },
    )
    .await;

    // 2) Alice contributes "option_a"
    send(
        &mut client,
        "alice_contributes_a",
        Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m1".into(),
            session_id: "mr1".into(),
            sender: "alice".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: br#"{"value":"option_a"}"#.to_vec(),
        },
    )
    .await;

    // 3) Bob contributes "option_b" (no convergence yet)
    send(
        &mut client,
        "bob_contributes_b",
        Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m2".into(),
            session_id: "mr1".into(),
            sender: "bob".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: br#"{"value":"option_b"}"#.to_vec(),
        },
    )
    .await;

    // Query session state — should be Open
    match client
        .get_session(GetSessionRequest {
            session_id: "mr1".into(),
        })
        .await
    {
        Ok(resp) => {
            let meta = resp.into_inner().metadata.unwrap();
            println!("[get_session] state={} mode={}", meta.state, meta.mode);
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    // 4) Bob revises to "option_a" (convergence -> auto-resolved)
    send(
        &mut client,
        "bob_revises_to_a",
        Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m3".into(),
            session_id: "mr1".into(),
            sender: "bob".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: br#"{"value":"option_a"}"#.to_vec(),
        },
    )
    .await;

    // Query session state — should be Resolved
    match client
        .get_session(GetSessionRequest {
            session_id: "mr1".into(),
        })
        .await
    {
        Ok(resp) => {
            let meta = resp.into_inner().metadata.unwrap();
            println!(
                "[get_session] state={} mode_version={}",
                meta.state, meta.mode_version
            );
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    // 5) Another message — should be rejected: SessionNotOpen
    send(
        &mut client,
        "after_convergence",
        Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m4".into(),
            session_id: "mr1".into(),
            sender: "alice".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: br#"{"value":"option_c"}"#.to_vec(),
        },
    )
    .await;

    println!("\n=== Demo Complete ===");
    Ok(())
}
