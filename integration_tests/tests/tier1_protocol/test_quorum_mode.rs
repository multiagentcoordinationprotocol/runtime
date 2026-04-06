use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn quorum_happy_path() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let requester = "agent://requester";
    let approver1 = "agent://approver1";
    let approver2 = "agent://approver2";
    let approver3 = "agent://approver3";

    // SessionStart
    let ack = send_as(
        &mut client,
        requester,
        envelope(
            MODE_QUORUM,
            "SessionStart",
            &new_message_id(),
            &sid,
            requester,
            session_start_payload(
                "approve deployment",
                &[requester, approver1, approver2, approver3],
                30_000,
            ),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // ApprovalRequest from requester (need 2 of 3 approvers)
    let ack = send_as(
        &mut client,
        requester,
        envelope(
            MODE_QUORUM,
            "ApprovalRequest",
            &new_message_id(),
            &sid,
            requester,
            approval_request_payload("r1", "deploy-prod", "Deploy v2 to production", 2),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Approve from approver1
    let ack = send_as(
        &mut client,
        approver1,
        envelope(
            MODE_QUORUM,
            "Approve",
            &new_message_id(),
            &sid,
            approver1,
            approve_payload("r1", "LGTM"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Approve from approver2 (quorum reached)
    let ack = send_as(
        &mut client,
        approver2,
        envelope(
            MODE_QUORUM,
            "Approve",
            &new_message_id(),
            &sid,
            approver2,
            approve_payload("r1", "Approved"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Commitment from requester
    let ack = send_as(
        &mut client,
        requester,
        envelope(
            MODE_QUORUM,
            "Commitment",
            &new_message_id(),
            &sid,
            requester,
            commitment_payload("c1", "deploy-prod", "ops-team", "quorum reached", true),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);
    assert_eq!(ack.session_state, 2);
}
