# MACP Runtime Documentation

Welcome to the Multi-Agent Coordination Protocol (MACP) Runtime documentation. This guide explains everything about the system in plain language, whether you are an experienced distributed-systems engineer or someone encountering multi-agent coordination for the first time.

---

## What Is This Project?

The MACP Runtime — also called the **Minimal Coordination Runtime (MCR)** — is a **gRPC server** that helps multiple AI agents or programs coordinate with each other. Think of it as a traffic controller for structured conversations between autonomous agents: it manages who can speak, tracks the state of each conversation, enforces time limits, and determines when a conversation has reached its conclusion.

Version **0.3** of the runtime implements **RFC-0001**, introducing a formal protocol handshake, structured error reporting, a rich Decision Mode lifecycle, session cancellation, message deduplication, participant validation, mode-aware authorization, and a host of new RPCs for runtime introspection.

### Real-World Analogy

Imagine you are chairing a formal committee meeting:

1. **Someone opens the meeting** — a `SessionStart` message creates a new coordination session.
2. **The chair announces the rules** — the `Initialize` handshake negotiates which protocol version everyone speaks and what capabilities the runtime supports.
3. **Participants discuss and propose** — agents send `Proposal`, `Evaluation`, `Objection`, and `Vote` messages through the Decision Mode, or `Contribute` messages through the Multi-Round Mode.
4. **The committee reaches a decision** — when the mode's convergence criteria are met, the session transitions to **Resolved** and the resolution is recorded.
5. **After the gavel falls, no more motions are accepted** — once a session is resolved or expired, no further messages can be sent to it.
6. **The chair can also adjourn early** — a `CancelSession` call terminates the session before natural resolution.

The MACP Runtime manages this entire lifecycle automatically and enforces the rules at every step.

---

## What Problem Does It Solve?

When multiple AI agents or programs need to work together, they need a way to:

- **Negotiate a common protocol** — agree on version, capabilities, and supported modes before any real work begins.
- **Start a conversation** — create a session with a declared intent, participant list, and time-to-live.
- **Exchange messages safely** — with deduplication, participant validation, and ordered logging.
- **Track the state** of the conversation — know whether it is open, resolved, or expired at any moment.
- **Reach a decision** — through a structured lifecycle (proposals, evaluations, votes, commitments) or through iterative convergence.
- **Know when it is done** — terminal states are enforced; resolved or expired sessions reject further messages.
- **Cancel gracefully** — terminate sessions explicitly with a recorded reason.
- **Discover capabilities** — query which modes are available, inspect manifests, and watch for registry changes.

Without a coordination runtime, each agent would need to implement all of this logic independently, leading to subtle bugs, inconsistent state machines, and fragile integrations. The MACP Runtime centralizes these concerns so that agents can focus on their domain logic.

---

## Key Concepts

### Protocol Version

The current protocol version is **`1.0`**. Every `Envelope` must carry `macp_version: "1.0"` or the message will be rejected with `UNSUPPORTED_PROTOCOL_VERSION`. Before sending any session messages, clients should call the `Initialize` RPC to negotiate the protocol version and discover runtime capabilities.

### Sessions

A **session** is a bounded coordination context — like a conversation thread with rules. Each session has:

- A unique **session ID** chosen by the creator.
- A **mode** that defines the coordination logic (e.g., `macp.mode.decision.v1` or `macp.mode.multi_round.v1`).
- A current **state**: `Open`, `Resolved`, or `Expired`.
- A **time-to-live (TTL)** — how long the session remains open before automatic expiry (default 60 seconds, max 24 hours).
- An optional **participant list** — if provided, only listed senders may contribute.
- An optional **resolution** — the final outcome, recorded when the mode resolves the session.
- **Version metadata** — intent, mode_version, configuration_version, and policy_version carried from the `SessionStartPayload`.

### Messages (Envelopes)

Every message is wrapped in an **Envelope** — a structured protobuf container that carries:

- **macp_version** — protocol version (`"1.0"`).
- **mode** — which coordination mode handles this message.
- **message_type** — the semantic type (`SessionStart`, `Message`, `Proposal`, `Vote`, `Contribute`, `Signal`, etc.).
- **message_id** — a unique identifier for deduplication and tracing.
- **session_id** — which session this belongs to (may be empty for `Signal` messages).
- **sender** — who is sending the message.
- **timestamp_unix_ms** — informational timestamp.
- **payload** — the actual content (protobuf-encoded or JSON, depending on the mode and message type).

### Acknowledgments (Ack)

Every `Send` call returns an **Ack** — a structured response that tells you:

- **ok** — `true` if accepted, `false` if rejected.
- **duplicate** — `true` if this was an idempotent replay of a previously accepted message.
- **message_id** and **session_id** — echoed back for correlation.
- **accepted_at_unix_ms** — server-side acceptance timestamp.
- **session_state** — the session's state after processing (OPEN, RESOLVED, EXPIRED).
- **error** — a structured `MACPError` with an RFC error code, human-readable message, and optional details.

### Session States

Sessions follow a strict state machine with three states:

| State | Can receive messages? | Transitions to |
|-------|----------------------|----------------|
| **Open** | Yes | Resolved, Expired |
| **Resolved** | No (terminal) | — |
| **Expired** | No (terminal) | — |

- **Open** — the session is active and accepting messages. This is the initial state after `SessionStart`.
- **Resolved** — a mode returned a `Resolve` or `PersistAndResolve` response, recording the final outcome. No further messages are accepted.
- **Expired** — the session's TTL elapsed (detected lazily on the next message), or the session was explicitly cancelled via `CancelSession`. No further messages are accepted.

### Modes

**Modes** are pluggable coordination strategies. The runtime provides the "physics" — session invariants, logging, TTL enforcement, routing — while modes provide the "coordination logic" — when to resolve, what state to track, and what convergence criteria to apply.

Two modes are built in:

| Mode Name | Aliases | Description |
|-----------|---------|-------------|
| `macp.mode.decision.v1` | `decision` | RFC-compliant decision lifecycle: Proposal → Evaluation → Objection → Vote → Commitment |
| `macp.mode.multi_round.v1` | `multi_round` | Participant-based convergence using `all_equal` strategy (experimental, not on discovery surfaces) |

An empty `mode` field defaults to `macp.mode.decision.v1` for backward compatibility.

### Signals

**Signal** messages are ambient, session-less messages. They can be sent with an empty `session_id` and do not create or modify any session. They are useful for out-of-band coordination hints, heartbeats, or cross-session correlation.

---

## How It Works (High Level)

```
Client                              MACP Runtime
  |                                      |
  |--- Initialize(["1.0"]) ------------>|
  |<-- InitializeResponse(v=1.0) -------|  (handshake complete)
  |                                      |
  |--- Send(SessionStart, s1) --------->|
  |<-- Ack(ok=true, state=OPEN) --------|  (session created)
  |                                      |
  |--- Send(Proposal, s1) ------------->|
  |<-- Ack(ok=true, state=OPEN) --------|  (proposal recorded)
  |                                      |
  |--- Send(Vote, s1) ----------------->|
  |<-- Ack(ok=true, state=OPEN) --------|  (vote recorded)
  |                                      |
  |--- Send(Commitment, s1) ----------->|
  |<-- Ack(ok=true, state=RESOLVED) ----|  (session resolved)
  |                                      |
  |--- Send(Message, s1) -------------->|
  |<-- Ack(ok=false, SESSION_NOT_OPEN) -|  (rejected: terminal)
  |                                      |
  |--- GetSession(s1) ----------------->|
  |<-- SessionMetadata(RESOLVED) -------|  (query state)
```

---

## What Is Built With

- **gRPC over HTTP/2** — high-performance, type-safe RPC framework with streaming support.
- **Protocol Buffers (protobuf)** — binary serialization for efficient, schema-enforced message exchange.
- **Rust** — memory-safe, concurrent systems language with zero-cost abstractions.
- **Tonic** — Rust's async gRPC framework built on Tokio.
- **Buf** — protobuf linting and breaking-change detection.

You do not need to know Rust to understand the protocol or use the runtime — any language with a gRPC client can connect.

---

## Components

This runtime consists of:

1. **Runtime Server** (`macp-runtime`) — the main coordination server managing sessions, modes, and protocol enforcement.
2. **Basic Client** (`client`) — a demo client exercising the happy path: Initialize, ListModes, SessionStart, Message, Resolve, GetSession.
3. **Fuzz Client** (`fuzz_client`) — a comprehensive test client exercising every error path, every new RPC, participant validation, signal messages, cancellation, and multi-round convergence.
4. **Multi-Round Client** (`multi_round_client`) — a focused demo of multi-round convergence with two participants reaching agreement.

---

## Documentation Structure

| Document | What It Covers |
|----------|---------------|
| **[protocol.md](./protocol.md)** | Full MACP v1.0 protocol specification — message types, validation rules, error codes, mode specifications |
| **[architecture.md](./architecture.md)** | Internal architecture — component design, data flow, concurrency model, design principles |
| **[examples.md](./examples.md)** | Step-by-step usage examples — client walkthroughs, common patterns, FAQ |

---

## Quick Start

**Terminal 1** — Start the server:
```bash
cargo run
```

You should see:
```
macp-runtime v0.3.0 (RFC-0001) listening on 127.0.0.1:50051
```

**Terminal 2** — Run a test client:
```bash
cargo run --bin client
```

You will see the client negotiate the protocol version, discover modes, create a session, send messages, resolve the session, and verify the final state.

---

## Next Steps

1. Read **[protocol.md](./protocol.md)** to understand the full MACP v1.0 protocol specification.
2. Read **[architecture.md](./architecture.md)** to understand how the runtime is built internally.
3. Read **[examples.md](./examples.md)** for practical, step-by-step usage examples.
