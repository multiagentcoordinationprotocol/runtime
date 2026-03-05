use macp_runtime::pb::macp_service_client::MacpServiceClient;
use macp_runtime::pb::{Envelope, GetSessionRequest, SendMessageRequest};

async fn send(client: &mut MacpServiceClient<tonic::transport::Channel>, label: &str, e: Envelope) {
    let req = SendMessageRequest { envelope: Some(e) };
    match client.send_message(req).await {
        Ok(resp) => {
            let ack = resp.into_inner();
            println!("[{label}] accepted={} error='{}'", ack.accepted, ack.error);
        }
        Err(status) => {
            println!("[{label}] grpc error: {status}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = MacpServiceClient::connect("http://127.0.0.1:50051").await?;

    println!("=== Multi-Round Convergence Demo ===\n");

    // 1) SessionStart with multi_round mode
    let payload = serde_json::json!({
        "participants": ["alice", "bob"],
        "convergence": {"type": "all_equal"},
        "ttl_ms": 60000
    });
    send(
        &mut client,
        "session_start",
        Envelope {
            macp_version: "v1".into(),
            mode: "multi_round".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "mr1".into(),
            sender: "coordinator".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: payload.to_string().into_bytes(),
        },
    )
    .await;

    // 2) Alice contributes "option_a"
    send(
        &mut client,
        "alice_contributes_a",
        Envelope {
            macp_version: "v1".into(),
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
            macp_version: "v1".into(),
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
            let info = resp.into_inner();
            println!(
                "[get_session] state={} mode={} participants={:?}",
                info.state, info.mode, info.participants
            );
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    // 4) Bob revises to "option_a" (convergence → auto-resolved)
    send(
        &mut client,
        "bob_revises_to_a",
        Envelope {
            macp_version: "v1".into(),
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
            let info = resp.into_inner();
            println!(
                "[get_session] state={} resolution={}",
                info.state,
                String::from_utf8_lossy(&info.resolution)
            );
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    // 5) Another message — should be rejected: SessionNotOpen
    send(
        &mut client,
        "after_convergence",
        Envelope {
            macp_version: "v1".into(),
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
