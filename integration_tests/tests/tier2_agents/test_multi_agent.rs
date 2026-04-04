use crate::common;
use macp_integration_tests::helpers::*;
use macp_integration_tests::macp_tools::{self, commit::*, quorum::*, session_start::*};
use rig::tool::ToolSet;

/// Three agents coordinate a quorum approval via Rig tools.
#[tokio::test]
async fn rig_tools_drive_quorum_approval() {
    let ep = common::endpoint().await;
    let req_client = macp_tools::shared_client(ep).await;
    let a1_client = macp_tools::shared_client(ep).await;
    let a2_client = macp_tools::shared_client(ep).await;
    let sid = new_session_id();

    let mut req_tools = ToolSet::default();
    req_tools.add_tool(StartSessionTool { client: req_client.clone(), agent_id: "agent://requester".into() });
    req_tools.add_tool(ApprovalRequestTool { client: req_client.clone(), agent_id: "agent://requester".into() });
    req_tools.add_tool(CommitTool { client: req_client.clone(), agent_id: "agent://requester".into(), mode: MODE_QUORUM.into() });

    let mut a1_tools = ToolSet::default();
    a1_tools.add_tool(ApproveTool { client: a1_client.clone(), agent_id: "agent://approver1".into() });

    let mut a2_tools = ToolSet::default();
    a2_tools.add_tool(ApproveTool { client: a2_client.clone(), agent_id: "agent://approver2".into() });

    // Requester starts session
    let r = req_tools.call("macp_start_session", serde_json::json!({
        "mode": MODE_QUORUM, "session_id": &sid,
        "intent": "approve production deploy",
        "participants": ["agent://requester", "agent://approver1", "agent://approver2"],
        "ttl_ms": 30000
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Requester submits approval request (need 2 approvals)
    let r = req_tools.call("macp_approval_request", serde_json::json!({
        "session_id": &sid, "request_id": "r1",
        "action": "deploy-prod", "summary": "Deploy v2 to production",
        "required_approvals": 2
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Approver 1 approves
    let r = a1_tools.call("macp_approve", serde_json::json!({
        "session_id": &sid, "request_id": "r1", "reason": "LGTM"
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Approver 2 approves (quorum met)
    let r = a2_tools.call("macp_approve", serde_json::json!({
        "session_id": &sid, "request_id": "r1", "reason": "Approved"
    }).to_string()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"].as_bool().unwrap());

    // Requester commits
    let r = req_tools.call("macp_commit", serde_json::json!({
        "session_id": &sid, "action": "deploy-prod",
        "authority_scope": "ops-team", "reason": "quorum reached"
    }).to_string()).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    assert!(v["ok"].as_bool().unwrap());
    assert_eq!(v["session_state"], 2);
}
