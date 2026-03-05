use macp_runtime::pb::macp_service_client::MacpServiceClient;
use macp_runtime::pb::{Envelope, SendMessageRequest};
use tokio::time::{sleep, Duration};

#[allow(clippy::too_many_arguments)]
fn env(
    macp_version: &str,
    mode: &str,
    message_type: &str,
    message_id: &str,
    session_id: &str,
    sender: &str,
    ts: i64,
    payload: &[u8],
) -> Envelope {
    Envelope {
        macp_version: macp_version.into(),
        mode: mode.into(),
        message_type: message_type.into(),
        message_id: message_id.into(),
        session_id: session_id.into(),
        sender: sender.into(),
        timestamp_unix_ms: ts,
        payload: payload.to_vec(),
    }
}

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

    // --- 1) Wrong MACP version (should reject InvalidMacpVersion)
    send(
        &mut client,
        "wrong_version",
        env(
            "v0",
            "decision",
            "SessionStart",
            "badv1",
            "sv",
            "ajit",
            1_700_000_000_100,
            b"",
        ),
    )
    .await;

    // --- 2) Missing required fields (should reject InvalidEnvelope)
    // message_id empty + sender empty + ts <= 0
    send(
        &mut client,
        "missing_fields",
        env(
            "v1",
            "decision",
            "SessionStart",
            "",
            "s_missing",
            "",
            0,
            b"",
        ),
    )
    .await;

    // --- 3) Message to unknown session (should reject UnknownSession)
    send(
        &mut client,
        "unknown_session_message",
        env(
            "v1",
            "decision",
            "Message",
            "m_unknown",
            "no_such_session",
            "ajit",
            1_700_000_000_101,
            b"hello",
        ),
    )
    .await;

    // --- 4) Valid SessionStart (should accept)
    send(
        &mut client,
        "session_start_ok",
        env(
            "v1",
            "decision",
            "SessionStart",
            "m1",
            "s1",
            "ajit",
            1_700_000_000_102,
            b"",
        ),
    )
    .await;

    // --- 5) Duplicate SessionStart (should reject DuplicateSession)
    send(
        &mut client,
        "session_start_duplicate",
        env(
            "v1",
            "decision",
            "SessionStart",
            "m1_dup",
            "s1",
            "ajit",
            1_700_000_000_103,
            b"",
        ),
    )
    .await;

    // --- 6) Valid Message (should accept)
    send(
        &mut client,
        "message_ok",
        env(
            "v1",
            "decision",
            "Message",
            "m2",
            "s1",
            "ajit",
            1_700_000_000_104,
            b"hello",
        ),
    )
    .await;

    // --- 7) Resolve (payload == "resolve" => session becomes RESOLVED)
    send(
        &mut client,
        "resolve",
        env(
            "v1",
            "decision",
            "Message",
            "m3",
            "s1",
            "ajit",
            1_700_000_000_105,
            b"resolve",
        ),
    )
    .await;

    // --- 8) Message after resolved (should reject SessionNotOpen)
    send(
        &mut client,
        "after_resolve",
        env(
            "v1",
            "decision",
            "Message",
            "m4",
            "s1",
            "ajit",
            1_700_000_000_106,
            b"should_fail",
        ),
    )
    .await;

    // --- 9) TTL Expiry: SessionStart with short TTL, wait, then send message
    send(
        &mut client,
        "ttl_session_start",
        env(
            "v1",
            "decision",
            "SessionStart",
            "m_ttl1",
            "s_ttl",
            "ajit",
            1_700_000_000_200,
            br#"{"ttl_ms":1000}"#,
        ),
    )
    .await;

    sleep(Duration::from_millis(1200)).await;

    send(
        &mut client,
        "ttl_expired_message",
        env(
            "v1",
            "decision",
            "Message",
            "m_ttl2",
            "s_ttl",
            "ajit",
            1_700_000_000_201,
            b"should_expire",
        ),
    )
    .await;

    // --- 10) Invalid TTL values
    send(
        &mut client,
        "invalid_ttl_zero",
        env(
            "v1",
            "decision",
            "SessionStart",
            "m_bad_ttl1",
            "s_bad_ttl1",
            "ajit",
            1_700_000_000_300,
            br#"{"ttl_ms":0}"#,
        ),
    )
    .await;

    send(
        &mut client,
        "invalid_ttl_negative",
        env(
            "v1",
            "decision",
            "SessionStart",
            "m_bad_ttl2",
            "s_bad_ttl2",
            "ajit",
            1_700_000_000_301,
            br#"{"ttl_ms":-5000}"#,
        ),
    )
    .await;

    send(
        &mut client,
        "invalid_ttl_exceeds_max",
        env(
            "v1",
            "decision",
            "SessionStart",
            "m_bad_ttl3",
            "s_bad_ttl3",
            "ajit",
            1_700_000_000_302,
            br#"{"ttl_ms":86400001}"#,
        ),
    )
    .await;

    // --- 11) Multi-round convergence test
    let mr_payload = r#"{"participants":["alice","bob"],"convergence":{"type":"all_equal"}}"#;
    send(
        &mut client,
        "multi_round_start",
        env(
            "v1",
            "multi_round",
            "SessionStart",
            "m_mr0",
            "s_mr",
            "creator",
            1_700_000_000_400,
            mr_payload.as_bytes(),
        ),
    )
    .await;

    send(
        &mut client,
        "multi_round_alice",
        env(
            "v1",
            "multi_round",
            "Contribute",
            "m_mr1",
            "s_mr",
            "alice",
            1_700_000_000_401,
            br#"{"value":"option_a"}"#,
        ),
    )
    .await;

    send(
        &mut client,
        "multi_round_bob_diff",
        env(
            "v1",
            "multi_round",
            "Contribute",
            "m_mr2",
            "s_mr",
            "bob",
            1_700_000_000_402,
            br#"{"value":"option_b"}"#,
        ),
    )
    .await;

    send(
        &mut client,
        "multi_round_bob_converge",
        env(
            "v1",
            "multi_round",
            "Contribute",
            "m_mr3",
            "s_mr",
            "bob",
            1_700_000_000_403,
            br#"{"value":"option_a"}"#,
        ),
    )
    .await;

    send(
        &mut client,
        "multi_round_after_resolve",
        env(
            "v1",
            "multi_round",
            "Contribute",
            "m_mr4",
            "s_mr",
            "alice",
            1_700_000_000_404,
            br#"{"value":"option_c"}"#,
        ),
    )
    .await;

    Ok(())
}
