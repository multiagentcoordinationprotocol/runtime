use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{
    Envelope, GetSessionRequest, InitializeRequest, ListModesRequest, SendRequest,
};

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

    // 3) SessionStart
    let start = Envelope {
        macp_version: "1.0".into(),
        mode: "decision".into(),
        message_type: "SessionStart".into(),
        message_id: "m1".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_000,
        payload: vec![],
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

    // 4) Normal message
    let msg = Envelope {
        macp_version: "1.0".into(),
        mode: "decision".into(),
        message_type: "Message".into(),
        message_id: "m2".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_001,
        payload: b"hello".to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(msg),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "Message ack: ok={} error={:?}",
        ack.ok,
        ack.error.as_ref().map(|e| &e.code)
    );

    // 5) Resolve message (DecisionMode resolves when payload == "resolve")
    let resolve = Envelope {
        macp_version: "1.0".into(),
        mode: "decision".into(),
        message_type: "Message".into(),
        message_id: "m3".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_002,
        payload: b"resolve".to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(resolve),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "Resolve ack: ok={} error={:?}",
        ack.ok,
        ack.error.as_ref().map(|e| &e.code)
    );

    // 6) Message after resolve (should be rejected: SessionNotOpen)
    let after = Envelope {
        macp_version: "1.0".into(),
        mode: "decision".into(),
        message_type: "Message".into(),
        message_id: "m4".into(),
        session_id: "s1".into(),
        sender: "ajit".into(),
        timestamp_unix_ms: 1_700_000_000_003,
        payload: b"should-fail".to_vec(),
    };

    let ack = client
        .send(SendRequest {
            envelope: Some(after),
        })
        .await?
        .into_inner()
        .ack
        .unwrap();
    println!(
        "After-resolve ack: ok={} error={:?}",
        ack.ok,
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
