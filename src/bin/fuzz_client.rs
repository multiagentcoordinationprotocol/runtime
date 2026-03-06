use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{
    CancelSessionRequest, Envelope, GetManifestRequest, GetSessionRequest, InitializeRequest,
    ListModesRequest, ListRootsRequest, SendRequest, SessionStartPayload,
};
use prost::Message;
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

fn encode_session_start(ttl_ms: i64, participants: Vec<String>) -> Vec<u8> {
    let payload = SessionStartPayload {
        intent: String::new(),
        participants,
        mode_version: String::new(),
        configuration_version: String::new(),
        policy_version: String::new(),
        ttl_ms,
        context: vec![],
        roots: vec![],
    };
    payload.encode_to_vec()
}

async fn send(
    client: &mut MacpRuntimeServiceClient<tonic::transport::Channel>,
    label: &str,
    e: Envelope,
) {
    match client.send(SendRequest { envelope: Some(e) }).await {
        Ok(resp) => {
            let ack = resp.into_inner().ack.unwrap();
            let err_code = ack.error.as_ref().map(|e| e.code.as_str()).unwrap_or("");
            println!(
                "[{label}] ok={} duplicate={} error='{}'",
                ack.ok, ack.duplicate, err_code
            );
        }
        Err(status) => {
            println!("[{label}] grpc error: {status}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = MacpRuntimeServiceClient::connect("http://127.0.0.1:50051").await?;

    // --- 0) Initialize
    match client
        .initialize(InitializeRequest {
            supported_protocol_versions: vec!["1.0".into()],
            client_info: None,
            capabilities: None,
        })
        .await
    {
        Ok(resp) => {
            let init = resp.into_inner();
            println!("[initialize] version={}", init.selected_protocol_version);
        }
        Err(status) => println!("[initialize] error: {status}"),
    }

    // --- 0b) Initialize with bad version
    match client
        .initialize(InitializeRequest {
            supported_protocol_versions: vec!["2.0".into()],
            client_info: None,
            capabilities: None,
        })
        .await
    {
        Ok(_) => println!("[initialize_bad_version] unexpected success"),
        Err(status) => println!("[initialize_bad_version] error: {status}"),
    }

    // --- 1) Wrong MACP version (should reject)
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
    send(
        &mut client,
        "missing_fields",
        env(
            "1.0",
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
            "1.0",
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
            "1.0",
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

    // --- 5) Duplicate SessionStart (should reject → INVALID_ENVELOPE)
    send(
        &mut client,
        "session_start_duplicate",
        env(
            "1.0",
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

    // --- 5b) Duplicate SessionStart with SAME message_id (idempotent)
    send(
        &mut client,
        "session_start_idempotent",
        env(
            "1.0",
            "decision",
            "SessionStart",
            "m1",
            "s1",
            "ajit",
            1_700_000_000_104,
            b"",
        ),
    )
    .await;

    // --- 6) Valid Message (should accept)
    send(
        &mut client,
        "message_ok",
        env(
            "1.0",
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

    // --- 6b) Duplicate message (same message_id)
    send(
        &mut client,
        "message_duplicate",
        env(
            "1.0",
            "decision",
            "Message",
            "m2",
            "s1",
            "ajit",
            1_700_000_000_105,
            b"hello",
        ),
    )
    .await;

    // --- 7) Resolve (payload == "resolve" => session becomes RESOLVED)
    send(
        &mut client,
        "resolve",
        env(
            "1.0",
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

    // --- 8) Message after resolved (should reject SESSION_NOT_OPEN)
    send(
        &mut client,
        "after_resolve",
        env(
            "1.0",
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

    // --- 9) TTL Expiry
    let ttl_payload = encode_session_start(1000, vec![]);
    send(
        &mut client,
        "ttl_session_start",
        env(
            "1.0",
            "decision",
            "SessionStart",
            "m_ttl1",
            "s_ttl",
            "ajit",
            1_700_000_000_200,
            &ttl_payload,
        ),
    )
    .await;

    sleep(Duration::from_millis(1200)).await;

    send(
        &mut client,
        "ttl_expired_message",
        env(
            "1.0",
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
    let bad_ttl = encode_session_start(-5000, vec![]);
    send(
        &mut client,
        "invalid_ttl_negative",
        env(
            "1.0",
            "decision",
            "SessionStart",
            "m_bad_ttl2",
            "s_bad_ttl2",
            "ajit",
            1_700_000_000_301,
            &bad_ttl,
        ),
    )
    .await;

    let bad_ttl = encode_session_start(86_400_001, vec![]);
    send(
        &mut client,
        "invalid_ttl_exceeds_max",
        env(
            "1.0",
            "decision",
            "SessionStart",
            "m_bad_ttl3",
            "s_bad_ttl3",
            "ajit",
            1_700_000_000_302,
            &bad_ttl,
        ),
    )
    .await;

    // --- 11) Multi-round convergence test
    let mr_payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
    send(
        &mut client,
        "multi_round_start",
        env(
            "1.0",
            "multi_round",
            "SessionStart",
            "m_mr0",
            "s_mr",
            "creator",
            1_700_000_000_400,
            &mr_payload,
        ),
    )
    .await;

    send(
        &mut client,
        "multi_round_alice",
        env(
            "1.0",
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
            "1.0",
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
            "1.0",
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
            "1.0",
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

    // --- 12) CancelSession
    send(
        &mut client,
        "cancel_session_start",
        env(
            "1.0",
            "decision",
            "SessionStart",
            "m_c1",
            "s_cancel",
            "ajit",
            1_700_000_000_500,
            b"",
        ),
    )
    .await;

    match client
        .cancel_session(CancelSessionRequest {
            session_id: "s_cancel".into(),
            reason: "test cancellation".into(),
        })
        .await
    {
        Ok(resp) => {
            let ack = resp.into_inner().ack.unwrap();
            println!("[cancel_session] ok={}", ack.ok);
        }
        Err(status) => println!("[cancel_session] error: {status}"),
    }

    send(
        &mut client,
        "after_cancel",
        env(
            "1.0",
            "decision",
            "Message",
            "m_c2",
            "s_cancel",
            "ajit",
            1_700_000_000_501,
            b"should_fail",
        ),
    )
    .await;

    // --- 13) Participant validation
    let p_payload = encode_session_start(0, vec!["alice".into(), "bob".into()]);
    send(
        &mut client,
        "participant_session_start",
        env(
            "1.0",
            "decision",
            "SessionStart",
            "m_p1",
            "s_participant",
            "alice",
            1_700_000_000_600,
            &p_payload,
        ),
    )
    .await;
    send(
        &mut client,
        "unauthorized_sender",
        env(
            "1.0",
            "decision",
            "Message",
            "m_p2",
            "s_participant",
            "charlie",
            1_700_000_000_601,
            b"hello",
        ),
    )
    .await;
    send(
        &mut client,
        "authorized_sender",
        env(
            "1.0",
            "decision",
            "Message",
            "m_p3",
            "s_participant",
            "alice",
            1_700_000_000_602,
            b"hello",
        ),
    )
    .await;

    // --- 14) Signal
    send(
        &mut client,
        "signal",
        env(
            "1.0",
            "",
            "Signal",
            "sig1",
            "",
            "alice",
            1_700_000_000_700,
            b"",
        ),
    )
    .await;

    // --- 15) GetSession
    match client
        .get_session(GetSessionRequest {
            session_id: "s1".into(),
        })
        .await
    {
        Ok(resp) => {
            let meta = resp.into_inner().metadata.unwrap();
            println!("[get_session] state={} mode={}", meta.state, meta.mode);
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    // --- 16) ListModes
    match client.list_modes(ListModesRequest {}).await {
        Ok(resp) => {
            let modes = resp.into_inner().modes;
            println!(
                "[list_modes] count={} modes={:?}",
                modes.len(),
                modes.iter().map(|m| &m.mode).collect::<Vec<_>>()
            );
        }
        Err(status) => println!("[list_modes] error: {status}"),
    }

    // --- 17) GetManifest
    match client
        .get_manifest(GetManifestRequest {
            agent_id: String::new(),
        })
        .await
    {
        Ok(resp) => {
            let manifest = resp.into_inner().manifest.unwrap();
            println!(
                "[get_manifest] agent_id={} modes={:?}",
                manifest.agent_id, manifest.supported_modes
            );
        }
        Err(status) => println!("[get_manifest] error: {status}"),
    }

    // --- 18) ListRoots
    match client.list_roots(ListRootsRequest {}).await {
        Ok(resp) => println!("[list_roots] count={}", resp.into_inner().roots.len()),
        Err(status) => println!("[list_roots] error: {status}"),
    }

    Ok(())
}
