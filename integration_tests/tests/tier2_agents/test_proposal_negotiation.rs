use crate::common;
use macp_integration_tests::helpers::*;
use macp_integration_tests::macp_tools::{self, commit::*, proposal::*, session_start::*};
use rig::tool::ToolSet;

/// Buyer and seller agents negotiate via proposal tools.
#[tokio::test]
async fn rig_tools_drive_proposal_negotiation() {
    let ep = common::endpoint().await;
    let buyer_client = macp_tools::shared_client(ep).await;
    let seller_client = macp_tools::shared_client(ep).await;
    let sid = new_session_id();

    let mut buyer_tools = ToolSet::default();
    buyer_tools.add_tool(StartSessionTool {
        client: buyer_client.clone(),
        agent_id: "agent://buyer".into(),
    });
    buyer_tools.add_tool(AcceptProposalTool {
        client: buyer_client.clone(),
        agent_id: "agent://buyer".into(),
    });
    buyer_tools.add_tool(CommitTool {
        client: buyer_client.clone(),
        agent_id: "agent://buyer".into(),
        mode: MODE_PROPOSAL.into(),
    });

    let mut seller_tools = ToolSet::default();
    seller_tools.add_tool(SubmitProposalTool {
        client: seller_client.clone(),
        agent_id: "agent://seller".into(),
    });
    seller_tools.add_tool(AcceptProposalTool {
        client: seller_client.clone(),
        agent_id: "agent://seller".into(),
    });

    // Buyer starts session
    let r = buyer_tools
        .call(
            "macp_start_session",
            serde_json::json!({
                "mode": MODE_PROPOSAL, "session_id": &sid,
                "intent": "negotiate price", "participants": ["agent://buyer", "agent://seller"],
                "ttl_ms": 30000
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Seller proposes
    let r = seller_tools
        .call(
            "macp_submit_proposal",
            serde_json::json!({
                "session_id": &sid, "proposal_id": "prop-1",
                "title": "Initial offer", "summary": "$100 per unit"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Both accept
    let r = seller_tools
        .call(
            "macp_accept_proposal",
            serde_json::json!({
                "session_id": &sid, "proposal_id": "prop-1", "reason": "fair price"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    let r = buyer_tools
        .call(
            "macp_accept_proposal",
            serde_json::json!({
                "session_id": &sid, "proposal_id": "prop-1", "reason": "agreed"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Buyer commits
    let r = buyer_tools
        .call(
            "macp_commit",
            serde_json::json!({
                "session_id": &sid, "action": "accept-offer",
                "authority_scope": "negotiation", "reason": "both accepted"
            })
            .to_string(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    assert!(v["ok"].as_bool().unwrap());
    assert_eq!(v["session_state"], 2);
}
