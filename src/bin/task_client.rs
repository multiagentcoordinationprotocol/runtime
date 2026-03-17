#[path = "support/common.rs"]
mod common;

use common::{
    canonical_commitment_payload, canonical_start_payload, envelope, get_session_as, print_ack,
    send_as,
};
use macp_runtime::task_pb::{
    TaskAcceptPayload, TaskCompletePayload, TaskRequestPayload, TaskUpdatePayload,
};
use prost::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = common::connect_client().await?;

    println!("=== Task Mode Demo ===\n");

    let ack = send_as(
        &mut client,
        "planner",
        envelope(
            "macp.mode.task.v1",
            "SessionStart",
            "m0",
            "task-demo-1",
            "planner",
            canonical_start_payload("summarize quarterly report", &["planner", "worker"], 60_000),
        ),
    )
    .await?;
    print_ack("session_start", &ack);

    let task_request = TaskRequestPayload {
        task_id: "t1".into(),
        title: "Summarize Q4 report".into(),
        instructions: "Produce a concise executive summary".into(),
        requested_assignee: "worker".into(),
        input: vec![],
        deadline_unix_ms: 0,
    };
    let ack = send_as(
        &mut client,
        "planner",
        envelope(
            "macp.mode.task.v1",
            "TaskRequest",
            "m1",
            "task-demo-1",
            "planner",
            task_request.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("task_request", &ack);

    let accept = TaskAcceptPayload {
        task_id: "t1".into(),
        assignee: "worker".into(),
        reason: "ready".into(),
    };
    let ack = send_as(
        &mut client,
        "worker",
        envelope(
            "macp.mode.task.v1",
            "TaskAccept",
            "m2",
            "task-demo-1",
            "worker",
            accept.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("task_accept", &ack);

    let update = TaskUpdatePayload {
        task_id: "t1".into(),
        status: "in_progress".into(),
        progress: 0.5,
        message: "draft summary complete".into(),
        partial_output: vec![],
    };
    let ack = send_as(
        &mut client,
        "worker",
        envelope(
            "macp.mode.task.v1",
            "TaskUpdate",
            "m3",
            "task-demo-1",
            "worker",
            update.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("task_update", &ack);

    let complete = TaskCompletePayload {
        task_id: "t1".into(),
        assignee: "worker".into(),
        output: b"Q4 summary".to_vec(),
        summary: "Report summarized successfully".into(),
    };
    let ack = send_as(
        &mut client,
        "worker",
        envelope(
            "macp.mode.task.v1",
            "TaskComplete",
            "m4",
            "task-demo-1",
            "worker",
            complete.encode_to_vec(),
        ),
    )
    .await?;
    print_ack("task_complete", &ack);

    let ack = send_as(
        &mut client,
        "planner",
        envelope(
            "macp.mode.task.v1",
            "Commitment",
            "m5",
            "task-demo-1",
            "planner",
            canonical_commitment_payload(
                "c1",
                "task.completed",
                "workflow",
                "worker completed task t1",
            ),
        ),
    )
    .await?;
    print_ack("commitment", &ack);

    let session = get_session_as(&mut client, "worker", "task-demo-1").await?;
    let meta = session.metadata.expect("metadata");
    println!("[get_session] state={} mode={}", meta.state, meta.mode);

    Ok(())
}
