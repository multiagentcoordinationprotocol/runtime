use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn decision_happy_path() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let coord = "agent://coordinator";
    let voter = "agent://voter";

    // SessionStart
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            coord,
            session_start_payload("decide deployment", &[coord, voter], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Proposal from coordinator
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "Proposal",
            &new_message_id(),
            &sid,
            coord,
            proposal_payload("p1", "deploy-v2", "better performance"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Evaluation from voter
    let ack = send_as(
        &mut client,
        voter,
        envelope(
            MODE_DECISION,
            "Evaluation",
            &new_message_id(),
            &sid,
            voter,
            evaluation_payload("p1", "approve", 0.9, "looks good"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Vote from voter
    let ack = send_as(
        &mut client,
        voter,
        envelope(
            MODE_DECISION,
            "Vote",
            &new_message_id(),
            &sid,
            voter,
            vote_payload("p1", "approve", "consensus"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Commitment from coordinator
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "Commitment",
            &new_message_id(),
            &sid,
            coord,
            commitment_payload("c1", "deploy-v2", "team", "consensus reached", true),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);
    assert_eq!(ack.session_state, 2); // RESOLVED

    // Verify via GetSession
    let resp = get_session_as(&mut client, coord, &sid).await.unwrap();
    let meta = resp.metadata.expect("metadata present");
    assert_eq!(meta.state, 2); // RESOLVED
}

#[tokio::test]
async fn decision_duplicate_message_is_flagged() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let coord = "agent://coordinator";
    let voter = "agent://voter";

    // SessionStart
    send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            coord,
            session_start_payload("test dedup", &[coord, voter], 30_000),
        ),
    )
    .await
    .unwrap();

    // Send same message_id twice
    let mid = new_message_id();
    let env1 = envelope(
        MODE_DECISION,
        "Proposal",
        &mid,
        &sid,
        coord,
        proposal_payload("p1", "option-A", "first"),
    );
    let env2 = envelope(
        MODE_DECISION,
        "Proposal",
        &mid,
        &sid,
        coord,
        proposal_payload("p1", "option-A", "first"),
    );

    let ack1 = send_as(&mut client, coord, env1).await.unwrap();
    assert!(ack1.ok);
    assert!(!ack1.duplicate);

    let ack2 = send_as(&mut client, coord, env2).await.unwrap();
    assert!(ack2.ok);
    assert!(ack2.duplicate);
}
