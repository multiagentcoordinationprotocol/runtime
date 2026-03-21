use chrono::Utc;
use macp_runtime::log_store::LogStore;
use macp_runtime::pb::{CommitmentPayload, Envelope, SessionStartPayload};
use macp_runtime::registry::SessionRegistry;
use macp_runtime::runtime::Runtime;
use macp_runtime::session::SessionState;
use macp_runtime::storage::MemoryBackend;
use prost::Message;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

#[derive(Deserialize)]
struct ConformanceFixture {
    mode: String,
    initiator: String,
    participants: Vec<String>,
    mode_version: String,
    configuration_version: String,
    policy_version: String,
    ttl_ms: i64,
    messages: Vec<ConformanceMessage>,
    expected_final_state: String,
}

#[derive(Deserialize)]
struct ConformanceMessage {
    sender: String,
    message_type: String,
    payload_type: String,
    payload: serde_json::Value,
    expect: String,
}

fn new_sid() -> String {
    uuid::Uuid::new_v4().as_hyphenated().to_string()
}

fn make_runtime() -> Runtime {
    let storage: Arc<dyn macp_runtime::storage::StorageBackend> = Arc::new(MemoryBackend);
    let registry = Arc::new(SessionRegistry::new());
    let log_store = Arc::new(LogStore::new());
    Runtime::new(storage, registry, log_store)
}

fn encode_payload(fixture: &ConformanceFixture, msg: &ConformanceMessage) -> Vec<u8> {
    match msg.payload_type.as_str() {
        "Commitment" => {
            let p = &msg.payload;
            CommitmentPayload {
                commitment_id: p["commitment_id"].as_str().unwrap_or_default().into(),
                action: p["action"].as_str().unwrap_or_default().into(),
                authority_scope: p["authority_scope"].as_str().unwrap_or_default().into(),
                reason: p["reason"].as_str().unwrap_or_default().into(),
                mode_version: p["mode_version"].as_str().unwrap_or_default().into(),
                policy_version: p["policy_version"].as_str().unwrap_or_default().into(),
                configuration_version: p["configuration_version"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
            }
            .encode_to_vec()
        }
        t if t.starts_with("decision.") => encode_decision_payload(msg),
        t if t.starts_with("proposal.") => encode_proposal_payload(msg),
        t if t.starts_with("task.") => encode_task_payload(msg),
        t if t.starts_with("handoff.") => encode_handoff_payload(msg),
        t if t.starts_with("quorum.") => encode_quorum_payload(msg),
        _ => panic!(
            "Unknown payload_type: {} in fixture for mode {}",
            msg.payload_type, fixture.mode
        ),
    }
}

fn encode_decision_payload(msg: &ConformanceMessage) -> Vec<u8> {
    let p = &msg.payload;
    match msg.message_type.as_str() {
        "Proposal" => macp_runtime::decision_pb::ProposalPayload {
            proposal_id: p["proposal_id"].as_str().unwrap_or_default().into(),
            option: p["option"].as_str().unwrap_or_default().into(),
            rationale: p["rationale"].as_str().unwrap_or_default().into(),
            supporting_data: vec![],
        }
        .encode_to_vec(),
        "Vote" => macp_runtime::decision_pb::VotePayload {
            proposal_id: p["proposal_id"].as_str().unwrap_or_default().into(),
            vote: p["vote"].as_str().unwrap_or_default().into(),
            reason: p["reason"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        _ => panic!("Unhandled decision message: {}", msg.message_type),
    }
}

fn encode_proposal_payload(msg: &ConformanceMessage) -> Vec<u8> {
    let p = &msg.payload;
    match msg.message_type.as_str() {
        "Proposal" => macp_runtime::proposal_pb::ProposalPayload {
            proposal_id: p["proposal_id"].as_str().unwrap_or_default().into(),
            title: p["title"].as_str().unwrap_or_default().into(),
            summary: p["summary"].as_str().unwrap_or_default().into(),
            details: vec![],
            tags: vec![],
        }
        .encode_to_vec(),
        "Accept" => macp_runtime::proposal_pb::AcceptPayload {
            proposal_id: p["proposal_id"].as_str().unwrap_or_default().into(),
            reason: p["reason"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        _ => panic!("Unhandled proposal message: {}", msg.message_type),
    }
}

fn encode_task_payload(msg: &ConformanceMessage) -> Vec<u8> {
    let p = &msg.payload;
    match msg.message_type.as_str() {
        "TaskRequest" => macp_runtime::task_pb::TaskRequestPayload {
            task_id: p["task_id"].as_str().unwrap_or_default().into(),
            title: p["title"].as_str().unwrap_or_default().into(),
            instructions: p["instructions"].as_str().unwrap_or_default().into(),
            requested_assignee: p["requested_assignee"].as_str().unwrap_or_default().into(),
            input: vec![],
            deadline_unix_ms: p["deadline_unix_ms"].as_i64().unwrap_or(0),
        }
        .encode_to_vec(),
        "TaskAccept" => macp_runtime::task_pb::TaskAcceptPayload {
            task_id: p["task_id"].as_str().unwrap_or_default().into(),
            assignee: p["assignee"].as_str().unwrap_or_default().into(),
            reason: p["reason"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        "TaskComplete" => macp_runtime::task_pb::TaskCompletePayload {
            task_id: p["task_id"].as_str().unwrap_or_default().into(),
            assignee: p["assignee"].as_str().unwrap_or_default().into(),
            output: vec![],
            summary: p["summary"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        _ => panic!("Unhandled task message: {}", msg.message_type),
    }
}

fn encode_handoff_payload(msg: &ConformanceMessage) -> Vec<u8> {
    let p = &msg.payload;
    match msg.message_type.as_str() {
        "HandoffOffer" => macp_runtime::handoff_pb::HandoffOfferPayload {
            handoff_id: p["handoff_id"].as_str().unwrap_or_default().into(),
            target_participant: p["target_participant"].as_str().unwrap_or_default().into(),
            scope: p["scope"].as_str().unwrap_or_default().into(),
            reason: p["reason"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        "HandoffAccept" => macp_runtime::handoff_pb::HandoffAcceptPayload {
            handoff_id: p["handoff_id"].as_str().unwrap_or_default().into(),
            accepted_by: p["accepted_by"].as_str().unwrap_or_default().into(),
            reason: p["reason"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        _ => panic!("Unhandled handoff message: {}", msg.message_type),
    }
}

fn encode_quorum_payload(msg: &ConformanceMessage) -> Vec<u8> {
    let p = &msg.payload;
    match msg.message_type.as_str() {
        "ApprovalRequest" => macp_runtime::quorum_pb::ApprovalRequestPayload {
            request_id: p["request_id"].as_str().unwrap_or_default().into(),
            action: p["action"].as_str().unwrap_or_default().into(),
            summary: p["summary"].as_str().unwrap_or_default().into(),
            details: vec![],
            required_approvals: p["required_approvals"].as_u64().unwrap_or(0) as u32,
        }
        .encode_to_vec(),
        "Approve" => macp_runtime::quorum_pb::ApprovePayload {
            request_id: p["request_id"].as_str().unwrap_or_default().into(),
            reason: p["reason"].as_str().unwrap_or_default().into(),
        }
        .encode_to_vec(),
        _ => panic!("Unhandled quorum message: {}", msg.message_type),
    }
}

async fn run_conformance_fixture(path: &Path) {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {e}", path.display()));
    let fixture: ConformanceFixture = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse fixture {}: {e}", path.display()));

    let rt = make_runtime();
    let sid = new_sid();

    // SessionStart
    let start_payload = SessionStartPayload {
        intent: "conformance".into(),
        participants: fixture.participants.clone(),
        mode_version: fixture.mode_version.clone(),
        configuration_version: fixture.configuration_version.clone(),
        policy_version: fixture.policy_version.clone(),
        ttl_ms: fixture.ttl_ms,
        context: vec![],
        roots: vec![],
    }
    .encode_to_vec();

    rt.process(
        &Envelope {
            macp_version: "1.0".into(),
            mode: fixture.mode.clone(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: sid.clone(),
            sender: fixture.initiator.clone(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: start_payload,
        },
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("SessionStart failed for {}: {e}", path.display()));

    // Process messages
    for (i, msg) in fixture.messages.iter().enumerate() {
        let payload = encode_payload(&fixture, msg);
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: fixture.mode.clone(),
            message_type: msg.message_type.clone(),
            message_id: format!("m{}", i + 1),
            session_id: sid.clone(),
            sender: msg.sender.clone(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload,
        };

        let result = rt.process(&env, None).await;
        match msg.expect.as_str() {
            "accept" => {
                result.unwrap_or_else(|e| {
                    panic!(
                        "Message {} ({}) expected accept but got error: {e} in {}",
                        i + 1,
                        msg.message_type,
                        path.display()
                    )
                });
            }
            "reject" => {
                assert!(
                    result.is_err(),
                    "Message {} ({}) expected reject but succeeded in {}",
                    i + 1,
                    msg.message_type,
                    path.display()
                );
            }
            other => panic!("Unknown expect value: {other}"),
        }
    }

    // Verify final state
    let session = rt.get_session_checked(&sid).await.unwrap();
    let expected_state = match fixture.expected_final_state.as_str() {
        "Open" => SessionState::Open,
        "Resolved" => SessionState::Resolved,
        "Expired" => SessionState::Expired,
        other => panic!("Unknown expected_final_state: {other}"),
    };
    assert_eq!(
        session.state,
        expected_state,
        "Final state mismatch for {}",
        path.display()
    );
}

macro_rules! conformance_test {
    ($name:ident, $file:expr) => {
        #[tokio::test]
        async fn $name() {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/conformance")
                .join($file);
            run_conformance_fixture(&path).await;
        }
    };
}

conformance_test!(conformance_decision_happy_path, "decision_happy_path.json");
conformance_test!(conformance_proposal_happy_path, "proposal_happy_path.json");
conformance_test!(conformance_task_happy_path, "task_happy_path.json");
conformance_test!(conformance_handoff_happy_path, "handoff_happy_path.json");
conformance_test!(conformance_quorum_happy_path, "quorum_happy_path.json");
