# Governance Policy Architecture

This document describes the MACP runtime's governance policy framework, implementing RFC-MACP-0012.

## Overview

Governance policies provide declarative, deterministic rules that constrain coordination sessions beyond the built-in mode semantics. Policies are resolved at `SessionStart` and evaluated at `Commitment` time.

## Policy Registry

The policy registry (`src/policy/registry.rs`) is an in-memory store of `PolicyDefinition` objects.

- **Default policy**: `policy.default` is always pre-loaded (mode `*`, empty rules, no constraints)
- **Registration**: `RegisterPolicy` gRPC RPC; validates rules against mode-specific JSON schema
- **Unregistration**: `UnregisterPolicy`; does not affect active sessions
- **Query**: `GetPolicy`, `ListPolicies` (with optional mode filter)
- **Watch**: `WatchPolicies` streams notifications on registry changes

### Conditional Validation

At registration time, the registry validates:
- `voting.algorithm == "weighted"` requires non-empty `voting.weights`
- `voting.algorithm == "supermajority"` requires `voting.threshold > 0.5`
- `commitment.authority == "designated_role"` requires non-empty `commitment.designated_roles`

## Policy Resolution (SessionStart)

When a `SessionStart` is processed (`src/runtime.rs`):

1. Extract `policy_version` from `SessionStartPayload`
2. If empty, resolve to `"policy.default"`
3. Look up in policy registry; fail with `UNKNOWN_POLICY_VERSION` if not found
4. Verify mode match: policy `mode` must be `"*"` or match session mode
5. Store the resolved `PolicyDefinition` immutably on the `Session` struct

## Policy Evaluation (Commitment)

When a `Commitment` message is processed, each mode calls its evaluator (`src/policy/evaluator.rs`):

| Mode | Evaluator | Checks |
|------|-----------|--------|
| Decision | `evaluate_decision_commitment()` | Evaluation confidence, objection veto, quorum, voting threshold |
| Proposal | `evaluate_proposal_commitment()` | Counter-proposal round limit |
| Task | `evaluate_task_commitment()` | Output requirement |
| Handoff | `evaluate_handoff_commitment()` | Always allows (implicit timeout handled by mode) |
| Quorum | `evaluate_quorum_commitment()` | Threshold, abstention interpretation |

### Determinism

Policy evaluation is a **pure function** of: resolved rules + accepted message history + declared participants. No wall-clock time, external calls, or randomness.

## Per-Mode Rule Schemas

Rule schemas are defined in `src/policy/rules.rs` as serde structs:

- `DecisionPolicyRules`: voting, objection_handling, evaluation, commitment
- `QuorumPolicyRules`: threshold, abstention, commitment
- `ProposalPolicyRules`: acceptance, counter_proposal, rejection, commitment
- `TaskPolicyRules`: assignment, completion, commitment
- `HandoffPolicyRules`: acceptance, commitment

## Replay Invariant

The resolved `PolicyDefinition` is persisted in the session snapshot. During replay, the stored descriptor is used — never re-resolved from the registry. This ensures deterministic outcomes across time.

## Error Codes

| Code | HTTP | When |
|------|------|------|
| `UNKNOWN_POLICY_VERSION` | 404 | Policy not found at SessionStart |
| `POLICY_DENIED` | 403 | Commitment rejected by governance rules |
| `INVALID_POLICY_DEFINITION` | 400 | Policy fails validation at registration |

## References

- RFC-MACP-0012: Governance Policy Framework
- RFC-MACP-0001 Section 7.3: Session lifecycle
- RFC-MACP-0003: Determinism and replay integrity
