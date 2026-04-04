use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn concurrent_sessions_across_modes() {
    let mut client = common::grpc_client().await;

    // Start 5 sessions in different modes simultaneously
    let modes = [
        MODE_DECISION,
        MODE_PROPOSAL,
        MODE_TASK,
        MODE_HANDOFF,
        MODE_QUORUM,
    ];

    let mut session_ids = Vec::new();
    for mode in &modes {
        let sid = new_session_id();
        let agent = "agent://concurrent-test";
        let partner = "agent://partner";

        let ack = send_as(
            &mut client,
            agent,
            envelope(
                mode,
                "SessionStart",
                &new_message_id(),
                &sid,
                agent,
                session_start_payload("concurrent", &[agent, partner], 30_000),
            ),
        )
        .await
        .unwrap();
        assert!(ack.ok, "Failed to start session for mode {mode}");
        session_ids.push((sid, *mode));
    }

    // Verify all sessions are open
    for (sid, mode) in &session_ids {
        let resp = get_session_as(&mut client, "agent://concurrent-test", sid)
            .await
            .unwrap();
        let meta = resp.metadata.expect("metadata present");
        assert_eq!(meta.state, 1, "Session for {mode} should be OPEN"); // OPEN
        assert_eq!(meta.mode, *mode);
    }
}

#[tokio::test]
async fn parallel_decision_sessions_are_independent() {
    let mut client = common::grpc_client().await;
    let coord = "agent://coord";
    let voter = "agent://voter";

    let sid1 = new_session_id();
    let sid2 = new_session_id();

    // Start two independent decision sessions
    for sid in [&sid1, &sid2] {
        send_as(
            &mut client,
            coord,
            envelope(
                MODE_DECISION,
                "SessionStart",
                &new_message_id(),
                sid,
                coord,
                session_start_payload("parallel", &[coord, voter], 30_000),
            ),
        )
        .await
        .unwrap();
    }

    // Resolve only session 1
    send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "Proposal",
            &new_message_id(),
            &sid1,
            coord,
            proposal_payload("p1", "option-A", "test"),
        ),
    )
    .await
    .unwrap();

    send_as(
        &mut client,
        voter,
        envelope(
            MODE_DECISION,
            "Vote",
            &new_message_id(),
            &sid1,
            voter,
            vote_payload("p1", "yes", "ok"),
        ),
    )
    .await
    .unwrap();

    send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "Commitment",
            &new_message_id(),
            &sid1,
            coord,
            commitment_payload("c1", "option-A", "team", "done"),
        ),
    )
    .await
    .unwrap();

    // Verify: session 1 resolved, session 2 still open
    let resp1 = get_session_as(&mut client, coord, &sid1).await.unwrap();
    assert_eq!(resp1.metadata.unwrap().state, 2); // RESOLVED

    let resp2 = get_session_as(&mut client, coord, &sid2).await.unwrap();
    assert_eq!(resp2.metadata.unwrap().state, 1); // OPEN
}
