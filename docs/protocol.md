# MACP Protocol Specification

This document describes the Multi-Agent Coordination Protocol (MACP) in detail. We explain what each field means, what rules apply, and why they exist.

## Protocol Version

Current version: **v1**

All messages must specify `macp_version: "v1"` or they will be rejected.

## Core Concepts

### What is a Protocol?

A protocol is a set of rules that everyone agrees to follow. Like how English grammar has rules (subject-verb-object), the MACP protocol has rules for:
- What information must be included in each message
- What order things must happen
- What's allowed and what's forbidden

### Why Have a Protocol?

Without a protocol:
- Different agents might format messages differently
- State transitions could be inconsistent
- Errors would be ambiguous
- Debugging would be impossible

With a protocol:
- Everyone speaks the same "language"
- Behavior is predictable
- Tools can be built to work with any MACP-compliant system

## Message Types

### Envelope

Every message sent to the server is wrapped in an **Envelope**. Think of it as an addressed package.

**Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `macp_version` | string | Yes | Protocol version (must be "v1") |
| `mode` | string | Yes | Coordination mode (e.g., "decision", "multi_round") |
| `message_type` | string | Yes | Type of message ("SessionStart", "Message", "Contribute", etc.) |
| `message_id` | string | Yes | Unique ID for this message |
| `session_id` | string | Yes | Which session this belongs to |
| `sender` | string | Yes | Who is sending this message |
| `timestamp_unix_ms` | int64 | Yes | When sent (Unix timestamp in milliseconds) |
| `payload` | bytes | No | The actual message content |

**Example (JSON representation for readability):**
```json
{
  "macp_version": "v1",
  "mode": "decision",
  "message_type": "SessionStart",
  "message_id": "m1",
  "session_id": "s1",
  "sender": "agent-alpha",
  "timestamp_unix_ms": 1700000000000,
  "payload": ""
}
```

### Ack (Acknowledgment)

Every message receives an **Ack** response. It tells you if the message was accepted or rejected.

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `accepted` | bool | `true` if accepted, `false` if rejected |
| `error` | string | Empty if accepted, error name if rejected |

**Success example:**
```json
{
  "accepted": true,
  "error": ""
}
```

**Error example:**
```json
{
  "accepted": false,
  "error": "SessionNotOpen"
}
```

### SessionQuery / SessionInfo

The `GetSession` RPC allows querying session state:

**SessionQuery:**

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | The session to query |

**SessionInfo:**

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | Session identifier |
| `mode` | string | Coordination mode |
| `state` | string | "Open", "Resolved", or "Expired" |
| `ttl_expiry` | int64 | TTL expiry timestamp (Unix ms) |
| `resolution` | bytes | Resolution data (if resolved) |
| `mode_state` | bytes | Current mode-specific state |
| `participants` | repeated string | Session participants |

## Mode Dispatcher

The runtime uses a **Mode Dispatcher** to route messages to the appropriate coordination mode. The `mode` field in the Envelope determines which Mode handles the message.

### Available Modes

| Mode | Description | Default? |
|------|-------------|----------|
| `decision` | Simple resolve-on-payload mode | Yes (empty mode field defaults here) |
| `multi_round` | Multi-round convergence with participant tracking | No |

### How Modes Work

1. Runtime receives an Envelope
2. Resolves the mode name (empty string → "decision")
3. Looks up the registered Mode implementation
4. Calls `mode.on_session_start()` or `mode.on_message()`
5. Mode returns a `ModeResponse` (`NoOp`, `PersistState`, `Resolve`, `PersistAndResolve`)
6. Runtime applies the response to mutate session state

Modes are **pure logic** — they receive immutable session state and return a response. The runtime kernel handles all mutation.

## Message Types

### SessionStart

Creates a new session. This must be the first message for any session.

**Requirements:**
- `message_type` must be `"SessionStart"`
- `session_id` must be unique (not already exist)
- `mode` must reference a registered mode (or be empty for default)
- All required Envelope fields must be present

**What happens:**
1. Server resolves mode (empty → "decision")
2. If mode is unknown → reject with `UnknownMode`
3. Server parses TTL from payload (see TTL Configuration below)
4. If TTL is invalid → reject with `InvalidTtl`
5. Server checks if session already exists → reject with `DuplicateSession`
6. Creates session log, appends incoming entry
7. Calls `mode.on_session_start()` → may return `PersistState` with initial mode state
8. Creates new session with state: `Open`, configured TTL, and mode state

**Example (decision mode, default TTL):**
```json
{
  "macp_version": "v1",
  "mode": "decision",
  "message_type": "SessionStart",
  "message_id": "msg-001",
  "session_id": "session-alpha",
  "sender": "agent-1",
  "timestamp_unix_ms": 1700000000000,
  "payload": ""
}
```

**Example (multi_round mode):**
```json
{
  "macp_version": "v1",
  "mode": "multi_round",
  "message_type": "SessionStart",
  "message_id": "msg-001",
  "session_id": "session-beta",
  "sender": "coordinator",
  "timestamp_unix_ms": 1700000000000,
  "payload": "{\"participants\":[\"alice\",\"bob\"],\"convergence\":{\"type\":\"all_equal\"},\"ttl_ms\":60000}"
}
```

### TTL Configuration

The `SessionStart` payload can optionally contain a JSON object to configure session TTL:

```json
{"ttl_ms": 5000}
```

**Rules:**

| Payload | Behavior |
|---------|----------|
| Empty (`b""`) | Default TTL: 60 seconds |
| `{"ttl_ms": 5000}` | Custom TTL: 5 seconds |
| `{}` or `{"ttl_ms": null}` | Default TTL: 60 seconds |
| `{"ttl_ms": 0}` or negative | Rejected with `InvalidTtl` |
| `{"ttl_ms": 86400001}` (>24h) | Rejected with `InvalidTtl` |
| Invalid JSON or non-UTF-8 | Rejected with `InvalidEnvelope` |

**Bounds:** `ttl_ms` must be in range `1..=86,400,000` (1ms to 24 hours).

For multi_round mode, the TTL is part of the mode-specific payload alongside `participants` and `convergence`.

### Regular Message (Decision Mode)

Sends content within an existing decision mode session.

**Requirements:**
- `message_type` can be anything except `"SessionStart"`
- `session_id` must reference an existing session
- Session must be in `Open` state

**What happens:**
1. Server finds the session
2. If not found → reject with `UnknownSession`
3. If found, Open, and TTL has expired → log internal entry, transition to `Expired`, reject with `TtlExpired`
4. If found but not Open → reject with `SessionNotOpen`
5. Append incoming log entry
6. Call `mode.on_message()`:
   - Decision mode: if payload is `"resolve"` → `Resolve`, else → `NoOp`
7. Apply mode response

### Contribute Message (Multi-Round Mode)

Submits a contribution in a multi-round convergence session.

**Requirements:**
- `message_type` must be `"Contribute"`
- `session_id` must reference an existing multi_round session
- `sender` should be one of the registered participants
- `payload` must be JSON: `{"value": "<contribution>"}`

**What happens:**
1. Mode decodes session's `mode_state`
2. If sender's value changed from previous → increment round counter
3. Check convergence: all participants contributed + all values identical
4. If converged → `PersistAndResolve` with resolution `{"converged_value":"...","round":N,"final":{...}}`
5. If not converged → `PersistState` with updated contributions

## Multi-Round Mode Specification

### SessionStart Payload

```json
{
  "participants": ["alice", "bob"],
  "convergence": {"type": "all_equal"},
  "ttl_ms": 60000
}
```

- `participants`: Non-empty list of participant identifiers
- `convergence.type`: Must be `"all_equal"` (only supported strategy)
- `ttl_ms`: Optional TTL override

### Convergence Strategy: `all_equal`

The session resolves automatically when:
1. All listed participants have submitted a contribution
2. All contribution values are identical

### Round Counting

- Round starts at 0
- Each time a participant submits a **new or changed** value, the round increments
- Re-submitting the same value does not increment the round

### Resolution Payload

When convergence is reached, the resolution contains:
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

## Session State Machine

Sessions follow a strict state machine:

```
         SessionStart
              ↓
         ┌────────┐
         │  OPEN  │ ← Initial state
         └────────┘
              ↓
    (mode returns Resolve or
     PersistAndResolve)
              ↓
       ┌──────────┐
       │ RESOLVED │ ← Terminal state
       └──────────┘

    (Alternative path)
              ↓
         (TTL expires)
              ↓
        ┌─────────┐
        │ EXPIRED │ ← Terminal state
        └─────────┘
```

**State descriptions:**

| State | Can receive messages? | Can transition to |
|-------|----------------------|-------------------|
| OPEN | Yes | RESOLVED, EXPIRED |
| RESOLVED | No | (none - terminal) |
| EXPIRED | No | (none - terminal) |

Resolution is now **mode-driven** — the runtime applies whatever `ModeResponse` the mode returns, rather than checking for hardcoded payloads.

## Validation Rules

The server validates every message before processing. Here are all the checks:

### 1. Version Check
```
IF macp_version != "v1"
THEN reject with InvalidMacpVersion
```

### 2. Required Fields Check
```
IF session_id is empty OR message_id is empty
THEN reject with InvalidEnvelope
```

### 3. Mode Check (for SessionStart)
```
IF mode is not registered
THEN reject with UnknownMode
```

### 4. Session Existence (for SessionStart)
```
IF message_type == "SessionStart" AND session exists
THEN reject with DuplicateSession
```

### 5. Session Existence (for other messages)
```
IF message_type != "SessionStart" AND session does not exist
THEN reject with UnknownSession
```

### 6. Session State Check
```
IF session exists AND session.state != OPEN
THEN reject with SessionNotOpen
```

### 7. TTL Payload Check (for SessionStart)
```
IF message_type == "SessionStart" AND payload is non-empty
THEN parse payload as JSON {"ttl_ms": <integer>}
IF invalid UTF-8 or invalid JSON THEN reject with InvalidEnvelope
IF ttl_ms <= 0 OR ttl_ms > 86400000 THEN reject with InvalidTtl
```

### 8. TTL Expiry Check (for non-SessionStart)
```
IF session.state == OPEN AND current_time > session.ttl_expiry
THEN log internal TtlExpired entry, transition session to EXPIRED, reject with TtlExpired
```

## Error Codes

All possible errors:

| Error Code | When it occurs | How to fix |
|------------|----------------|------------|
| `InvalidMacpVersion` | `macp_version` is not "v1" | Use `macp_version: "v1"` |
| `InvalidEnvelope` | Missing required fields | Include all required fields |
| `DuplicateSession` | SessionStart for existing session | Use a different `session_id` |
| `UnknownSession` | Message for non-existent session | Send SessionStart first |
| `SessionNotOpen` | Message to resolved/expired session | Can't send more messages |
| `TtlExpired` | Session TTL has elapsed | Create a new session |
| `InvalidTtl` | TTL value out of range (<=0 or >24h) | Use ttl_ms in range 1..=86400000 |
| `UnknownMode` | Mode field references unregistered mode | Use "decision" or "multi_round" |
| `InvalidModeState` | Internal mode state is corrupted | Typically an internal error |
| `InvalidPayload` | Payload doesn't match mode's expected format | Check mode-specific payload requirements |

## gRPC Service Definition

In Protocol Buffers syntax:

```protobuf
service MACPService {
  rpc SendMessage(Envelope) returns (Ack);
  rpc GetSession(SessionQuery) returns (SessionInfo);
}
```

**What this means:**
- Service name: `MACPService`
- Two operations: `SendMessage` and `GetSession`
- `SendMessage`: Takes `Envelope`, returns `Ack`
- `GetSession`: Takes `SessionQuery`, returns `SessionInfo` (or gRPC NOT_FOUND)
- Communication: Synchronous (client waits for response)

## Transport

The protocol uses **gRPC over HTTP/2**:

**Advantages:**
- Binary protocol (efficient)
- Type-safe (schema enforcement)
- Streaming support (future extension)
- Wide language support
- Built-in authentication (TLS)

**Default address:** `127.0.0.1:50051`

## Future Extensions (Planned)

### 1. Background TTL Cleanup
Currently, TTL is enforced on message receipt (lazy expiry). Future versions will:
- Run a background task to periodically remove expired sessions from memory
- Reduce memory footprint for long-running servers

### 2. Replay Engine
Replay session logs to reconstruct state for debugging and auditing.

### 3. GetSessionLog RPC
New RPC to query session event logs:
```protobuf
rpc GetSessionLog(SessionQuery) returns (SessionLog);
```

### 4. Participant Membership Gating
Enforce that only registered participants can send messages to a session.

### 5. Additional Convergence Strategies
- `majority` — resolve when a majority of participants agree
- `threshold` — resolve when N participants agree

### 6. Streaming
Support for bidirectional streaming:
```protobuf
rpc StreamMessages(stream Envelope) returns (stream Ack);
```

## Comparison to Other Protocols

### vs HTTP REST
- MACP: Binary, type-safe, generated clients
- REST: Text-based, flexible, manual clients

### vs WebSockets
- MACP: RPC-style (request/response pairs)
- WebSockets: Raw bidirectional streaming

### vs Message Queues (RabbitMQ, Kafka)
- MACP: Synchronous acknowledgment, session-oriented
- Message Queues: Asynchronous, topic-oriented

## Best Practices

### For Clients

1. **Always check Ack.accepted**
   ```rust
   let ack = client.send_message(env).await?.into_inner();
   if !ack.accepted {
       println!("Error: {}", ack.error);
   }
   ```

2. **Use unique message IDs**
   - UUIDs are recommended
   - Helps with debugging and tracing

3. **Send SessionStart first**
   - Before any other messages
   - Keep track of which sessions you've started

4. **Handle all error codes**
   - Don't just check `accepted`
   - Log specific errors for debugging

5. **Respect Resolved state**
   - Don't send messages after resolve
   - Cache the state locally to avoid unnecessary calls

6. **Use GetSession to check state**
   - Query session state before sending messages
   - Useful for resuming after disconnection

## Next Steps

- Read [architecture.md](./architecture.md) to understand how this is implemented
- Read [examples.md](./examples.md) for practical code examples
