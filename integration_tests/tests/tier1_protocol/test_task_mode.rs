use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn task_happy_path() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let planner = "agent://planner";
    let worker = "agent://worker";

    // SessionStart
    let ack = send_as(
        &mut client,
        planner,
        envelope(
            MODE_TASK,
            "SessionStart",
            &new_message_id(),
            &sid,
            planner,
            session_start_payload("delegate analysis", &[planner, worker], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // TaskRequest from planner
    let ack = send_as(
        &mut client,
        planner,
        envelope(
            MODE_TASK,
            "TaskRequest",
            &new_message_id(),
            &sid,
            planner,
            task_request_payload("t1", "Analyze data", "Run the analysis pipeline", worker),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // TaskAccept from worker
    let ack = send_as(
        &mut client,
        worker,
        envelope(
            MODE_TASK,
            "TaskAccept",
            &new_message_id(),
            &sid,
            worker,
            task_accept_payload("t1", worker),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // TaskUpdate from worker
    let ack = send_as(
        &mut client,
        worker,
        envelope(
            MODE_TASK,
            "TaskUpdate",
            &new_message_id(),
            &sid,
            worker,
            task_update_payload("t1", "in_progress", 0.5, "halfway done"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // TaskComplete from worker
    let ack = send_as(
        &mut client,
        worker,
        envelope(
            MODE_TASK,
            "TaskComplete",
            &new_message_id(),
            &sid,
            worker,
            task_complete_payload("t1", worker, "analysis complete"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Commitment from planner
    let ack = send_as(
        &mut client,
        planner,
        envelope(
            MODE_TASK,
            "Commitment",
            &new_message_id(),
            &sid,
            planner,
            commitment_payload("c1", "task-completed", "planner", "worker delivered"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);
    assert_eq!(ack.session_state, 2);
}
