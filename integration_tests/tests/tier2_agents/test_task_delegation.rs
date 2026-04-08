use crate::common;
use macp_integration_tests::helpers::*;
use macp_integration_tests::macp_tools::{self, commit::*, session_start::*, task::*};
use rig::tool::ToolSet;

/// Planner agent delegates a task to worker agent, worker completes it.
#[tokio::test]
async fn rig_tools_drive_task_delegation() {
    let ep = common::endpoint().await;
    let planner_client = macp_tools::shared_client(ep).await;
    let worker_client = macp_tools::shared_client(ep).await;
    let sid = new_session_id();

    let mut planner_tools = ToolSet::default();
    planner_tools.add_tool(StartSessionTool {
        client: planner_client.clone(),
        agent_id: "agent://planner".into(),
    });
    planner_tools.add_tool(TaskRequestTool {
        client: planner_client.clone(),
        agent_id: "agent://planner".into(),
    });
    planner_tools.add_tool(CommitTool {
        client: planner_client.clone(),
        agent_id: "agent://planner".into(),
        mode: MODE_TASK.into(),
    });

    let mut worker_tools = ToolSet::default();
    worker_tools.add_tool(TaskAcceptTool {
        client: worker_client.clone(),
        agent_id: "agent://worker".into(),
    });
    worker_tools.add_tool(TaskCompleteTool {
        client: worker_client.clone(),
        agent_id: "agent://worker".into(),
    });

    // Planner starts session
    let r = planner_tools
        .call(
            "macp_start_session",
            serde_json::json!({
                "mode": MODE_TASK, "session_id": &sid,
                "intent": "analyze data", "participants": ["agent://planner", "agent://worker"],
                "ttl_ms": 30000
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Planner creates task
    let r = planner_tools
        .call(
            "macp_task_request",
            serde_json::json!({
                "session_id": &sid, "task_id": "t1", "title": "Data analysis",
                "instructions": "Analyze Q4 metrics", "assignee": "agent://worker"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Worker accepts
    let r = worker_tools
        .call(
            "macp_task_accept",
            serde_json::json!({
                "session_id": &sid, "task_id": "t1"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Worker completes
    let r = worker_tools
        .call(
            "macp_task_complete",
            serde_json::json!({
                "session_id": &sid, "task_id": "t1", "summary": "Q4 metrics show 20% growth"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"]
        .as_bool()
        .unwrap());

    // Planner commits
    let r = planner_tools
        .call(
            "macp_commit",
            serde_json::json!({
                "session_id": &sid, "action": "task-completed",
                "authority_scope": "planner", "reason": "worker delivered results"
            })
            .to_string(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    assert!(v["ok"].as_bool().unwrap());
    assert_eq!(v["session_state"], 2);
}
