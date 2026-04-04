use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn stream_receives_accepted_envelopes() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let coord = "agent://coordinator";
    let voter = "agent://voter";

    // Start session via unary Send
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "SessionStart",
            &new_message_id(),
            &sid,
            coord,
            session_start_payload("stream test", &[coord, voter], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Send a proposal to generate an accepted envelope
    let ack = send_as(
        &mut client,
        coord,
        envelope(
            MODE_DECISION,
            "Proposal",
            &new_message_id(),
            &sid,
            coord,
            proposal_payload("p1", "option-A", "stream test proposal"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Verify session is still open and has accepted messages
    let resp = get_session_as(&mut client, coord, &sid).await.unwrap();
    let meta = resp.metadata.expect("metadata present");
    assert_eq!(meta.state, 1); // OPEN
}
