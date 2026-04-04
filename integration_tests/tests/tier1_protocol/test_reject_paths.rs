//! Mode-specific rejection tests — exercises protocol violations that the runtime must reject.
//! These mirror the conformance reject-path fixtures but go through the real gRPC boundary.

use crate::common;
use macp_integration_tests::helpers::*;

// ── Decision Mode ───────────────────────────────────────────────────────

#[tokio::test]
async fn decision_commitment_from_non_initiator_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let coord = "agent://coordinator";
    let voter = "agent://voter";

    // Start session (coord is initiator)
    send_as(&mut client, coord, envelope(
        MODE_DECISION, "SessionStart", &new_message_id(), &sid, coord,
        session_start_payload("test", &[coord, voter], 30_000),
    )).await.unwrap();

    // Proposal from coordinator
    send_as(&mut client, coord, envelope(
        MODE_DECISION, "Proposal", &new_message_id(), &sid, coord,
        proposal_payload("p1", "option-A", "test"),
    )).await.unwrap();

    // Voter tries to Commit — only initiator can commit per RFC-MACP-0007
    let ack = send_as(&mut client, voter, envelope(
        MODE_DECISION, "Commitment", &new_message_id(), &sid, voter,
        commitment_payload("c1", "option-A", "team", "unauthorized"),
    )).await.unwrap();
    assert!(!ack.ok, "Non-initiator Commitment must be rejected");
}

// ── Proposal Mode ───────────────────────────────────────────────────────

#[tokio::test]
async fn proposal_commitment_without_accept_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let buyer = "agent://buyer";
    let seller = "agent://seller";

    // Start session
    send_as(&mut client, buyer, envelope(
        MODE_PROPOSAL, "SessionStart", &new_message_id(), &sid, buyer,
        session_start_payload("negotiate", &[buyer, seller], 30_000),
    )).await.unwrap();

    // Proposal from seller (no Accept yet)
    send_as(&mut client, seller, envelope(
        MODE_PROPOSAL, "Proposal", &new_message_id(), &sid, seller,
        proposal_mode_payload("prop-1", "Offer", "$100"),
    )).await.unwrap();

    // Buyer tries to Commit without any Accept — should be rejected
    let ack = send_as(&mut client, buyer, envelope(
        MODE_PROPOSAL, "Commitment", &new_message_id(), &sid, buyer,
        commitment_payload("c1", "accept-offer", "negotiation", "premature"),
    )).await.unwrap();
    assert!(!ack.ok, "Commitment without Accept must be rejected");
}

// ── Task Mode ───────────────────────────────────────────────────────────

#[tokio::test]
async fn task_request_from_non_initiator_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let planner = "agent://planner";
    let worker = "agent://worker";

    // Start session (planner is initiator)
    send_as(&mut client, planner, envelope(
        MODE_TASK, "SessionStart", &new_message_id(), &sid, planner,
        session_start_payload("delegate", &[planner, worker], 30_000),
    )).await.unwrap();

    // Worker tries to send TaskRequest — only initiator can per RFC-MACP-0009
    let ack = send_as(&mut client, worker, envelope(
        MODE_TASK, "TaskRequest", &new_message_id(), &sid, worker,
        task_request_payload("t1", "Sneaky task", "unauthorized", planner),
    )).await.unwrap();
    assert!(!ack.ok, "TaskRequest from non-initiator must be rejected");
}

#[tokio::test]
async fn task_duplicate_task_id_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let planner = "agent://planner";
    let worker = "agent://worker";

    send_as(&mut client, planner, envelope(
        MODE_TASK, "SessionStart", &new_message_id(), &sid, planner,
        session_start_payload("delegate", &[planner, worker], 30_000),
    )).await.unwrap();

    // First TaskRequest succeeds
    let ack = send_as(&mut client, planner, envelope(
        MODE_TASK, "TaskRequest", &new_message_id(), &sid, planner,
        task_request_payload("t1", "First task", "do something", worker),
    )).await.unwrap();
    assert!(ack.ok);

    // Second TaskRequest with same task_id — should be rejected (one request per session)
    let ack = send_as(&mut client, planner, envelope(
        MODE_TASK, "TaskRequest", &new_message_id(), &sid, planner,
        task_request_payload("t1", "Duplicate task", "do again", worker),
    )).await.unwrap();
    assert!(!ack.ok, "Duplicate TaskRequest must be rejected");
}

// ── Handoff Mode ────────────────────────────────────────────────────────

#[tokio::test]
async fn handoff_accept_without_offer_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let source = "agent://source";
    let target = "agent://target";

    send_as(&mut client, source, envelope(
        MODE_HANDOFF, "SessionStart", &new_message_id(), &sid, source,
        session_start_payload("handoff", &[source, target], 30_000),
    )).await.unwrap();

    // Target tries to Accept without any HandoffOffer — should be rejected
    let ack = send_as(&mut client, target, envelope(
        MODE_HANDOFF, "HandoffAccept", &new_message_id(), &sid, target,
        handoff_accept_payload("h1", target, "premature accept"),
    )).await.unwrap();
    assert!(!ack.ok, "HandoffAccept without prior HandoffOffer must be rejected");
}

// ── Quorum Mode ─────────────────────────────────────────────────────────

#[tokio::test]
async fn quorum_approve_before_request_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let requester = "agent://requester";
    let approver = "agent://approver";

    send_as(&mut client, requester, envelope(
        MODE_QUORUM, "SessionStart", &new_message_id(), &sid, requester,
        session_start_payload("approve", &[requester, approver], 30_000),
    )).await.unwrap();

    // Approver tries to Approve without any ApprovalRequest — should be rejected
    let ack = send_as(&mut client, approver, envelope(
        MODE_QUORUM, "Approve", &new_message_id(), &sid, approver,
        approve_payload("r1", "premature approve"),
    )).await.unwrap();
    assert!(!ack.ok, "Approve before ApprovalRequest must be rejected");
}

#[tokio::test]
async fn quorum_commitment_before_quorum_reached_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let requester = "agent://requester";
    let approver1 = "agent://approver1";
    let approver2 = "agent://approver2";

    send_as(&mut client, requester, envelope(
        MODE_QUORUM, "SessionStart", &new_message_id(), &sid, requester,
        session_start_payload("approve", &[requester, approver1, approver2], 30_000),
    )).await.unwrap();

    // ApprovalRequest needs 2 approvals
    send_as(&mut client, requester, envelope(
        MODE_QUORUM, "ApprovalRequest", &new_message_id(), &sid, requester,
        approval_request_payload("r1", "deploy", "Deploy v2", 2),
    )).await.unwrap();

    // Only 1 approval (quorum not reached)
    send_as(&mut client, approver1, envelope(
        MODE_QUORUM, "Approve", &new_message_id(), &sid, approver1,
        approve_payload("r1", "LGTM"),
    )).await.unwrap();

    // Requester tries to Commit before quorum — should be rejected
    let ack = send_as(&mut client, requester, envelope(
        MODE_QUORUM, "Commitment", &new_message_id(), &sid, requester,
        commitment_payload("c1", "deploy", "ops", "premature"),
    )).await.unwrap();
    assert!(!ack.ok, "Commitment before quorum reached must be rejected");
}

// ── Multi-Round Mode ────────────────────────────────────────────────────

#[tokio::test]
async fn multi_round_commitment_before_convergence_rejected() {
    let mut client = common::grpc_client().await;
    let sid = new_session_id();
    let agent_a = "agent://agent-a";
    let agent_b = "agent://agent-b";

    send_as(&mut client, agent_a, envelope(
        MODE_MULTI_ROUND, "SessionStart", &new_message_id(), &sid, agent_a,
        session_start_payload("converge", &[agent_a, agent_b], 30_000),
    )).await.unwrap();

    // Divergent contributions (not converged)
    send_as(&mut client, agent_a, envelope(
        MODE_MULTI_ROUND, "Contribute", &new_message_id(), &sid, agent_a,
        serde_json::to_vec(&serde_json::json!({"value": "alpha"})).unwrap(),
    )).await.unwrap();

    send_as(&mut client, agent_b, envelope(
        MODE_MULTI_ROUND, "Contribute", &new_message_id(), &sid, agent_b,
        serde_json::to_vec(&serde_json::json!({"value": "beta"})).unwrap(),
    )).await.unwrap();

    // Try to Commit before convergence — should be rejected
    let ack = send_as(&mut client, agent_a, envelope(
        MODE_MULTI_ROUND, "Commitment", &new_message_id(), &sid, agent_a,
        commitment_payload("c1", "premature", "group", "not converged"),
    )).await.unwrap();
    assert!(!ack.ok, "Commitment before convergence must be rejected");
}
