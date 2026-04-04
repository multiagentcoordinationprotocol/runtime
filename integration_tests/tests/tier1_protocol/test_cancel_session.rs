use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn cancel_active_session() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let coord = "agent://coordinator";
    let participant = "agent://participant";

    // Start a decision session
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            coord,
            session_start_payload("cancel test", &[coord, participant], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Cancel from initiator
    let ack = cancel_session_as(&mut client, coord, &sid, "changed my mind").await.unwrap();
    assert!(ack.ok);

    // Verify session is no longer open
    let resp = get_session_as(&mut client, coord, &sid).await.unwrap();
    let meta = resp.metadata.expect("metadata present");
    assert_ne!(meta.state, 1); // not OPEN
}

#[tokio::test]
async fn send_to_cancelled_session_fails() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let coord = "agent://coordinator";
    let voter = "agent://voter";

    // Start and cancel
    send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            coord,
            session_start_payload("cancel test 2", &[coord, voter], 30_000),
        ),
    )
    .await
    .unwrap();

    cancel_session_as(&mut client, coord, &sid, "done").await.unwrap();

    // Try sending to cancelled session
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "Proposal",
            &new_message_id(),
            &sid,
            coord,
            proposal_payload("p1", "late proposal", "should fail"),
        ),
    )
    .await
    .unwrap();
    assert!(!ack.ok);
}
