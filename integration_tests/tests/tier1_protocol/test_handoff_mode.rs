use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn handoff_happy_path() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let source = "agent://source";
    let target = "agent://target";

    // SessionStart
    let ack = send_as(
        &mut client,
        source,
        envelope(
            MODE_HANDOFF,
            "SessionStart",
            &new_message_id(),
            &sid,
            source,
            session_start_payload("handoff customer", &[source, target], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // HandoffOffer from source
    let ack = send_as(
        &mut client,
        source,
        envelope(
            MODE_HANDOFF,
            "HandoffOffer",
            &new_message_id(),
            &sid,
            source,
            handoff_offer_payload("h1", target, "customer-support", "escalation needed"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // HandoffContext from source
    let ack = send_as(
        &mut client,
        source,
        envelope(
            MODE_HANDOFF,
            "HandoffContext",
            &new_message_id(),
            &sid,
            source,
            handoff_context_payload("h1", "application/json", b"{\"customer_id\": 42}"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // HandoffAccept from target
    let ack = send_as(
        &mut client,
        target,
        envelope(
            MODE_HANDOFF,
            "HandoffAccept",
            &new_message_id(),
            &sid,
            target,
            handoff_accept_payload("h1", target, "ready to assist"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Commitment from source
    let ack = send_as(
        &mut client,
        source,
        envelope(
            MODE_HANDOFF,
            "Commitment",
            &new_message_id(),
            &sid,
            source,
            commitment_payload("c1", "handoff-complete", "support", "transferred"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);
    assert_eq!(ack.session_state, 2);
}
