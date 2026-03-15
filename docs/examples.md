# Examples and Usage

This document provides step-by-step examples of using the MACP Runtime v0.3. It covers the full lifecycle — from protocol handshake to session creation, decision-making, convergence, cancellation, and error handling — with detailed explanations of what happens at each step.

---

## Table of Contents

1. [Quick Start](#quick-start)
2. [Example 1: Basic Decision Mode Client](#example-1-basic-decision-mode-client)
3. [Example 2: Full Decision Mode Lifecycle](#example-2-full-decision-mode-lifecycle)
4. [Example 3: Multi-Round Convergence](#example-3-multi-round-convergence)
5. [Example 4: Fuzz Client — Testing Every Error Path](#example-4-fuzz-client--testing-every-error-path)
6. [Example 5: Using the New RPCs](#example-5-using-the-new-rpcs)
7. [Example 6: Session Cancellation](#example-6-session-cancellation)
8. [Example 7: Message Deduplication](#example-7-message-deduplication)
9. [Example 8: Participant Validation](#example-8-participant-validation)
10. [Example 9: Signal Messages](#example-9-signal-messages)
11. [Example 10: Session with Custom TTL](#example-10-session-with-custom-ttl)
12. [Example 11: Multi-Agent Scenario](#example-11-multi-agent-scenario)
13. [Common Patterns](#common-patterns)
14. [Common Questions](#common-questions)

---

## Quick Start

### Running the Server

**Terminal 1:**
```bash
cd /path/to/runtime
cargo run
```

You should see:
```
macp-runtime v0.3.0 (RFC-0001) listening on 127.0.0.1:50051
```

The server is now ready to accept connections on port 50051.

### Running the Test Clients

**Terminal 2** — Basic demo:
```bash
cargo run --bin client
```

**Terminal 2** — Comprehensive error testing:
```bash
cargo run --bin fuzz_client
```

**Terminal 2** — Multi-round convergence:
```bash
cargo run --bin multi_round_client
```

---

## Example 1: Basic Decision Mode Client

The basic client (`src/bin/client.rs`) demonstrates the core happy path: Initialize, ListModes, SessionStart, Message, Resolve, post-resolve rejection, and GetSession.

### Step 1: Connect to the Server

```rust
let mut client = MacpRuntimeServiceClient::connect("http://127.0.0.1:50051").await?;
```

This creates a gRPC client and connects to the runtime. If the server isn't running, this will fail with a connection error.

### Step 2: Initialize — Negotiate Protocol Version

```rust
let init_resp = client
    .initialize(InitializeRequest {
        supported_protocol_versions: vec!["1.0".into()],
        client_info: None,
        capabilities: None,
    })
    .await?
    .into_inner();
println!(
    "Initialize: version={} runtime={}",
    init_resp.selected_protocol_version,
    init_resp.runtime_info.as_ref().map(|r| r.name.as_str()).unwrap_or("?")
);
```

**What happens:** The client proposes protocol version `"1.0"`. The server confirms it and returns runtime info (name, version), capabilities, and the list of supported modes.

**Expected output:**
```
Initialize: version=1.0 runtime=macp-runtime
```

### Step 3: Discover Available Modes

```rust
let modes_resp = client.list_modes(ListModesRequest {}).await?.into_inner();
println!(
    "ListModes: {:?}",
    modes_resp.modes.iter().map(|m| &m.mode).collect::<Vec<_>>()
);
```

**Expected output:**
```
ListModes: ["macp.mode.decision.v1"]
```

### Step 4: Create a Session (SessionStart)

```rust
let start = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "SessionStart".into(),
    message_id: "m1".into(),
    session_id: "s1".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: 1_700_000_000_000,
    payload: vec![],
};

let ack = client
    .send(SendRequest { envelope: Some(start) })
    .await?
    .into_inner()
    .ack
    .unwrap();
println!("SessionStart ack: ok={} error={:?}", ack.ok, ack.error.as_ref().map(|e| &e.code));
```

**What each field means:**
- `macp_version: "1.0"` — Protocol version (must be exactly `"1.0"`).
- `mode: "decision"` — Use the Decision Mode (alias for `"macp.mode.decision.v1"`).
- `message_type: "SessionStart"` — This creates a new session.
- `message_id: "m1"` — Unique ID for this message.
- `session_id: "s1"` — The session ID we're creating.
- `sender: "ajit"` — Who is sending this message.
- `payload: vec![]` — Empty payload means default TTL (60s), no participants.

**Expected output:**
```
SessionStart ack: ok=true error=None
```

### Step 5: Send a Normal Message

```rust
let msg = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "Message".into(),
    message_id: "m2".into(),
    session_id: "s1".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: 1_700_000_000_001,
    payload: b"hello".to_vec(),
};
```

In the Decision Mode, a `Message` with a non-`"resolve"` payload returns `NoOp` — the message is accepted but produces no state change. This is the backward-compatible behavior from v0.1.

**Expected output:**
```
Message ack: ok=true error=None
```

### Step 6: Resolve the Session (Legacy)

```rust
let resolve = Envelope {
    // ...
    message_type: "Message".into(),
    message_id: "m3".into(),
    payload: b"resolve".to_vec(),  // Legacy resolve trigger
};
```

When the Decision Mode sees `message_type: "Message"` with `payload == b"resolve"`, it resolves the session immediately. This is the backward-compatible resolution mechanism from v0.1.

**Expected output:**
```
Resolve ack: ok=true error=None
```

### Step 7: Attempt Message After Resolution

```rust
let after = Envelope {
    // ...
    message_type: "Message".into(),
    message_id: "m4".into(),
    payload: b"should-fail".to_vec(),
};
```

The session is now `Resolved` (terminal state). Any further message is rejected with `SESSION_NOT_OPEN`.

**Expected output:**
```
After-resolve ack: ok=false error=Some("SESSION_NOT_OPEN")
```

### Step 8: Verify State with GetSession

```rust
let resp = client
    .get_session(GetSessionRequest { session_id: "s1".into() })
    .await?
    .into_inner();
let meta = resp.metadata.unwrap();
println!("GetSession: state={} mode={}", meta.state, meta.mode);
```

**Expected output:**
```
GetSession: state=2 mode=decision
```

(State `2` is `SESSION_STATE_RESOLVED` in the protobuf enum.)

---

## Example 2: Full Decision Mode Lifecycle

The Decision Mode in v0.3 supports a rich lifecycle: Proposal, Evaluation, Objection, Vote, and Commitment. Here is how a complete decision process flows:

### Step 1: Create a Session

```rust
let start = Envelope {
    macp_version: "1.0".into(),
    mode: "macp.mode.decision.v1".into(),  // RFC-compliant name
    message_type: "SessionStart".into(),
    message_id: uuid::Uuid::new_v4().to_string(),
    session_id: "decision-001".into(),
    sender: "coordinator".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: vec![],
};
```

### Step 2: Submit a Proposal

```rust
let proposal = Envelope {
    macp_version: "1.0".into(),
    mode: "macp.mode.decision.v1".into(),
    message_type: "Proposal".into(),
    message_id: uuid::Uuid::new_v4().to_string(),
    session_id: "decision-001".into(),
    sender: "agent-alpha".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "p1",
        "option": "Deploy v2.1 to production",
        "rationale": "All integration tests pass, staging validation complete",
        "supporting_data": ""
    })).unwrap(),
};
```

After this message, the Decision Mode's phase advances from `Proposal` to `Evaluation`.

### Step 3: Submit an Evaluation

```rust
let evaluation = Envelope {
    // ...
    message_type: "Evaluation".into(),
    sender: "agent-beta".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "p1",
        "recommendation": "APPROVE",
        "confidence": 0.92,
        "reason": "Performance metrics look excellent"
    })).unwrap(),
};
```

Evaluations are appended to the state — multiple agents can evaluate the same proposal.

### Step 4: Raise an Objection (Optional)

```rust
let objection = Envelope {
    // ...
    message_type: "Objection".into(),
    sender: "agent-gamma".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "p1",
        "reason": "Security audit pending for the new auth module",
        "severity": "medium"
    })).unwrap(),
};
```

Objections are recorded but do not block the decision process — they are informational.

### Step 5: Cast Votes

```rust
let vote = Envelope {
    // ...
    message_type: "Vote".into(),
    sender: "agent-alpha".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "p1",
        "vote": "approve",
        "reason": "Objection addressed in patch v2.1.1"
    })).unwrap(),
};
```

Votes are keyed by sender — if the same sender votes again, the previous vote is overwritten. The phase advances to `Voting`.

### Step 6: Commit the Decision

```rust
let commitment = Envelope {
    // ...
    message_type: "Commitment".into(),
    sender: "coordinator".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "commitment_id": "c1",
        "action": "deploy-v2.1.1",
        "authority_scope": "team-alpha",
        "reason": "Unanimous approval with addressed objection"
    })).unwrap(),
};
```

The `Commitment` message finalizes the decision:
- The phase advances to `Committed`.
- The session resolves with the commitment payload as the resolution.
- No further messages are accepted.

### Full Phase Progression

```
Proposal → Evaluation → Voting → Committed (resolved)
```

---

## Example 3: Multi-Round Convergence

The multi-round client (`src/bin/multi_round_client.rs`) demonstrates participant-based convergence.

### Run the Demo

```bash
cargo run --bin multi_round_client
```

### Expected Output

```
=== Multi-Round Convergence Demo ===

[session_start] ok=true error=''
[alice_contributes_a] ok=true error=''
[bob_contributes_b] ok=true error=''
[get_session] state=1 mode=multi_round
[bob_revises_to_a] ok=true error=''
[get_session] state=2 mode_version=
[after_convergence] ok=false error='SESSION_NOT_OPEN'

=== Demo Complete ===
```

### Step-by-Step Walkthrough

#### 1. Create a Multi-Round Session

```rust
let start_payload = SessionStartPayload {
    intent: "convergence test".into(),
    ttl_ms: 60000,
    participants: vec!["alice".into(), "bob".into()],
    mode_version: String::new(),
    configuration_version: String::new(),
    policy_version: String::new(),
    context: vec![],
    roots: vec![],
};

let start = Envelope {
    macp_version: "1.0".into(),
    mode: "multi_round".into(),
    message_type: "SessionStart".into(),
    message_id: "m0".into(),
    session_id: "mr1".into(),
    sender: "coordinator".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: start_payload.encode_to_vec(),  // Protobuf-encoded
};
```

**Key points:**
- The `participants` field declares that `alice` and `bob` are the expected contributors.
- The payload is protobuf-encoded (using `prost::Message::encode_to_vec()`), not JSON.
- The `intent` field provides a human-readable description.

#### 2. Alice Contributes "option_a"

```rust
let contribute = Envelope {
    macp_version: "1.0".into(),
    mode: "multi_round".into(),
    message_type: "Contribute".into(),
    message_id: "m1".into(),
    session_id: "mr1".into(),
    sender: "alice".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: br#"{"value":"option_a"}"#.to_vec(),
};
```

**State after:** Round 1. Contributions: `{alice: "option_a"}`. Bob hasn't contributed yet — no convergence.

#### 3. Bob Contributes "option_b" (Divergence)

```rust
let contribute = Envelope {
    // ...
    sender: "bob".into(),
    payload: br#"{"value":"option_b"}"#.to_vec(),
};
```

**State after:** Round 2. Contributions: `{alice: "option_a", bob: "option_b"}`. All participants have contributed but values differ — no convergence.

#### 4. Query Session State — Still Open

```rust
let resp = client.get_session(GetSessionRequest { session_id: "mr1".into() }).await?;
// state=1 (SESSION_STATE_OPEN)
```

#### 5. Bob Revises to "option_a" (Convergence!)

```rust
let contribute = Envelope {
    // ...
    sender: "bob".into(),
    payload: br#"{"value":"option_a"}"#.to_vec(),
};
```

**State after:** Round 3. Contributions: `{alice: "option_a", bob: "option_a"}`. All participants have contributed and all values are identical — **convergence reached!** The session auto-resolves with:

```json
{
  "converged_value": "option_a",
  "round": 3,
  "final_values": {
    "alice": "option_a",
    "bob": "option_a"
  }
}
```

#### 6. Attempt After Convergence — Rejected

```rust
let contribute = Envelope {
    // ...
    sender: "alice".into(),
    payload: br#"{"value":"option_c"}"#.to_vec(),
};
// Ack: ok=false, error="SESSION_NOT_OPEN"
```

The session is resolved — no further contributions are accepted.

### Why Round 3?

- Round 0: Session starts with no contributions.
- Round 1: Alice contributes "option_a" (new contribution).
- Round 2: Bob contributes "option_b" (new contribution).
- Round 3: Bob revises to "option_a" (value changed from "option_b").

Re-submitting the same value does **not** increment the round. Only substantive changes count.

---

## Example 4: Fuzz Client — Testing Every Error Path

The fuzz client (`src/bin/fuzz_client.rs`) is a comprehensive test that exercises every error code, every new RPC, and every edge case. Here's what it tests and what each test proves:

### Expected Output

```
[initialize] version=1.0
[initialize_bad_version] error: ...INVALID_ARGUMENT...
[wrong_version] ok=false duplicate=false error='UNSUPPORTED_PROTOCOL_VERSION'
[missing_fields] ok=false duplicate=false error='INVALID_ENVELOPE'
[unknown_session_message] ok=false duplicate=false error='SESSION_NOT_FOUND'
[session_start_ok] ok=true duplicate=false error=''
[session_start_duplicate] ok=false duplicate=false error='INVALID_ENVELOPE'
[session_start_idempotent] ok=true duplicate=true error=''
[message_ok] ok=true duplicate=false error=''
[message_duplicate] ok=true duplicate=true error=''
[resolve] ok=true duplicate=false error=''
[after_resolve] ok=false duplicate=false error='SESSION_NOT_OPEN'
[ttl_session_start] ok=true duplicate=false error=''
[ttl_expired_message] ok=false duplicate=false error='SESSION_NOT_OPEN'
[invalid_ttl_negative] ok=false duplicate=false error='INVALID_ENVELOPE'
[invalid_ttl_exceeds_max] ok=false duplicate=false error='INVALID_ENVELOPE'
[multi_round_start] ok=true duplicate=false error=''
[multi_round_alice] ok=true duplicate=false error=''
[multi_round_bob_diff] ok=true duplicate=false error=''
[multi_round_bob_converge] ok=true duplicate=false error=''
[multi_round_after_resolve] ok=false duplicate=false error='SESSION_NOT_OPEN'
[cancel_session_start] ok=true duplicate=false error=''
[cancel_session] ok=true
[after_cancel] ok=false duplicate=false error='SESSION_NOT_OPEN'
[participant_session_start] ok=true duplicate=false error=''
[unauthorized_sender] ok=false duplicate=false error='INVALID_ENVELOPE'
[authorized_sender] ok=true duplicate=false error=''
[signal] ok=true duplicate=false error=''
[get_session] state=2 mode=decision
[list_modes] count=1 modes=["macp.mode.decision.v1"]
[get_manifest] agent_id=macp-runtime modes=["macp.mode.decision.v1"]
[list_roots] count=0
```

### What Each Test Proves

| Test | What It Proves |
|------|---------------|
| `initialize` | Protocol handshake with version "1.0" succeeds |
| `initialize_bad_version` | Unsupported version "2.0" returns gRPC INVALID_ARGUMENT |
| `wrong_version` | Envelope with `macp_version: "v0"` is rejected |
| `missing_fields` | Empty `message_id` is rejected |
| `unknown_session_message` | Message to non-existent session returns SESSION_NOT_FOUND |
| `session_start_ok` | Valid SessionStart creates a session |
| `session_start_duplicate` | Second SessionStart with different message_id is rejected |
| `session_start_idempotent` | Same SessionStart with same message_id is idempotent (duplicate=true) |
| `message_ok` | Valid message to open session succeeds |
| `message_duplicate` | Same message_id is idempotent (duplicate=true) |
| `resolve` | Legacy `payload="resolve"` resolves the session |
| `after_resolve` | Message to resolved session returns SESSION_NOT_OPEN |
| `ttl_session_start` | Session with 1-second TTL is created |
| `ttl_expired_message` | Message after TTL expiry returns SESSION_NOT_OPEN |
| `invalid_ttl_negative` | Negative TTL is rejected |
| `invalid_ttl_exceeds_max` | TTL > 24h is rejected |
| `multi_round_*` | Full multi-round convergence cycle works |
| `cancel_session` | CancelSession transitions session to Expired |
| `after_cancel` | Message to cancelled session returns SESSION_NOT_OPEN |
| `participant_*` | Unauthorized sender is rejected, authorized sender succeeds |
| `signal` | Signal message with empty session_id succeeds |
| `get_session` | GetSession returns correct state |
| `list_modes` | ListModes returns both registered modes |
| `get_manifest` | GetManifest returns runtime identity and modes |
| `list_roots` | ListRoots returns empty list |

---

## Example 5: Using the New RPCs

### Initialize

```rust
let init_resp = client
    .initialize(InitializeRequest {
        supported_protocol_versions: vec!["1.0".into()],
        client_info: Some(ClientInfo {
            name: "my-agent".into(),
            title: "My Coordination Agent".into(),
            version: "1.0.0".into(),
            description: "An agent that coordinates deployments".into(),
            website_url: String::new(),
        }),
        capabilities: None,
    })
    .await?
    .into_inner();

println!("Selected version: {}", init_resp.selected_protocol_version);
println!("Runtime: {:?}", init_resp.runtime_info);
println!("Supported modes: {:?}", init_resp.supported_modes);
```

### ListModes

```rust
let modes = client.list_modes(ListModesRequest {}).await?.into_inner().modes;
for mode in &modes {
    println!("Mode: {} (v{})", mode.mode, mode.mode_version);
    println!("  Title: {}", mode.title);
    println!("  Message types: {:?}", mode.message_types);
    println!("  Terminal types: {:?}", mode.terminal_message_types);
    println!("  Determinism: {}", mode.determinism_class);
    println!("  Participant model: {}", mode.participant_model);
}
```

### GetManifest

```rust
let manifest = client
    .get_manifest(GetManifestRequest { agent_id: String::new() })
    .await?
    .into_inner()
    .manifest
    .unwrap();

println!("Runtime: {} - {}", manifest.agent_id, manifest.description);
println!("Supported modes: {:?}", manifest.supported_modes);
```

### GetSession

```rust
let metadata = client
    .get_session(GetSessionRequest { session_id: "s1".into() })
    .await?
    .into_inner()
    .metadata
    .unwrap();

println!("Session: {}", metadata.session_id);
println!("Mode: {}", metadata.mode);
println!("State: {} (1=Open, 2=Resolved, 3=Expired)", metadata.state);
println!("Started at: {}", metadata.started_at_unix_ms);
println!("Expires at: {}", metadata.expires_at_unix_ms);
println!("Mode version: {}", metadata.mode_version);
```

---

## Example 6: Session Cancellation

```rust
// Create a session
let start = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "SessionStart".into(),
    message_id: "m_c1".into(),
    session_id: "s_cancel".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: vec![],
};
client.send(SendRequest { envelope: Some(start) }).await?;

// Cancel the session
let cancel_resp = client
    .cancel_session(CancelSessionRequest {
        session_id: "s_cancel".into(),
        reason: "User requested cancellation".into(),
    })
    .await?
    .into_inner();

let ack = cancel_resp.ack.unwrap();
println!("Cancel: ok={}", ack.ok);  // ok=true

// Try to send a message — rejected
let msg = Envelope {
    // ...
    session_id: "s_cancel".into(),
    message_id: "m_c2".into(),
    payload: b"should-fail".to_vec(),
    // ...
};
let ack = client.send(SendRequest { envelope: Some(msg) }).await?.into_inner().ack.unwrap();
println!("After cancel: ok={} error={}", ack.ok, ack.error.unwrap().code);
// ok=false error=SESSION_NOT_OPEN
```

**Key behaviors:**
- Cancelling an open session transitions it to `Expired` and logs the reason.
- Cancelling an already resolved or expired session is idempotent — returns `ok: true`.
- Messages to a cancelled session are rejected with `SESSION_NOT_OPEN`.

---

## Example 7: Message Deduplication

The runtime deduplicates messages by `message_id` within a session:

```rust
// Send a message
let msg = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "Message".into(),
    message_id: "m_dedup".into(),
    session_id: "s1".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: b"hello".to_vec(),
};

// First send — accepted normally
let ack1 = client.send(SendRequest { envelope: Some(msg.clone()) }).await?.into_inner().ack.unwrap();
println!("First: ok={} duplicate={}", ack1.ok, ack1.duplicate);
// ok=true duplicate=false

// Second send (same message_id) — idempotent duplicate
let ack2 = client.send(SendRequest { envelope: Some(msg.clone()) }).await?.into_inner().ack.unwrap();
println!("Second: ok={} duplicate={}", ack2.ok, ack2.duplicate);
// ok=true duplicate=true
```

**Why this matters:** Network retries are safe. If a client isn't sure whether a message was received (e.g., timeout on the response), it can safely resend with the same `message_id`. The runtime will recognize it as a duplicate and return success without re-processing.

This also works for `SessionStart`:

```rust
// Same SessionStart with same message_id is idempotent
let start = Envelope {
    message_type: "SessionStart".into(),
    message_id: "m1".into(),
    session_id: "s1".into(),
    // ...
};

// If s1 was already created with message_id "m1":
let ack = client.send(SendRequest { envelope: Some(start) }).await?.into_inner().ack.unwrap();
// ok=true, duplicate=true
```

---

## Example 8: Participant Validation

Sessions can restrict which senders are allowed:

```rust
// Create a session with a participant list
let start_payload = SessionStartPayload {
    participants: vec!["alice".into(), "bob".into()],
    ttl_ms: 60000,
    ..Default::default()
};

let start = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "SessionStart".into(),
    message_id: "m_p1".into(),
    session_id: "s_participant".into(),
    sender: "alice".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: start_payload.encode_to_vec(),
};
client.send(SendRequest { envelope: Some(start) }).await?;

// Charlie tries to send — REJECTED (not a participant)
let msg = Envelope {
    // ...
    session_id: "s_participant".into(),
    sender: "charlie".into(),
    message_id: "m_p2".into(),
    // ...
};
let ack = client.send(SendRequest { envelope: Some(msg) }).await?.into_inner().ack.unwrap();
println!("Charlie: ok={}", ack.ok);  // ok=false (INVALID_ENVELOPE)

// Alice sends — ACCEPTED (is a participant)
let msg = Envelope {
    // ...
    session_id: "s_participant".into(),
    sender: "alice".into(),
    message_id: "m_p3".into(),
    // ...
};
let ack = client.send(SendRequest { envelope: Some(msg) }).await?.into_inner().ack.unwrap();
println!("Alice: ok={}", ack.ok);  // ok=true
```

---

## Example 9: Signal Messages

Signal messages are ambient, session-less messages:

```rust
let signal = Envelope {
    macp_version: "1.0".into(),
    mode: String::new(),            // mode is optional for signals
    message_type: "Signal".into(),
    message_id: "sig1".into(),
    session_id: String::new(),      // session_id can be empty!
    sender: "alice".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: vec![],
};

let ack = client.send(SendRequest { envelope: Some(signal) }).await?.into_inner().ack.unwrap();
println!("Signal: ok={}", ack.ok);  // ok=true
```

**Key points:**
- `session_id` may be empty — signals don't belong to any session.
- No session is created or modified.
- The runtime simply acknowledges receipt.
- Useful for heartbeats, coordination hints, or cross-session correlation.

---

## Example 10: Session with Custom TTL

```rust
// Create a session with a 1-second TTL
let start_payload = SessionStartPayload {
    ttl_ms: 1000,  // 1 second
    ..Default::default()
};

let start = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "SessionStart".into(),
    message_id: "m_ttl1".into(),
    session_id: "s_ttl".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: start_payload.encode_to_vec(),
};

client.send(SendRequest { envelope: Some(start) }).await?;

// Wait for TTL to expire
tokio::time::sleep(Duration::from_millis(1200)).await;

// This message will be rejected — session expired
let msg = Envelope {
    macp_version: "1.0".into(),
    mode: "decision".into(),
    message_type: "Message".into(),
    message_id: "m_ttl2".into(),
    session_id: "s_ttl".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
    payload: b"too-late".to_vec(),
};

let ack = client.send(SendRequest { envelope: Some(msg) }).await?.into_inner().ack.unwrap();
println!("After TTL: ok={} error={}", ack.ok, ack.error.unwrap().code);
// ok=false error=SESSION_NOT_OPEN
```

**TTL rules:**
- `ttl_ms: 0` (or absent) → default 60 seconds.
- `ttl_ms: 1` to `86,400,000` → custom TTL.
- Negative or > 24h → rejected with `INVALID_ENVELOPE`.
- TTL is enforced lazily on the next message — no background cleanup.

---

## Example 11: Multi-Agent Scenario

Imagine three agents coordinating a deployment decision using the full Decision Mode lifecycle:

### Phase 1: Setup

```rust
// Coordinator creates a decision session with participant list
let start_payload = SessionStartPayload {
    intent: "Decide on v3.0 release strategy".into(),
    participants: vec!["lead".into(), "security".into(), "ops".into()],
    ttl_ms: 300000,  // 5 minutes
    ..Default::default()
};

// ... send SessionStart ...
```

### Phase 2: Proposal

```rust
// Lead agent proposes a deployment strategy
let proposal = Envelope {
    message_type: "Proposal".into(),
    sender: "lead".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "release-v3",
        "option": "Blue-green deployment with 10% canary",
        "rationale": "Minimizes risk while allowing quick rollback"
    })).unwrap(),
    // ... other fields ...
};
```

### Phase 3: Evaluation + Objection

```rust
// Security agent evaluates
let eval = Envelope {
    message_type: "Evaluation".into(),
    sender: "security".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "release-v3",
        "recommendation": "REVIEW",
        "confidence": 0.75,
        "reason": "Need to verify WAF rules for new endpoints"
    })).unwrap(),
    // ...
};

// Security agent raises objection
let objection = Envelope {
    message_type: "Objection".into(),
    sender: "security".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "proposal_id": "release-v3",
        "reason": "New /admin endpoint lacks rate limiting",
        "severity": "high"
    })).unwrap(),
    // ...
};
```

### Phase 4: Voting

```rust
// All agents vote after objection is addressed
for (sender, vote) in [("lead", "approve"), ("security", "approve"), ("ops", "approve")] {
    let vote_msg = Envelope {
        message_type: "Vote".into(),
        sender: sender.into(),
        payload: serde_json::to_vec(&serde_json::json!({
            "proposal_id": "release-v3",
            "vote": vote,
            "reason": "Objection addressed in hotfix"
        })).unwrap(),
        // ...
    };
    client.send(SendRequest { envelope: Some(vote_msg) }).await?;
}
```

### Phase 5: Commitment

```rust
// Lead commits the decision — session resolves
let commitment = Envelope {
    message_type: "Commitment".into(),
    sender: "lead".into(),
    payload: serde_json::to_vec(&serde_json::json!({
        "commitment_id": "release-v3-commit",
        "action": "deploy-blue-green-canary",
        "authority_scope": "release-team",
        "reason": "Unanimous approval after security review"
    })).unwrap(),
    // ...
};
```

The session is now `Resolved` with the commitment as the resolution.

---

## Common Patterns

### Pattern 1: Structured Error Handling

```rust
let ack = client.send(SendRequest { envelope: Some(env) }).await?.into_inner().ack.unwrap();

if ack.ok {
    if ack.duplicate {
        println!("Idempotent duplicate — already processed");
    } else {
        println!("Success! Session state: {:?}", ack.session_state);
    }
} else {
    let err = ack.error.unwrap();
    match err.code.as_str() {
        "UNSUPPORTED_PROTOCOL_VERSION" => println!("Use macp_version: 1.0"),
        "INVALID_ENVELOPE" => println!("Check required fields and payload format"),
        "SESSION_NOT_FOUND" => println!("Session doesn't exist — send SessionStart first"),
        "SESSION_NOT_OPEN" => println!("Session is resolved or expired"),
        "MODE_NOT_SUPPORTED" => println!("Use a registered mode name"),
        code => println!("Error {}: {}", code, err.message),
    }
}
```

### Pattern 2: Protobuf-Encoded SessionStart Payload

```rust
use macp_runtime::pb::SessionStartPayload;
use prost::Message;

let payload = SessionStartPayload {
    intent: "My coordination task".into(),
    participants: vec!["agent-a".into(), "agent-b".into()],
    ttl_ms: 120000,  // 2 minutes
    mode_version: "1.0.0".into(),
    configuration_version: String::new(),
    policy_version: String::new(),
    context: vec![],
    roots: vec![],
};

let start = Envelope {
    // ...
    payload: payload.encode_to_vec(),
};
```

### Pattern 3: Unique Message IDs with UUIDs

```rust
use uuid::Uuid;

let envelope = Envelope {
    message_id: Uuid::new_v4().to_string(),
    // ... other fields
};
```

### Pattern 4: Current Timestamp

```rust
use chrono::Utc;

let envelope = Envelope {
    timestamp_unix_ms: Utc::now().timestamp_millis(),
    // ... other fields
};
```

### Pattern 5: Helper Function

```rust
fn create_envelope(
    mode: &str,
    message_type: &str,
    session_id: &str,
    sender: &str,
    payload: &[u8],
) -> Envelope {
    Envelope {
        macp_version: "1.0".into(),
        mode: mode.into(),
        message_type: message_type.into(),
        message_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.into(),
        sender: sender.into(),
        timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
        payload: payload.to_vec(),
    }
}

// Usage:
let start = create_envelope("decision", "SessionStart", "s1", "alice", b"");
let vote = create_envelope("decision", "Vote", "s1", "alice",
    &serde_json::to_vec(&serde_json::json!({
        "proposal_id": "p1", "vote": "approve", "reason": "LGTM"
    })).unwrap()
);
let contribute = create_envelope("multi_round", "Contribute", "mr1", "alice",
    br#"{"value":"option_a"}"#
);
```

---

## Common Questions

### Q: What protocol version should I use?

**A:** Use `macp_version: "1.0"`. This is the only supported version in v0.3. Always call `Initialize` first to confirm.

### Q: How do I encode the SessionStart payload?

**A:** Use protobuf encoding. In Rust, create a `SessionStartPayload` and call `.encode_to_vec()`. In other languages, use the generated protobuf code for `macp.v1.SessionStartPayload`. An empty payload (zero bytes) is also valid and uses all defaults.

### Q: What's the difference between "decision" and "macp.mode.decision.v1"?

**A:** They refer to the same mode. `"decision"` is a backward-compatible alias; `"macp.mode.decision.v1"` is the RFC-compliant canonical name. Both work identically. The same applies to `"multi_round"` and `"macp.mode.multi_round.v1"`.

### Q: Can I send messages from different senders to the same session?

**A:** Yes, if the session has no participant list (open participation). If the session has a participant list, only listed senders can send.

### Q: What if I use the same message_id twice?

**A:** The runtime treats it as an idempotent duplicate — it returns `ok: true, duplicate: true` without re-processing. This is by design for safe retries.

### Q: Can I create multiple sessions with the same ID?

**A:** No. The second SessionStart (with a different message_id) will be rejected with `INVALID_ENVELOPE`. However, resending the same SessionStart (same message_id) is idempotent and returns success.

### Q: How long do sessions last?

**A:** Sessions have a configurable TTL (default 60 seconds, max 24 hours). Set `ttl_ms` in the `SessionStartPayload`.

### Q: Can I cancel a session?

**A:** Yes. Use the `CancelSession` RPC with a session_id and reason. The session transitions to Expired and the reason is logged.

### Q: Can I "unresolve" a session?

**A:** No. Resolved and Expired are terminal states. Create a new session if you need to continue coordination.

### Q: What happens if the server crashes?

**A:** All session state is lost (it's in-memory only). Clients would need to reconnect and restart sessions. Future versions may add persistent storage.

### Q: What modes are available?

**A:** Two modes:
- `macp.mode.decision.v1` (alias: `decision`) — RFC lifecycle with Proposal/Evaluation/Objection/Vote/Commitment.
- `macp.mode.multi_round.v1` (alias: `multi_round`) — Participant-based convergence.

Use `ListModes` to discover them at runtime.

### Q: How do Signal messages work?

**A:** Signals are fire-and-forget messages that don't require a session_id. They don't create or modify sessions. They're useful for heartbeats, hints, or cross-session correlation.

### Q: What's the difference between the old "resolve" payload and the new Commitment message?

**A:** The old mechanism (sending `payload: b"resolve"` with `message_type: "Message"`) still works for backward compatibility. The new `Commitment` message type is richer — it carries a `CommitmentPayload` with fields like `commitment_id`, `action`, `authority_scope`, and `reason`. Both resolve the session, but the new mechanism provides much more context in the resolution.

---

## Next Steps

Now that you've seen examples, you can:

1. **Build your own client** — in Python, JavaScript, Go, or any language with a gRPC client. Use the `.proto` files to generate client code.
2. **Explore the Decision Mode lifecycle** — try building a multi-agent voting system.
3. **Implement convergence scenarios** — use Multi-Round Mode for consensus-building.
4. **Extend the protocol** — add new modes by implementing the `Mode` trait.
5. **Integrate with your agent framework** — use the `Initialize` and `ListModes` RPCs for dynamic discovery.

For deeper understanding:
- Read **[architecture.md](./architecture.md)** to see how it's implemented internally.
- Read **[protocol.md](./protocol.md)** for the complete protocol specification.
