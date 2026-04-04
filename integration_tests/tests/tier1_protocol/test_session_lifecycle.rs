use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn session_expires_after_ttl() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let agent = "agent://ttl-test";
    let partner = "agent://partner";

    // Start session with very short TTL (100ms)
    let ack = send_as(
        &mut client,
        agent,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            agent,
            session_start_payload("ttl test", &[agent, partner], 100),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Wait for TTL to expire
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Try sending — should fail because session expired
    let ack = send_as(
        &mut client,
        agent,
        envelope(
            MODE_DECISION,
            "Proposal",
            &new_message_id(),
            &sid,
            agent,
            proposal_payload("p1", "late", "expired"),
        ),
    )
    .await
    .unwrap();
    assert!(!ack.ok);
}

#[tokio::test]
async fn get_session_returns_open_state() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let agent = "agent://lifecycle-test";
    let partner = "agent://partner";

    // Start session
    send_as(
        &mut client,
        agent,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            agent,
            session_start_payload("lifecycle test", &[agent, partner], 30_000),
        ),
    )
    .await
    .unwrap();

    // GetSession should show OPEN
    let resp = get_session_as(&mut client, agent, &sid).await.unwrap();
    let meta = resp.metadata.expect("metadata present");
    assert_eq!(meta.state, 1); // OPEN
    assert_eq!(meta.mode, MODE_DECISION);
    assert_eq!(meta.session_id, sid);
}
