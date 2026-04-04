use crate::common;
use macp_integration_tests::helpers::*;
use macp_integration_tests::macp_tools::{self, commit::*, handoff::*, session_start::*};
use rig::tool::ToolSet;

/// Source agent hands off to target agent via Rig tools.
#[tokio::test]
async fn rig_tools_drive_handoff() {
    let ep = common::endpoint().await;
    let source_client = macp_tools::shared_client(ep).await;
    let target_client = macp_tools::shared_client(ep).await;
    let sid = new_session_id();

    let mut source_tools = ToolSet::default();
    source_tools.add_tool(StartSessionTool { client: source_client.clone(), agent_id: "agent://source".into() });
    source_tools.add_tool(HandoffOfferTool { client: source_client.clone(), agent_id: "agent://source".into() });
    source_tools.add_tool(CommitTool { client: source_client.clone(), agent_id: "agent://source".into(), mode: MODE_HANDOFF.into() });

    let mut target_tools = ToolSet::default();
    target_tools.add_tool(HandoffAcceptTool { client: target_client.clone(), agent_id: "agent://target".into() });

    // Source starts session
    let r = source_tools.call("macp_start_session", serde_json::json!({
        "mode": MODE_HANDOFF, "session_id": &sid,
        "intent": "escalate customer issue", "participants": ["agent://source", "agent://target"],
        "ttl_ms": 30000
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Source offers handoff
    let r = source_tools.call("macp_handoff_offer", serde_json::json!({
        "session_id": &sid, "handoff_id": "h1",
        "target": "agent://target", "scope": "customer-support",
        "reason": "needs specialist"
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Target accepts
    let r = target_tools.call("macp_handoff_accept", serde_json::json!({
        "session_id": &sid, "handoff_id": "h1", "reason": "ready to assist"
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Source commits
    let r = source_tools.call("macp_commit", serde_json::json!({
        "session_id": &sid, "action": "handoff-complete",
        "authority_scope": "support", "reason": "transferred"
    }).to_string()).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    assert!(v["ok"].as_bool().unwrap());
    assert_eq!(v["session_state"], 2);
}
