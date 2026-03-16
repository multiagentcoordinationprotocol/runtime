use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::{Envelope, GetSessionRequest, SendRequest, SessionStartPayload};
use macp_runtime::task_pb::{
    TaskAcceptPayload, TaskCompletePayload, TaskRequestPayload, TaskUpdatePayload,
};
use prost::Message;

async fn send(
    client: &mut MacpRuntimeServiceClient<tonic::transport::Channel>,
    label: &str,
    e: Envelope,
) {
    match client.send(SendRequest { envelope: Some(e) }).await {
        Ok(resp) => {
            let ack = resp.into_inner().ack.unwrap();
            let err_code = ack.error.as_ref().map(|e| e.code.as_str()).unwrap_or("");
            println!("[{label}] ok={} error='{}'", ack.ok, err_code);
        }
        Err(status) => {
            println!("[{label}] grpc error: {status}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = MacpRuntimeServiceClient::connect("http://127.0.0.1:50051").await?;

    println!("=== Task Mode Demo ===\n");

    // 1) SessionStart
    let start_payload = SessionStartPayload {
        intent: "summarize document".into(),
        ttl_ms: 60000,
        participants: vec!["planner".into(), "worker".into()],
        mode_version: String::new(),
        configuration_version: String::new(),
        policy_version: String::new(),
        context: vec![],
        roots: vec![],
    };
    send(
        &mut client,
        "session_start",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.task.v1".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "task-1".into(),
            sender: "planner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        },
    )
    .await;

    // 2) TaskRequest
    let task_req = TaskRequestPayload {
        task_id: "t1".into(),
        title: "Summarize Q4 report".into(),
        instructions: "Summarize the key findings".into(),
        requested_assignee: "worker".into(),
        input: vec![],
        deadline_unix_ms: 0,
    };
    send(
        &mut client,
        "task_request",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.task.v1".into(),
            message_type: "TaskRequest".into(),
            message_id: "m1".into(),
            session_id: "task-1".into(),
            sender: "planner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: task_req.encode_to_vec(),
        },
    )
    .await;

    // 3) TaskAccept
    let accept = TaskAcceptPayload {
        task_id: "t1".into(),
        assignee: "worker".into(),
        reason: "ready".into(),
    };
    send(
        &mut client,
        "task_accept",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.task.v1".into(),
            message_type: "TaskAccept".into(),
            message_id: "m2".into(),
            session_id: "task-1".into(),
            sender: "worker".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: accept.encode_to_vec(),
        },
    )
    .await;

    // 4) TaskUpdate
    let update = TaskUpdatePayload {
        task_id: "t1".into(),
        status: "in_progress".into(),
        progress: 0.5,
        message: "halfway done".into(),
        partial_output: vec![],
    };
    send(
        &mut client,
        "task_update",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.task.v1".into(),
            message_type: "TaskUpdate".into(),
            message_id: "m3".into(),
            session_id: "task-1".into(),
            sender: "worker".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: update.encode_to_vec(),
        },
    )
    .await;

    // 5) TaskComplete
    let complete = TaskCompletePayload {
        task_id: "t1".into(),
        assignee: "worker".into(),
        output: b"Executive summary: Revenue up 15%".to_vec(),
        summary: "completed".into(),
    };
    send(
        &mut client,
        "task_complete",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.task.v1".into(),
            message_type: "TaskComplete".into(),
            message_id: "m4".into(),
            session_id: "task-1".into(),
            sender: "worker".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: complete.encode_to_vec(),
        },
    )
    .await;

    // 6) Commitment
    let commitment = macp_runtime::pb::CommitmentPayload {
        commitment_id: "c1".into(),
        action: "task.completed".into(),
        authority_scope: "ops".into(),
        reason: "task done".into(),
        mode_version: "1.0.0".into(),
        policy_version: String::new(),
        configuration_version: String::new(),
    };
    send(
        &mut client,
        "commitment",
        Envelope {
            macp_version: "1.0".into(),
            mode: "macp.mode.task.v1".into(),
            message_type: "Commitment".into(),
            message_id: "m5".into(),
            session_id: "task-1".into(),
            sender: "planner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            payload: commitment.encode_to_vec(),
        },
    )
    .await;

    // 7) GetSession
    match client
        .get_session(GetSessionRequest {
            session_id: "task-1".into(),
        })
        .await
    {
        Ok(resp) => {
            let meta = resp.into_inner().metadata.unwrap();
            println!("[get_session] state={} mode={}", meta.state, meta.mode);
        }
        Err(status) => println!("[get_session] error: {status}"),
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}
