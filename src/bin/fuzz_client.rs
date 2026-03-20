#[path = "support/common.rs"]
mod common;

use common::{
    cancel_session_as, canonical_start_payload, envelope, get_manifest, get_session_as, initialize,
    list_modes, list_roots, print_ack, send_as,
};
use macp_runtime::task_pb::TaskRequestPayload;
use prost::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = common::connect_client().await?;
    let session_id = common::new_session_id();

    println!("=== Freeze-Profile Error Path Demo ===\n");

    let init = initialize(&mut client).await?;
    println!("[initialize] version={}", init.selected_protocol_version);

    let modes = list_modes(&mut client).await?;
    println!(
        "[list_modes] {:?}",
        modes
            .modes
            .iter()
            .map(|m| m.mode.as_str())
            .collect::<Vec<_>>()
    );

    let roots = list_roots(&mut client).await?;
    println!("[list_roots] count={}", roots.roots.len());

    let manifest = get_manifest(&mut client, "demo-agent").await?;
    println!(
        "[get_manifest] agent_id={} modes={}",
        manifest
            .manifest
            .as_ref()
            .map(|m| m.agent_id.as_str())
            .unwrap_or("?"),
        manifest
            .manifest
            .as_ref()
            .map(|m| m.supported_modes.len())
            .unwrap_or(0)
    );

    let mut invalid_version = envelope(
        "macp.mode.decision.v1",
        "SessionStart",
        "bad-1",
        "freeze-check-1",
        "coordinator",
        canonical_start_payload("bad version", &["alice", "bob"], 60_000),
    );
    invalid_version.macp_version = "2.0".into();
    let ack = send_as(&mut client, "coordinator", invalid_version).await?;
    print_ack("invalid_version", &ack);

    let empty_mode = envelope(
        "",
        "SessionStart",
        "bad-2",
        "freeze-check-2",
        "coordinator",
        canonical_start_payload("empty mode", &["alice", "bob"], 60_000),
    );
    let ack = send_as(&mut client, "coordinator", empty_mode).await?;
    print_ack("empty_mode", &ack);

    let start = envelope(
        "macp.mode.task.v1",
        "SessionStart",
        "m0",
        &session_id,
        "planner",
        canonical_start_payload("freeze checks", &["planner", "worker"], 60_000),
    );
    let ack = send_as(&mut client, "planner", start).await?;
    print_ack("session_start", &ack);

    let duplicate_request = TaskRequestPayload {
        task_id: "dup-task".into(),
        title: "check duplicate handling".into(),
        instructions: "noop".into(),
        requested_assignee: "worker".into(),
        input: vec![],
        deadline_unix_ms: 0,
    };
    let duplicate = envelope(
        "macp.mode.task.v1",
        "TaskRequest",
        "dup-1",
        &session_id,
        "planner",
        duplicate_request.encode_to_vec(),
    );
    let ack = send_as(&mut client, "planner", duplicate.clone()).await?;
    print_ack("duplicate_first", &ack);
    let ack = send_as(&mut client, "planner", duplicate).await?;
    print_ack("duplicate_second", &ack);

    let spoofed = envelope(
        "macp.mode.task.v1",
        "TaskRequest",
        "spoof-1",
        &session_id,
        "mallory",
        vec![1, 2, 3],
    );
    let ack = send_as(&mut client, "planner", spoofed).await?;
    print_ack("spoofed_sender", &ack);

    let oversized = envelope(
        "macp.mode.task.v1",
        "TaskUpdate",
        "big-1",
        &session_id,
        "worker",
        vec![b'x'; 2_000_000],
    );
    let ack = send_as(&mut client, "worker", oversized).await?;
    print_ack("payload_too_large", &ack);

    match get_session_as(&mut client, "outsider", &session_id).await {
        Ok(resp) => println!(
            "[forbidden_get_session] unexpected success: {:?}",
            resp.metadata
        ),
        Err(status) => println!("[forbidden_get_session] grpc error: {status}"),
    }

    let cancelled = cancel_session_as(&mut client, "planner", &session_id, "end demo").await?;
    if let Some(ack) = cancelled.ack.as_ref() {
        print_ack("cancel_session", ack);
    }

    Ok(())
}
