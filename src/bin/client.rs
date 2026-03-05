use macp_runtime::pb::macp_service_client::MacpServiceClient;
use macp_runtime::pb::{Envelope, SendMessageRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Server address from main.rs
    let mut client = MacpServiceClient::connect("http://127.0.0.1:50051").await?;

    // 1) SessionStart
    let start = Envelope {
        macp_version: "v1".into(),
        mode: "decision".into(),
        message_type: "SessionStart".into(),
        message_id: "m1".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_000,
        payload: vec![],
    };

    let ack = client
        .send_message(SendMessageRequest {
            envelope: Some(start),
        })
        .await?
        .into_inner();
    println!(
        "SessionStart ack: accepted={} error='{}'",
        ack.accepted, ack.error
    );

    // 2) Normal message
    let msg = Envelope {
        macp_version: "v1".into(),
        mode: "decision".into(),
        message_type: "Message".into(),
        message_id: "m2".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_001,
        payload: b"hello".to_vec(),
    };

    let ack = client
        .send_message(SendMessageRequest {
            envelope: Some(msg),
        })
        .await?
        .into_inner();
    println!(
        "Message ack: accepted={} error='{}'",
        ack.accepted, ack.error
    );

    // 3) Resolve message (DecisionMode resolves when payload == "resolve")
    let resolve = Envelope {
        macp_version: "v1".into(),
        mode: "decision".into(),
        message_type: "Message".into(),
        message_id: "m3".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_002,
        payload: b"resolve".to_vec(),
    };

    let ack = client
        .send_message(SendMessageRequest {
            envelope: Some(resolve),
        })
        .await?
        .into_inner();
    println!(
        "Resolve ack: accepted={} error='{}'",
        ack.accepted, ack.error
    );

    // 4) Message after resolve (should be rejected: SessionNotOpen)
    let after = Envelope {
        macp_version: "v1".into(),
        mode: "decision".into(),
        message_type: "Message".into(),
        message_id: "m4".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_003,
        payload: b"should-fail".to_vec(),
    };

    let ack = client
        .send_message(SendMessageRequest {
            envelope: Some(after),
        })
        .await?
        .into_inner();
    println!(
        "After-resolve ack: accepted={} error='{}'",
        ack.accepted, ack.error
    );

    Ok(())
}
