# Examples and local development guide

These examples target `macp-runtime v0.4.0` and the stream-capable freeze profile.

They intentionally use the local-development security shortcut:

```bash
export MACP_ALLOW_INSECURE=1
export MACP_ALLOW_DEV_SENDER_HEADER=1
cargo run
```

The example binaries in `src/bin` attach `x-macp-agent-id` metadata so the runtime can derive the authenticated sender.

## Ground rules for every standards-track session

For these modes:

- `macp.mode.decision.v1`
- `macp.mode.proposal.v1`
- `macp.mode.task.v1`
- `macp.mode.handoff.v1`
- `macp.mode.quorum.v1`

`SessionStartPayload` must include all of the following:

- `participants`
- `mode_version`
- `configuration_version`
- positive `ttl_ms`

The example clients use:

- `mode_version = "1.0.0"`
- `configuration_version = "config.default"`
- `policy_version = "policy.default"`

## Example 1: Decision Mode

Run:

```bash
cargo run --bin client
```

Flow:

1. `Initialize`
2. `ListModes`
3. `SessionStart` by `coordinator`
4. `Proposal` by `coordinator`
5. `Evaluation` by a participant
6. `Vote` by a participant
7. `Commitment` by `coordinator`
8. `GetSession`

Important runtime behavior:

- initiator/coordinator may emit `Proposal` and `Commitment`
- declared participants may also emit `Proposal`, `Evaluation`, `Objection`, and `Vote`
- duplicate proposal IDs are rejected
- votes are tracked per proposal, per sender
- `CommitmentPayload` version fields must match the bound session versions

## Example 2: Proposal Mode

Run:

```bash
cargo run --bin proposal_client
```

Flow:

1. buyer starts the session
2. seller creates a proposal
3. buyer counters
4. both required participants accept the same live proposal
5. buyer emits `Commitment`

Important runtime behavior:

- a proposal session does **not** resolve merely because a proposal exists
- `Commitment` is accepted only after acceptance convergence or a terminal rejection

## Example 3: Task Mode

Run:

```bash
cargo run --bin task_client
```

Flow:

1. planner starts the session
2. planner sends `TaskRequest`
3. worker sends `TaskAccept`
4. worker sends `TaskUpdate`
5. worker sends `TaskComplete`
6. planner emits `Commitment`

## Example 4: Handoff Mode

Run:

```bash
cargo run --bin handoff_client
```

Flow:

1. owner starts the session
2. owner sends `HandoffOffer`
3. owner sends `HandoffContext`
4. target sends `HandoffAccept`
5. owner emits `Commitment`

## Example 5: Quorum Mode

Run:

```bash
cargo run --bin quorum_client
```

Flow:

1. coordinator starts the session
2. coordinator sends `ApprovalRequest`
3. participants send ballots
4. coordinator emits `Commitment` after threshold is satisfied

## Example 6: Experimental multi-round mode

Run:

```bash
cargo run --bin multi_round_client
```

This mode is still experimental. It remains callable by the explicit canonical name `macp.mode.multi_round.v1`, but it is not advertised by discovery RPCs and it does not use the strict standards-track `SessionStart` contract.

## Example 7: StreamSession (disabled in freeze profile)

`StreamSession` is disabled in the unary-first freeze profile. The `Initialize` response advertises `stream: false` and the RPC returns `UNIMPLEMENTED`. The implementation is retained for future activation.

When enabled, `StreamSession` emits only accepted canonical MACP envelopes. A single gRPC stream binds to one session. If a client needs negative per-message acknowledgements, it should continue to use `Send`.

Practical notes:

- bind a stream by sending a session-scoped envelope for the target session
- use `SessionStart` to create a new session over the stream
- stream attachment starts observing future accepted envelopes from the bind point; it does not replay earlier history
- use a session-scoped `Signal` envelope with the correct `session_id` and `mode` to attach to an existing session without mutating it
- mixed-session streams are rejected with `FAILED_PRECONDITION`

## Example 8: Freeze-check / error-path client

Run:

```bash
cargo run --bin fuzz_client
```

This client exercises common failure paths for the freeze profile, including:

- invalid protocol version
- empty mode
- invalid payloads
- duplicate messages
- unauthorized sender spoofing
- payload too large
- session access without membership

## Session ID policy

Session IDs must be either:

- **UUID v4/v7** in hyphenated lowercase canonical form (36 characters, e.g. `550e8400-e29b-41d4-a716-446655440000`)
- **Base64url token** of at least 22 characters using only `[A-Za-z0-9_-]`

Human-readable or short IDs (e.g. `"my-session"`, `"s1"`) are rejected with `INVALID_SESSION_ID`. The example clients generate UUID v4 session IDs automatically.

## Common troubleshooting

### `UNAUTHENTICATED`

Either send a bearer token that exists in the configured auth map, or start the runtime with:

```bash
export MACP_ALLOW_DEV_SENDER_HEADER=1
```

and ensure the client sets `x-macp-agent-id`.

### `INVALID_ENVELOPE` on `SessionStart`

For a standards-track mode, check that:

- the mode name is canonical
- the payload is not empty
- `mode_version` is present
- `configuration_version` is present
- `ttl_ms > 0`
- participants are present and unique

### `SESSION_NOT_OPEN`

The session is already resolved or expired. Use `GetSession` to confirm the terminal state.

### `RATE_LIMITED`

Increase the limits only if you understand the operational impact:

```bash
export MACP_SESSION_START_LIMIT_PER_MINUTE=120
export MACP_MESSAGE_LIMIT_PER_MINUTE=2000
```
