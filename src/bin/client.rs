use macp_runtime::decision_pb::ProposalPayload;
use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{
    Envelope, GetSessionRequest, InitializeRequest, ListModesRequest, SendRequest,
    SessionStartPayload,
};
use prost::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = MacpRuntimeServiceClient::connect("http://127.0.0.1:50051").await?;

    // 1) Initialize — negotiate protocol version
    let init_resp = client
        .initialize(InitializeRequest {
            supported_protocol_versions: vec!["1.0".into()],
            client_info: None,
            capabilities: None,
        })
        .await?
        .into_inner();
    println!(
        "Initialize: version={} runtime={}",
        init_resp.selected_protocol_version,
        init_resp
            .runtime_info
            .as_ref()
            .map(|r| r.name.as_str())
            .unwrap_or("?")
    );

    // 2) ListModes — discover available modes
    let modes_resp = client.list_modes(ListModesRequest {}).await?.into_inner();
    println!(
        "ListModes: {:?}",
        modes_resp.modes.iter().map(|m| &m.mode).collect::<Vec<_>>()
    );

    // 3) SessionStart with participants (canonical mode)
    let start_payload = SessionStartPayload {
        intent: "demo canonical lifecycle".into(),
        participants: vec!["alice".into(), "bob".into()],
        mode_version: String::new(),
        configuration_version: String::new(),
        policy_version: String::new(),
        ttl_ms: 60_000,
        context: vec![],
        roots: vec![],
    };

    let start = Envelope {
        macp_version: "1.0".into(),
        mode: "macp.mode.decision.v1".into(),
        message_type: "SessionStart".into(),
        message_id: "m1".into(),
        session_id: "s1".into(),
        sender: "coordinator".into(),
        timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
        payload: start_payload.encode_to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(start),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "SessionStart ack: ok={} error={:?}",
        ack.ok,
        ack.error.as_ref().map(|e| &e.code)
    );

    // 4) Proposal (protobuf-encoded)
    let proposal = ProposalPayload {
        proposal_id: "p1".into(),
        option: "Deploy v2.1 to production".into(),
        rationale: "All integration tests pass".into(),
        supporting_data: vec![],
    };
    let proposal_env = Envelope {
        macp_version: "1.0".into(),
        mode: "macp.mode.decision.v1".into(),
        message_type: "Proposal".into(),
        message_id: "m2".into(),
        session_id: "s1".into(),
        sender: "alice".into(),
        timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
        payload: proposal.encode_to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(proposal_env),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "Proposal ack: ok={} error={:?}",
        ack.ok,
        ack.error.as_ref().map(|e| &e.code)
    );

    // 5) Evaluation (protobuf-encoded)
    let eval = macp_runtime::decision_pb::EvaluationPayload {
        proposal_id: "p1".into(),
        recommendation: "APPROVE".into(),
        confidence: 0.95,
        reason: "Looks good".into(),
    };
    let eval_env = Envelope {
        macp_version: "1.0".into(),
        mode: "macp.mode.decision.v1".into(),
        message_type: "Evaluation".into(),
        message_id: "m3".into(),
        session_id: "s1".into(),
        sender: "bob".into(),
        timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
        payload: eval.encode_to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(eval_env),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "Evaluation ack: ok={} error={:?}",
        ack.ok,
        ack.error.as_ref().map(|e| &e.code)
    );

    // 6) Commitment (votes not required per RFC — orchestrator bypass)
    let commitment = Envelope {
        macp_version: "1.0".into(),
        mode: "macp.mode.decision.v1".into(),
        message_type: "Commitment".into(),
        message_id: "m4".into(),
        session_id: "s1".into(),
        sender: "coordinator".into(),
        timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
        payload: b"deploy-approved".to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(commitment),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "Commitment ack: ok={} state={} error={:?}",
        ack.ok,
        ack.session_state,
        ack.error.as_ref().map(|e| &e.code)
    );

    // 7) GetSession — verify resolved state
    let resp = client
        .get_session(GetSessionRequest {
            session_id: "s1".into(),
        })
        .await?
        .into_inner();
    let meta = resp.metadata.unwrap();
    println!("GetSession: state={} mode={}", meta.state, meta.mode);

    Ok(())
}
