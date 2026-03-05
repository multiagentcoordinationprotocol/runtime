# Examples and Usage

This document provides step-by-step examples of using the MACP Runtime. Even if you don't know Rust, you can follow along and understand what's happening.

## Quick Start

### Running the Server

**Terminal 1:**
```bash
cd /path/to/runtime
cargo run
```

You should see:
```
macp-runtime v0.1 listening on 127.0.0.1:50051
```

The server is now ready to accept connections.

### Running the Test Client

**Terminal 2:**
```bash
cargo run --bin client
```

You should see output like:
```
SessionStart ack: accepted=true error=''
Message ack: accepted=true error=''
Resolve ack: accepted=true error=''
After-resolve ack: accepted=false error='SessionNotOpen'
```

**What happened?**
1. Client created a session (decision mode)
2. Client sent a normal message
3. Client sent a "resolve" message (session transitions to Resolved)
4. Client tried to send another message (rejected because session is Resolved)

## Example 1: Basic Client Walkthrough

Let's walk through the client code step by step (src/bin/client.rs).

### Step 1: Connect to the Server

```rust
let mut client = MacpServiceClient::connect("http://127.0.0.1:50051").await?;
```

**What this does:**
- Creates a gRPC client
- Connects to the server at `127.0.0.1:50051`
- If server isn't running, this will fail

### Step 2: Create an Envelope

```rust
let start = Envelope {
    macp_version: "v1".into(),
    mode: "decision".into(),
    message_type: "SessionStart".into(),
    message_id: "m1".into(),
    session_id: "s1".into(),
    sender: "ajit".into(),
    timestamp_unix_ms: 1_700_000_000_000,
    payload: vec![],
};
```

**What each field means:**
- `macp_version`: Protocol version (must be "v1")
- `mode`: Coordination mode ("decision" for simple resolve, "multi_round" for convergence)
- `message_type`: "SessionStart" to create a session
- `message_id`: Unique ID for this message ("m1")
- `session_id`: Which session ("s1")
- `sender`: Who's sending it ("ajit")
- `timestamp_unix_ms`: When sent (Unix timestamp in milliseconds)
- `payload`: Message content (empty for basic SessionStart)

### Step 3: Send and Receive Ack

```rust
let ack = client.send_message(start).await?.into_inner();
println!("SessionStart ack: accepted={} error='{}'", ack.accepted, ack.error);
```

### Step 4: Resolve the Session

```rust
let resolve = Envelope {
    // ...
    payload: b"resolve".to_vec(),  // Magic payload for decision mode
};
```

In decision mode, payload `"resolve"` triggers session resolution.

## Example 2: Multi-Round Convergence

The multi-round mode enables participant-based convergence. Run the demo:

**Terminal 2:**
```bash
cargo run --bin multi_round_client
```

**Expected output:**
```
=== Multi-Round Convergence Demo ===

[session_start] accepted=true error=''
[alice_contributes_a] accepted=true error=''
[bob_contributes_b] accepted=true error=''
[get_session] state=Open mode=multi_round participants=["alice", "bob"]
[bob_revises_to_a] accepted=true error=''
[get_session] state=Resolved resolution={"converged_value":"option_a","round":3,"final":{"alice":"option_a","bob":"option_a"}}
[after_convergence] accepted=false error='SessionNotOpen'

=== Demo Complete ===
```

### Step-by-Step Walkthrough

#### 1. Create a Multi-Round Session

```rust
let payload = serde_json::json!({
    "participants": ["alice", "bob"],
    "convergence": {"type": "all_equal"},
    "ttl_ms": 60000
});

let start = Envelope {
    macp_version: "v1".into(),
    mode: "multi_round".into(),
    message_type: "SessionStart".into(),
    message_id: "m0".into(),
    session_id: "mr1".into(),
    sender: "coordinator".into(),
    timestamp_unix_ms: Utc::now().timestamp_millis(),
    payload: payload.to_string().into_bytes(),
};
```

**SessionStart payload for multi_round mode:**
- `participants`: List of participant IDs who will contribute
- `convergence.type`: "all_equal" — resolve when all participants submit the same value
- `ttl_ms`: Optional TTL override

#### 2. Submit Contributions

```rust
let contribute = Envelope {
    macp_version: "v1".into(),
    mode: "multi_round".into(),
    message_type: "Contribute".into(),
    message_id: "m1".into(),
    session_id: "mr1".into(),
    sender: "alice".into(),
    timestamp_unix_ms: Utc::now().timestamp_millis(),
    payload: br#"{"value":"option_a"}"#.to_vec(),
};
```

Each participant sends a `Contribute` message with `{"value": "..."}` payload.

#### 3. Convergence

When all participants have contributed and all values are identical, the session auto-resolves with:
```json
{
  "converged_value": "option_a",
  "round": 3,
  "final": {
    "alice": "option_a",
    "bob": "option_a"
  }
}
```

#### 4. Revisions

Participants can revise their contributions. Each new value increments the round counter. Re-submitting the same value does not increment the round.

## Example 3: Fuzz Client (Testing Error Paths)

The fuzz client tests all the ways things can go wrong, including multi-round scenarios:

**Terminal 2:**
```bash
cargo run --bin fuzz_client
```

**Expected output:**
```
[wrong_version] accepted=false error='InvalidMacpVersion'
[missing_fields] accepted=false error='InvalidEnvelope'
[unknown_session_message] accepted=false error='UnknownSession'
[session_start_ok] accepted=true error=''
[session_start_duplicate] accepted=false error='DuplicateSession'
[message_ok] accepted=true error=''
[resolve] accepted=true error=''
[after_resolve] accepted=false error='SessionNotOpen'
[ttl_session_start] accepted=true error=''
[ttl_expired_message] accepted=false error='TtlExpired'
[invalid_ttl_zero] accepted=false error='InvalidTtl'
[invalid_ttl_negative] accepted=false error='InvalidTtl'
[invalid_ttl_exceeds_max] accepted=false error='InvalidTtl'
[multi_round_start] accepted=true error=''
[multi_round_alice] accepted=true error=''
[multi_round_bob_diff] accepted=true error=''
[multi_round_bob_converge] accepted=true error=''
[multi_round_after_resolve] accepted=false error='SessionNotOpen'
```

The last 5 lines show the multi-round scenario:
1. Create multi-round session with alice and bob
2. Alice contributes "option_a"
3. Bob contributes "option_b" (values differ, no convergence)
4. Bob revises to "option_a" (all equal → convergence → auto-resolved)
5. Alice tries to contribute after resolution → `SessionNotOpen`

## Example 4: Using GetSession

Query session state at any time:

```rust
use macp_runtime::pb::SessionQuery;

let info = client.get_session(SessionQuery {
    session_id: "s1".into(),
}).await?.into_inner();

println!("Session {} is in state {}", info.session_id, info.state);
println!("Mode: {}", info.mode);
println!("Participants: {:?}", info.participants);

if !info.resolution.is_empty() {
    println!("Resolution: {}", String::from_utf8_lossy(&info.resolution));
}
```

**Response fields:**
- `session_id`, `mode`, `state` ("Open"/"Resolved"/"Expired")
- `ttl_expiry` (Unix ms timestamp)
- `resolution` (bytes, empty if not resolved)
- `mode_state` (bytes, mode-specific internal state)
- `participants` (list of participant IDs)

If the session doesn't exist, the RPC returns a gRPC `NOT_FOUND` status.

## Example 5: Common Patterns

### Pattern 1: Error Handling

Always check the Ack:

```rust
let ack = client.send_message(envelope).await?.into_inner();

if ack.accepted {
    println!("Success!");
} else {
    match ack.error.as_str() {
        "InvalidMacpVersion" => println!("Use version v1"),
        "InvalidEnvelope" => println!("Check required fields"),
        "DuplicateSession" => println!("Session already exists"),
        "UnknownSession" => println!("Session doesn't exist"),
        "SessionNotOpen" => println!("Session is resolved/expired"),
        "TtlExpired" => println!("Session TTL has elapsed, create a new session"),
        "InvalidTtl" => println!("TTL must be 1..=86400000 ms"),
        "UnknownMode" => println!("Use 'decision' or 'multi_round'"),
        "InvalidModeState" => println!("Internal mode state error"),
        "InvalidPayload" => println!("Check mode-specific payload format"),
        _ => println!("Unknown error: {}", ack.error),
    }
}
```

### Pattern 2: Unique Message IDs

Use UUIDs for message IDs:

```rust
use uuid::Uuid;

let message_id = Uuid::new_v4().to_string();

let envelope = Envelope {
    message_id: message_id,
    // ... other fields
};
```

### Pattern 3: Current Timestamp

Use the current time for timestamps:

```rust
use chrono::Utc;

let timestamp = Utc::now().timestamp_millis();

let envelope = Envelope {
    timestamp_unix_ms: timestamp,
    // ... other fields
};
```

### Pattern 4: Helper Function

Create a helper to build envelopes:

```rust
fn create_envelope(
    mode: &str,
    message_type: &str,
    session_id: &str,
    sender: &str,
    payload: &[u8],
) -> Envelope {
    Envelope {
        macp_version: "v1".into(),
        mode: mode.into(),
        message_type: message_type.into(),
        message_id: Uuid::new_v4().to_string(),
        session_id: session_id.into(),
        sender: sender.into(),
        timestamp_unix_ms: Utc::now().timestamp_millis(),
        payload: payload.to_vec(),
    }
}

// Usage:
let start = create_envelope("decision", "SessionStart", "s1", "alice", b"");
let msg = create_envelope("decision", "Message", "s1", "alice", b"hello");
let contribute = create_envelope("multi_round", "Contribute", "mr1", "alice",
    br#"{"value":"option_a"}"#);
```

## Example 6: Multi-Agent Scenario

Imagine two agents coordinating via multi-round convergence:

### Coordinator starts the session

```rust
let payload = serde_json::json!({
    "participants": ["alpha", "beta"],
    "convergence": {"type": "all_equal"}
});

let start = create_envelope(
    "multi_round", "SessionStart", "decision-001", "coordinator",
    payload.to_string().as_bytes()
);
client.send_message(start).await?;
```

### Agent Alpha contributes

```rust
let contribute = create_envelope(
    "multi_round", "Contribute", "decision-001", "alpha",
    br#"{"value":"option_a"}"#
);
client.send_message(contribute).await?;
```

### Agent Beta contributes (different value)

```rust
let contribute = create_envelope(
    "multi_round", "Contribute", "decision-001", "beta",
    br#"{"value":"option_b"}"#
);
client.send_message(contribute).await?;
// Session still Open — values differ
```

### Agent Beta revises

```rust
let contribute = create_envelope(
    "multi_round", "Contribute", "decision-001", "beta",
    br#"{"value":"option_a"}"#
);
client.send_message(contribute).await?;
// Session auto-resolved! All participants agreed on "option_a"
```

### Check resolution

```rust
let info = client.get_session(SessionQuery {
    session_id: "decision-001".into(),
}).await?.into_inner();

assert_eq!(info.state, "Resolved");
// info.resolution contains {"converged_value":"option_a","round":3,"final":{...}}
```

## Example 7: Session with Custom TTL

You can configure a session's time-to-live by providing a JSON payload in `SessionStart`:

```rust
// Start a session with a 5-second TTL
let start = Envelope {
    macp_version: "v1".into(),
    mode: "decision".into(),
    message_type: "SessionStart".into(),
    message_id: "m1".into(),
    session_id: "s_ttl_demo".into(),
    sender: "agent-1".into(),
    timestamp_unix_ms: Utc::now().timestamp_millis(),
    payload: br#"{"ttl_ms": 5000}"#.to_vec(),
};

let ack = client.send_message(start).await?.into_inner();
assert!(ack.accepted); // Session created with 5s TTL

// Wait for TTL to expire
tokio::time::sleep(Duration::from_secs(6)).await;

// This message will be rejected with TtlExpired
let late_msg = Envelope {
    // ...
    payload: b"too late".to_vec(),
};

let ack = client.send_message(late_msg).await?.into_inner();
assert!(!ack.accepted);
assert_eq!(ack.error, "TtlExpired");
```

**TTL rules:**
- Empty payload → default 60-second TTL
- `{"ttl_ms": 5000}` → 5-second TTL
- `ttl_ms` must be between 1 and 86,400,000 (24 hours)
- Values outside this range are rejected with `InvalidTtl`

## Common Questions

### Q: Can I send messages from different senders to the same session?

**A:** Yes! The `sender` field is informational. Any sender can send to any session.

### Q: What if I use the same message_id twice?

**A:** The server doesn't currently check for duplicate message IDs. This is a validation you'd add in your client.

### Q: Can I create multiple sessions with the same ID?

**A:** No. The second SessionStart will be rejected with `DuplicateSession`.

### Q: How long do sessions last?

**A:** Sessions have a configurable TTL (default 60 seconds, max 24 hours). Specify a custom TTL by including `{"ttl_ms": <value>}` in the `SessionStart` payload.

### Q: Can I "unresolve" a session?

**A:** No. Resolved is a terminal state. You'd need to create a new session.

### Q: What happens if the server crashes?

**A:** All session state is lost (it's in-memory only). Clients would need to reconnect and restart sessions.

### Q: What modes are available?

**A:** Currently two modes:
- `decision` (default): Simple resolve-on-payload mode
- `multi_round`: Multi-round convergence with participant tracking

### Q: What happens if I use an empty mode field?

**A:** It defaults to `"decision"` for backward compatibility.

### Q: Can non-participants contribute in multi_round mode?

**A:** Currently yes — participant membership gating is planned for a future release.

## Next Steps

Now that you've seen examples, you can:

1. **Modify the test clients** - Try different scenarios
2. **Build your own client** - In Python, JavaScript, Go, etc.
3. **Add business logic** - Interpret payloads for your use case
4. **Extend the protocol** - Add new modes with the Mode trait

For deeper understanding:
- Read [architecture.md](./architecture.md) to see how it's implemented
- Read [protocol.md](./protocol.md) for the complete specification
