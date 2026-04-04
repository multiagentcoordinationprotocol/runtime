use crate::common;
use macp_integration_tests::helpers::*;

#[tokio::test]
async fn multi_round_happy_path() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let agent_a = "agent://agent-a";
    let agent_b = "agent://agent-b";

    // SessionStart
    let ack = send_as(
        &mut client,
        agent_a,
        envelope(
            MODE_MULTI_ROUND,
            "SessionStart",
            &new_message_id(),
            &sid,
            agent_a,
            session_start_payload("converge on value", &[agent_a, agent_b], 30_000),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Round 1: Contribute from both agents
    let ack = send_as(
        &mut client,
        agent_a,
        envelope(
            MODE_MULTI_ROUND,
            "Contribute",
            &new_message_id(),
            &sid,
            agent_a,
            serde_json::to_vec(&serde_json::json!({
                "value": "alpha"
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    let ack = send_as(
        &mut client,
        agent_b,
        envelope(
            MODE_MULTI_ROUND,
            "Contribute",
            &new_message_id(),
            &sid,
            agent_b,
            serde_json::to_vec(&serde_json::json!({
                "value": "beta"
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Round 2: Revised contributions
    let ack = send_as(
        &mut client,
        agent_a,
        envelope(
            MODE_MULTI_ROUND,
            "Contribute",
            &new_message_id(),
            &sid,
            agent_a,
            serde_json::to_vec(&serde_json::json!({
                "value": "converged"
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    let ack = send_as(
        &mut client,
        agent_b,
        envelope(
            MODE_MULTI_ROUND,
            "Contribute",
            &new_message_id(),
            &sid,
            agent_b,
            serde_json::to_vec(&serde_json::json!({
                "value": "converged"
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);

    // Commitment
    let ack = send_as(
        &mut client,
        agent_a,
        envelope(
            MODE_MULTI_ROUND,
            "Commitment",
            &new_message_id(),
            &sid,
            agent_a,
            commitment_payload("c1", "converged", "group", "all agreed"),
        ),
    )
    .await
    .unwrap();
    assert!(ack.ok);
    assert_eq!(ack.session_state, 2);
}
