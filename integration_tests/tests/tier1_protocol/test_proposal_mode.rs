use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn proposal_happy_path() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let buyer = "agent://buyer";
    let seller = "agent://seller";

    // SessionStart
    let ack = send_as(
        &mut client,
        buyer,
        envelope(
            MODE_PROPOSAL,
            "SessionStart",
            &new_message_id(),
            &sid,
            buyer,
            session_start_payload("negotiate price", &[buyer, seller], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Proposal from seller
    let ack = send_as(
        &mut client,
        seller,
        envelope(
            MODE_PROPOSAL,
            "Proposal",
            &new_message_id(),
            &sid,
            seller,
            proposal_mode_payload("prop-1", "Initial offer", "$100"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // CounterProposal from buyer
    let ack = send_as(
        &mut client,
        buyer,
        envelope(
            MODE_PROPOSAL,
            "CounterProposal",
            &new_message_id(),
            &sid,
            buyer,
            counter_proposal_payload("prop-2", "prop-1", "Counter offer", "$80"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Accept from seller
    let ack = send_as(
        &mut client,
        seller,
        envelope(
            MODE_PROPOSAL,
            "Accept",
            &new_message_id(),
            &sid,
            seller,
            accept_proposal_payload("prop-2", "acceptable price"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Accept from buyer
    let ack = send_as(
        &mut client,
        buyer,
        envelope(
            MODE_PROPOSAL,
            "Accept",
            &new_message_id(),
            &sid,
            buyer,
            accept_proposal_payload("prop-2", "agreed"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Commitment from buyer (initiator)
    let ack = send_as(
        &mut client,
        buyer,
        envelope(
            MODE_PROPOSAL,
            "Commitment",
            &new_message_id(),
            &sid,
            buyer,
            commitment_payload("c1", "accept-counter", "negotiation", "both accepted", true),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);
    assert_eq!(ack.session_state, 2);
}
