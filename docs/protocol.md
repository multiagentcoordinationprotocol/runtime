# MACP Protocol Specification (v1.0 — RFC-0001)

This document is the authoritative specification of the Multi-Agent Coordination Protocol (MACP) as implemented by `macp-runtime` v0.3. It describes every message type, every field, every validation rule, every error code, and every behavioral guarantee in narrative detail. Whether you are building a client, implementing a new mode, or auditing the protocol for correctness, this document is your reference.

---

## Table of Contents

1. [Protocol Version](#protocol-version)
2. [Core Concepts](#core-concepts)
3. [Protobuf Schema Organization](#protobuf-schema-organization)
4. [The Envelope](#the-envelope)
5. [The Ack Response](#the-ack-response)
6. [Structured Errors (MACPError)](#structured-errors-macperror)
7. [Session State Enum](#session-state-enum)
8. [gRPC Service Definition](#grpc-service-definition)
9. [RPC: Initialize](#rpc-initialize)
10. [RPC: Send](#rpc-send)
11. [RPC: GetSession](#rpc-getsession)
12. [RPC: CancelSession](#rpc-cancelsession)
13. [RPC: GetManifest](#rpc-getmanifest)
14. [RPC: ListModes](#rpc-listmodes)
15. [RPC: StreamSession](#rpc-streamsession)
16. [RPC: ListRoots, WatchModeRegistry, WatchRoots](#rpc-listrootswatchmoderegistrywatchroots)
17. [Message Type: SessionStart](#message-type-sessionstart)
18. [Message Type: Regular Message](#message-type-regular-message)
19. [Message Type: Signal](#message-type-signal)
20. [TTL Configuration](#ttl-configuration)
21. [Message Deduplication](#message-deduplication)
22. [Participant Validation](#participant-validation)
23. [Session State Machine](#session-state-machine)
24. [Mode System](#mode-system)
25. [Decision Mode Specification](#decision-mode-specification)
26. [Proposal Mode Specification](#proposal-mode-specification)
27. [Task Mode Specification](#task-mode-specification)
28. [Handoff Mode Specification](#handoff-mode-specification)
29. [Quorum Mode Specification](#quorum-mode-specification)
30. [Multi-Round Mode Specification](#multi-round-mode-specification)
31. [Validation Rules (Complete)](#validation-rules-complete)
32. [Error Codes (Complete)](#error-codes-complete)
33. [Transport](#transport)
34. [Best Practices](#best-practices)
35. [Future Extensions](#future-extensions)

---

## Protocol Version

Current version: **`1.0`**

All messages must carry `macp_version: "1.0"`. The `Initialize` RPC is the mechanism by which client and server agree on a protocol version. The server currently supports only `"1.0"` — if the client proposes only unsupported versions, the `Initialize` call returns a gRPC `INVALID_ARGUMENT` error.

> **Migration note from v0.1:** The previous protocol used `macp_version: "v1"`. Version 0.2 uses `"1.0"`. Old clients sending `"v1"` will receive `UNSUPPORTED_PROTOCOL_VERSION`.

---

## Core Concepts

### What Is a Protocol?

A protocol is a set of rules that all participants agree to follow. Just as HTTP defines how browsers and servers exchange web pages, the MACP protocol defines how agents exchange coordination messages: what information must be included, what order things must happen, what is allowed, and what is forbidden.

### Why a Formal Protocol?

Without a formal protocol, different agents might format messages differently, state transitions could be inconsistent, errors would be ambiguous, and debugging would be nearly impossible. With MACP:

- Everyone speaks the same structured language.
- Behavior is predictable and deterministic.
- Tools and clients can be built for any MACP-compliant runtime.
- Audit logs are meaningful because every event follows a known schema.

---

## Protobuf Schema Organization

The protocol is defined across seven protobuf files, organized by concern:

```
proto/
├── buf.yaml                                    # Buf linter config (STANDARD lint, FILE breaking)
└── macp/
    ├── v1/
    │   ├── envelope.proto                      # Envelope, Ack, MACPError, SessionState enum
    │   └── core.proto                          # Service definition, all request/response types,
    │                                           #   capability messages, session payloads, manifests,
    │                                           #   mode descriptors, streaming types
    └── modes/
        ├── decision/
        │   └── v1/
        │       └── decision.proto              # ProposalPayload, EvaluationPayload,
        │                                       #   ObjectionPayload, VotePayload
        ├── proposal/
        │   └── v1/
        │       └── proposal.proto              # Proposal mode payload types
        ├── task/
        │   └── v1/
        │       └── task.proto                  # Task mode payload types
        ├── handoff/
        │   └── v1/
        │       └── handoff.proto               # Handoff mode payload types
        └── quorum/
            └── v1/
                └── quorum.proto                # Quorum mode payload types
```

**`envelope.proto`** contains the foundational types that every message touches: the `Envelope` wrapper, the `Ack` acknowledgment, the `MACPError` structured error, and the `SessionState` enum. These are imported by `core.proto`.

**`core.proto`** contains everything else: the `MACPRuntimeService` definition with all ten RPCs, the request/response wrappers, capability negotiation messages (`ClientInfo`, `RuntimeInfo`, `Capabilities` and its sub-capabilities), session lifecycle payloads (`SessionStartPayload`, `SessionCancelPayload`, `CommitmentPayload`), introspection types (`AgentManifest`, `ModeDescriptor`), and streaming types.

**`decision.proto`** contains the mode-specific payload types for the Decision Mode: `ProposalPayload`, `EvaluationPayload`, `ObjectionPayload`, and `VotePayload`. These are not referenced by the core proto — they are domain-level schemas that clients use to structure their payloads.

**`proposal.proto`** contains the payload types for the Proposal Mode's peer-based propose/accept/reject lifecycle.

**`task.proto`** contains the payload types for the Task Mode's orchestrated task assignment and completion tracking.

**`handoff.proto`** contains the payload types for the Handoff Mode's delegated context transfer between agents.

**`quorum.proto`** contains the payload types for the Quorum Mode's threshold-based voting and resolution.

The `buf.yaml` file configures the Buf linter with `STANDARD` lint rules and `FILE`-level breaking-change detection, ensuring the proto schema evolves safely.

---

## The Envelope

Every message sent through the `Send` or `StreamSession` RPC is wrapped in an **Envelope**. The Envelope is the universal container — it carries both the routing metadata and the actual payload.

```protobuf
message Envelope {
  string macp_version     = 1;   // Must be "1.0"
  string mode             = 2;   // Coordination mode (e.g., "decision", "macp.mode.decision.v1")
  string message_type     = 3;   // Semantic type: "SessionStart", "Message", "Proposal", etc.
  string message_id       = 4;   // Unique ID for this message (used for deduplication)
  string session_id       = 5;   // Session this belongs to (empty for Signal messages)
  string sender           = 6;   // Who is sending
  int64  timestamp_unix_ms = 7;  // Informational client-side timestamp
  bytes  payload          = 8;   // The actual content
}
```

**Field-by-field narrative:**

- **`macp_version`** — The protocol version. The server checks this first. If it is not `"1.0"`, the message is immediately rejected with `UNSUPPORTED_PROTOCOL_VERSION`. This is a hard gate — no further processing occurs.

- **`mode`** — The name of the coordination mode that should handle this message. Accepted values include RFC-compliant names (`macp.mode.decision.v1`, `macp.mode.multi_round.v1`) and backward-compatible aliases (`decision`, `multi_round`). An empty string defaults to `macp.mode.decision.v1`. If the name does not match any registered mode, the message is rejected with `MODE_NOT_SUPPORTED`.

- **`message_type`** — Determines how the runtime routes the message. Three routing categories exist:
  - `"SessionStart"` — creates a new session.
  - `"Signal"` — ambient message that does not require a session.
  - Everything else (`"Message"`, `"Proposal"`, `"Evaluation"`, `"Objection"`, `"Vote"`, `"Commitment"`, `"Contribute"`, etc.) — dispatched to the mode's `on_message()` handler within an existing session.

- **`message_id`** — A client-chosen unique identifier. The runtime uses this for deduplication: if a message with the same `message_id` has already been accepted for a given session, the runtime returns `ok: true, duplicate: true` without re-processing. Clients should use UUIDs or similarly unique values.

- **`session_id`** — Identifies the session. Required for all message types except `Signal`. For `SessionStart`, this becomes the ID of the newly created session. For subsequent messages, the runtime looks up this session in the registry.

- **`sender`** — Identifies who is sending the message. If the session has a non-empty participant list, the sender must be a member of that list or the message is rejected with `INVALID_ENVELOPE`.

- **`timestamp_unix_ms`** — An informational timestamp set by the client. The runtime does not use this for any logic — it records its own `accepted_at_unix_ms` in the Ack. This field exists for client-side tracing and ordering.

- **`payload`** — The actual content of the message, encoded as raw bytes. The interpretation depends on the `message_type` and the mode:
  - For `SessionStart`: a protobuf-encoded `SessionStartPayload` (or empty bytes for defaults).
  - For Decision Mode messages: JSON-encoded payloads matching `ProposalPayload`, `EvaluationPayload`, etc.
  - For Multi-Round `Contribute` messages: JSON `{"value": "<string>"}`.
  - For `Signal`: arbitrary bytes.

---

## The Ack Response

Every `Send` call returns an `Ack` — a structured acknowledgment that provides complete information about what happened.

```protobuf
message Ack {
  bool         ok                = 1;   // true if accepted
  bool         duplicate         = 2;   // true if this was an idempotent replay
  string       message_id        = 3;   // echoed from the request
  string       session_id        = 4;   // echoed from the request
  int64        accepted_at_unix_ms = 5; // server-side timestamp
  SessionState session_state     = 6;   // session state after processing
  MACPError    error             = 7;   // structured error (if ok == false)
}
```

**Understanding the Ack fields:**

- **`ok`** — The primary success indicator. `true` means the message was accepted and processed. `false` means it was rejected — consult the `error` field for details.

- **`duplicate`** — Set to `true` when the runtime recognizes a previously-accepted `message_id` for the same session. The message is not reprocessed; the Ack simply confirms idempotent acceptance. This allows clients to safely retry without side effects.

- **`message_id`** and **`session_id`** — Echoed back from the request for client-side correlation, especially useful in asynchronous or batched workflows.

- **`accepted_at_unix_ms`** — The server-side timestamp (milliseconds since Unix epoch) at the moment the message was accepted. This is authoritative — clients should use this rather than their own `timestamp_unix_ms` for ordering guarantees.

- **`session_state`** — The session's state *after* the message was processed. This tells the client whether the session is still `OPEN`, has been `RESOLVED` (e.g., after a `Commitment` message in Decision Mode), or has `EXPIRED`. For messages that don't touch a session (e.g., `Signal`), this is `SESSION_STATE_OPEN`.

- **`error`** — A structured `MACPError` object present when `ok == false`. Contains the RFC error code, a human-readable message, and optional correlation fields. See the next section for details.

> **Migration note from v0.1:** The old `Ack` had only `accepted: bool` and `error: string`. The new Ack is significantly richer — clients should update to read the structured `error` field and the `duplicate` and `session_state` fields.

---

## Structured Errors (MACPError)

When a message is rejected, the `Ack.error` field contains a structured error:

```protobuf
message MACPError {
  string code       = 1;   // RFC error code (e.g., "INVALID_ENVELOPE")
  string message    = 2;   // Human-readable description
  string session_id = 3;   // Correlated session (if applicable)
  string message_id = 4;   // Correlated message (if applicable)
  bytes  details    = 5;   // Optional additional detail payload
}
```

The `code` field uses a fixed vocabulary of RFC-compliant error codes (see [Error Codes](#error-codes-complete) below). The `message` field provides a human-readable explanation. The `session_id` and `message_id` fields echo back the relevant identifiers for correlation. The `details` field is reserved for future use (e.g., structured error payloads for specific modes).

---

## Session State Enum

Session state is represented as a protobuf enum:

```protobuf
enum SessionState {
  SESSION_STATE_UNSPECIFIED = 0;
  SESSION_STATE_OPEN        = 1;
  SESSION_STATE_RESOLVED    = 2;
  SESSION_STATE_EXPIRED     = 3;
}
```

The `UNSPECIFIED` value is the protobuf default and should not be set intentionally. The runtime maps its internal `SessionState` enum (`Open`, `Resolved`, `Expired`) to these wire values.

---

## gRPC Service Definition

The `MACPRuntimeService` is the single gRPC service exposed by the runtime:

```protobuf
service MACPRuntimeService {
  rpc Initialize(InitializeRequest)         returns (InitializeResponse);
  rpc Send(SendRequest)                     returns (SendResponse);
  rpc StreamSession(stream StreamSessionRequest) returns (stream StreamSessionResponse);
  rpc GetSession(GetSessionRequest)         returns (GetSessionResponse);
  rpc CancelSession(CancelSessionRequest)   returns (CancelSessionResponse);
  rpc GetManifest(GetManifestRequest)       returns (GetManifestResponse);
  rpc ListModes(ListModesRequest)           returns (ListModesResponse);
  rpc WatchModeRegistry(WatchModeRegistryRequest) returns (stream WatchModeRegistryResponse);
  rpc ListRoots(ListRootsRequest)           returns (ListRootsResponse);
  rpc WatchRoots(WatchRootsRequest)         returns (stream WatchRootsResponse);
}
```

> **Migration note from v0.1:** The old service was named `MACPService` with only two RPCs (`SendMessage` and `GetSession`). The v0.2 service is `MACPRuntimeService` with ten RPCs. The `SendMessage` RPC has been replaced by `Send` (which wraps the `Envelope` in a `SendRequest`).

---

## RPC: Initialize

The `Initialize` RPC is a protocol handshake that should be called before any session work begins. It negotiates the protocol version and exchanges capability information.

**Request:**
```protobuf
message InitializeRequest {
  repeated string supported_protocol_versions = 1;  // e.g., ["1.0"]
  ClientInfo      client_info                  = 2;  // optional client metadata
  Capabilities    capabilities                 = 3;  // optional client capabilities
}
```

**Response:**
```protobuf
message InitializeResponse {
  string          selected_protocol_version = 1;  // "1.0"
  RuntimeInfo     runtime_info              = 2;  // server name, version, description
  Capabilities    capabilities              = 3;  // server capabilities
  repeated string supported_modes           = 4;  // registered mode names
  string          instructions              = 5;  // human-readable usage instructions
}
```

**Behavior:**

1. The server inspects the client's `supported_protocol_versions` list.
2. If `"1.0"` is in the list, it is selected. If not, the RPC returns a gRPC `INVALID_ARGUMENT` status with a descriptive message.
3. The response includes the runtime's identity (`RuntimeInfo` with name `"macp-runtime"`, version `"0.2.0"`), its capabilities (sessions with streaming, cancellation, progress, manifest, mode registry, and roots), and the list of supported modes.
4. The `instructions` field provides a brief human-readable note about the runtime.

**Capabilities advertised:**

| Capability | Value | Description |
|------------|-------|-------------|
| `sessions.stream` | `true` | StreamSession RPC is available |
| `cancellation.cancel_session` | `true` | CancelSession RPC is available |
| `progress.progress` | `true` | Progress tracking is supported |
| `manifest.get_manifest` | `true` | GetManifest RPC is available |
| `mode_registry.list_modes` | `true` | ListModes RPC is available |
| `mode_registry.list_changed` | `true` | WatchModeRegistry RPC is available |
| `roots.list_roots` | `true` | ListRoots RPC is available |
| `roots.list_changed` | `true` | WatchRoots RPC is available |

---

## RPC: Send

The `Send` RPC is the primary message ingestion point. It accepts a `SendRequest` containing an `Envelope` and returns a `SendResponse` containing an `Ack`.

**Request:**
```protobuf
message SendRequest {
  Envelope envelope = 1;
}
```

**Response:**
```protobuf
message SendResponse {
  Ack ack = 1;
}
```

**Processing flow:**

1. **Validate the Envelope** — check `macp_version == "1.0"`, check that `session_id` and `message_id` are non-empty (except for `Signal` messages where `session_id` may be empty).
2. **Delegate to the Runtime** — the `Runtime::process()` method routes to `process_session_start()`, `process_signal()`, or `process_message()` based on `message_type`.
3. **Build the Ack** — the server constructs a full `Ack` with `ok`, `duplicate`, echoed IDs, server timestamp, session state, and any error.

All errors are returned in the Ack — the gRPC status is always `OK` for protocol-level errors. Only infrastructure-level failures (e.g., missing `Envelope` in the request) return non-OK gRPC statuses.

---

## RPC: GetSession

Retrieves metadata for a specific session.

**Request:**
```protobuf
message GetSessionRequest {
  string session_id = 1;
}
```

**Response:**
```protobuf
message GetSessionResponse {
  SessionMetadata metadata = 1;
}

message SessionMetadata {
  string       session_id            = 1;
  string       mode                  = 2;
  SessionState state                 = 3;
  int64        started_at_unix_ms    = 4;
  int64        expires_at_unix_ms    = 5;
  string       mode_version          = 6;
  string       configuration_version = 7;
  string       policy_version        = 8;
}
```

**Behavior:**

If the session exists, its metadata is returned — including mode name, current state (as a `SessionState` enum value), creation timestamp, TTL expiry timestamp, and the version fields from the original `SessionStartPayload`.

If the session does not exist, the RPC returns a gRPC `NOT_FOUND` status.

> **Migration note from v0.1:** The old `GetSession` returned a `SessionInfo` with fields like `state` (as a string), `resolution`, `mode_state`, and `participants`. The new response uses `SessionMetadata` with typed `SessionState` enum and version metadata fields.

---

## RPC: CancelSession

Explicitly cancels an active session, transitioning it to `Expired` state.

**Request:**
```protobuf
message CancelSessionRequest {
  string session_id = 1;
  string reason     = 2;
}
```

**Response:**
```protobuf
message CancelSessionResponse {
  Ack ack = 1;
}
```

**Behavior:**

1. If the session does not exist, returns `ok: false` with `SESSION_NOT_FOUND`.
2. If the session is already `Resolved` or `Expired`, the cancellation is idempotent — returns `ok: true` without modification.
3. If the session is `Open`, logs an internal `SessionCancel` entry with the provided reason, transitions the session to `Expired`, and returns `ok: true`.

The cancellation reason is persisted in the session's audit log, providing a clear record of why the session was terminated.

---

## RPC: GetManifest

Retrieves the agent manifest — a description of the runtime's identity and capabilities.

**Request:**
```protobuf
message GetManifestRequest {
  string agent_id = 1;  // currently unused
}
```

**Response:**
```protobuf
message GetManifestResponse {
  AgentManifest manifest = 1;
}

message AgentManifest {
  string              agent_id             = 1;
  string              title                = 2;
  string              description          = 3;
  repeated string     supported_modes      = 4;
  repeated string     input_content_types  = 5;
  repeated string     output_content_types = 6;
  map<string, string> metadata             = 7;
}
```

The response includes the runtime's identity (`"macp-runtime"`, `"MACP Coordination Runtime"`), a description, and the list of supported mode names.

---

## RPC: ListModes

Discovers the coordination modes registered in the runtime.

**Request:** `ListModesRequest {}` (empty)

**Response:**
```protobuf
message ListModesResponse {
  repeated ModeDescriptor modes = 1;
}

message ModeDescriptor {
  string              mode                   = 1;
  string              mode_version           = 2;
  string              title                  = 3;
  string              description            = 4;
  string              determinism_class      = 5;
  string              participant_model      = 6;
  repeated string     message_types          = 7;
  repeated string     terminal_message_types = 8;
  map<string, string> schema_uris            = 9;
}
```

**Currently returned descriptors:**

1. **Decision Mode:**
   - `mode`: `"macp.mode.decision.v1"`
   - `mode_version`: `"1.0.0"`
   - `title`: `"Decision Mode"`
   - `determinism_class`: `"deterministic"`
   - `participant_model`: `"open"`
   - `message_types`: `["Proposal", "Evaluation", "Objection", "Vote", "Commitment"]`
   - `terminal_message_types`: `["Commitment"]`

2. **Multi-Round Mode:**
   - `mode`: `"macp.mode.multi_round.v1"`
   - `mode_version`: `"1.0.0"`
   - `title`: `"Multi-Round Convergence Mode"`
   - `determinism_class`: `"deterministic"`
   - `participant_model`: `"closed"`
   - `message_types`: `["Contribute"]`
   - `terminal_message_types`: `["Contribute"]` (the final Contribute that triggers convergence)

---

## RPC: StreamSession

Bidirectional streaming RPC for real-time session interaction.

```protobuf
rpc StreamSession(stream StreamSessionRequest) returns (stream StreamSessionResponse);
```

The client sends a stream of `StreamSessionRequest` messages (each wrapping an `Envelope`), and the server responds with a stream of `StreamSessionResponse` messages (each wrapping an echoed `Envelope` with an updated `message_type` reflecting the processing result). This enables real-time, interactive coordination without polling.

---

## RPC: ListRoots, WatchModeRegistry, WatchRoots

- **ListRoots** — Returns an empty list of `Root` objects. Reserved for future resource-root discovery.
- **WatchModeRegistry** — Server-streaming RPC for mode registry change notifications. Currently returns `UNIMPLEMENTED`.
- **WatchRoots** — Server-streaming RPC for root change notifications. Currently returns `UNIMPLEMENTED`.

---

## Message Type: SessionStart

A `SessionStart` message creates a new coordination session.

**Required fields:**
- `message_type`: `"SessionStart"`
- `session_id`: Must be unique — no session with this ID may already exist.
- `message_id`: Must be non-empty.
- `mode`: Must reference a registered mode (or be empty for the default `macp.mode.decision.v1`).

**Payload:**

The payload should be a protobuf-encoded `SessionStartPayload`:

```protobuf
message SessionStartPayload {
  string         intent                = 1;   // human-readable purpose
  repeated string participants         = 2;   // participant IDs (empty = open participation)
  string         mode_version          = 3;   // version of the mode to use
  string         configuration_version = 4;   // configuration version identifier
  string         policy_version        = 5;   // policy version identifier
  int64          ttl_ms                = 6;   // TTL in milliseconds (0 = default 60s)
  bytes          context               = 7;   // arbitrary context data
  repeated Root  roots                 = 8;   // resource roots
}
```

An empty payload (zero bytes) is valid — the runtime uses defaults (60s TTL, no participants, empty version strings).

**Processing sequence:**

1. Runtime resolves the mode name (empty → `"macp.mode.decision.v1"`).
2. Looks up the mode in the registry — rejects with `MODE_NOT_SUPPORTED` if not found.
3. Decodes the payload as a protobuf `SessionStartPayload` — rejects with `INVALID_ENVELOPE` if decoding fails.
4. Extracts and validates TTL — rejects with `INVALID_ENVELOPE` if out of range (see [TTL Configuration](#ttl-configuration)).
5. Acquires write lock on the session registry.
6. Checks for duplicate session ID:
   - If the session exists and the `message_id` matches the session's `seen_message_ids`, returns `ok: true, duplicate: true` (idempotent).
   - If the session exists with a different `message_id`, rejects with `INVALID_ENVELOPE` (duplicate session).
7. Creates a session log and appends an `Incoming` entry.
8. Calls `mode.on_session_start()` — the mode may return `PersistState` with initial mode state.
9. Creates a `Session` object with state `Open`, computed TTL expiry, participants, version metadata, and the message_id recorded in `seen_message_ids`.
10. Applies the `ModeResponse` to mutate the session (e.g., storing initial mode state).
11. Inserts the session into the registry.

---

## Message Type: Regular Message

Any message with a `message_type` other than `"SessionStart"` or `"Signal"` is treated as a regular message dispatched to the session's mode.

**Required fields:**
- `session_id`: Must reference an existing session.
- `message_id`: Must be non-empty.

**Processing sequence:**

1. Acquires write lock on the session registry.
2. Finds the session — rejects with `SESSION_NOT_FOUND` if not found.
3. **Deduplication check** — if `message_id` is already in the session's `seen_message_ids`, returns `ok: true, duplicate: true` without re-processing.
4. **TTL check** — if the session is `Open` and the current time exceeds `ttl_expiry`, logs an internal `TtlExpired` entry, transitions the session to `Expired`, and rejects with `SESSION_NOT_OPEN`.
5. **State check** — if the session is not `Open` (already `Resolved` or `Expired`), rejects with `SESSION_NOT_OPEN`.
6. **Participant check** — if the session has a non-empty `participants` list and the `sender` is not in it, rejects with `INVALID_ENVELOPE`.
7. Records `message_id` in `seen_message_ids`.
8. Appends an `Incoming` log entry.
9. Calls `mode.on_message(session, envelope)`.
10. Applies the `ModeResponse` to mutate session state.

---

## Message Type: Signal

`Signal` messages are ambient, session-less messages. They are fire-and-forget coordination hints.

**Special rules:**
- `session_id` may be empty.
- `message_id` must be non-empty.
- No session is created, modified, or looked up.
- The runtime simply acknowledges receipt.

**Use cases:**
- Heartbeats between agents.
- Out-of-band coordination hints.
- Cross-session correlation signals (using the `SignalPayload.correlation_session_id` field).

---

## TTL Configuration

Session TTL (time-to-live) determines how long a session remains open before it is considered expired.

**Encoding:** TTL is specified in the `SessionStartPayload.ttl_ms` field (protobuf int64).

| `ttl_ms` value | Behavior |
|----------------|----------|
| `0` (or field absent) | Default TTL: **60,000 ms** (60 seconds) |
| `1` to `86,400,000` | Custom TTL in milliseconds |
| Negative | Rejected with `INVALID_ENVELOPE` |
| `> 86,400,000` (> 24h) | Rejected with `INVALID_ENVELOPE` |

**TTL enforcement:** TTL is enforced **lazily** — the runtime checks `current_time > ttl_expiry` on each non-SessionStart message. When expiry is detected:

1. An internal `TtlExpired` log entry is appended.
2. The session transitions to `Expired`.
3. The message is rejected with error code `SESSION_NOT_OPEN`.

There is no background cleanup thread — expired sessions remain in memory until the server is restarted. This is a deliberate simplification; future versions may add background eviction.

> **Migration note from v0.1:** TTL was previously specified as a JSON payload `{"ttl_ms": <value>}`. It is now a field in the protobuf `SessionStartPayload`.

---

## Message Deduplication

The runtime provides **at-least-once** delivery with idempotent acceptance via message deduplication.

Each session maintains a `seen_message_ids: HashSet<String>`. When a message arrives:

1. If `message_id` is already in `seen_message_ids`, the runtime returns `ok: true, duplicate: true` without re-processing the message or calling the mode.
2. If `message_id` is new, it is added to `seen_message_ids` before processing.

This applies to both `SessionStart` and regular messages:

- **SessionStart deduplication:** If a `SessionStart` arrives for a session that already exists and the `message_id` matches one in the session's `seen_message_ids`, it is treated as an idempotent retry. If the `message_id` is different, it is rejected as a duplicate session.
- **Regular message deduplication:** If a regular message's `message_id` matches a previously accepted message for that session, it is returned as a duplicate.

This design allows clients to safely retry failed network requests without causing double-processing.

---

## Participant Validation

Sessions can optionally restrict which senders are allowed to contribute.

**Configuration:** The `SessionStartPayload.participants` field is a list of participant identifiers. If this list is non-empty, only senders whose name appears in the list may send messages to the session.

**Enforcement:**

- For regular messages (not `SessionStart` or `Signal`), the runtime checks whether `envelope.sender` is in `session.participants`.
- If the participant list is non-empty and the sender is not in it, the message is rejected with error code `INVALID_ENVELOPE`.
- If the participant list is empty, any sender is allowed (open participation).

**Mode-specific behavior:**

- In **Multi-Round Mode**, participants are essential — convergence is checked against the participant list. All listed participants must contribute for convergence to trigger.
- In **Decision Mode**, participants are optional — the mode works with or without a restricted participant list.

---

## Session State Machine

Sessions follow a strict state machine with three states and two terminal transitions:

```
                SessionStart
                     │
                     ▼
              ┌────────────┐
              │    OPEN     │ ← Initial state
              └────────────┘
               │           │
    (mode returns     (TTL expires or
     Resolve or        CancelSession)
     PersistAndResolve)
               │           │
               ▼           ▼
        ┌──────────┐  ┌─────────┐
        │ RESOLVED │  │ EXPIRED │
        └──────────┘  └─────────┘
         (terminal)    (terminal)
```

**Transition rules:**

| From | To | Trigger |
|------|----|---------|
| Open | Resolved | Mode returns `ModeResponse::Resolve` or `ModeResponse::PersistAndResolve` |
| Open | Expired | TTL check fails on next message, or `CancelSession` RPC called |
| Resolved | — | Terminal — no transitions allowed |
| Expired | — | Terminal — no transitions allowed |

Once a session reaches a terminal state, any subsequent message to that session is rejected with `SESSION_NOT_OPEN`.

---

## Mode System

The Mode system is the heart of MACP's extensibility. The runtime provides "physics" — session invariants, TTL enforcement, logging, routing, participant validation — while Modes provide "coordination logic" — when to resolve, what intermediate state to track, and what convergence criteria to apply.

### The Mode Trait

```rust
pub trait Mode: Send + Sync {
    fn on_session_start(&self, session: &Session, env: &Envelope)
        -> Result<ModeResponse, MacpError>;
    fn on_message(&self, session: &Session, env: &Envelope)
        -> Result<ModeResponse, MacpError>;
}
```

Both methods receive **immutable** references to the session and envelope. They cannot directly mutate state — they return a `ModeResponse` that the runtime applies as a single atomic mutation.

### ModeResponse

```rust
pub enum ModeResponse {
    NoOp,                                          // No state change
    PersistState(Vec<u8>),                         // Update mode_state bytes
    Resolve(Vec<u8>),                              // Set resolution, transition to Resolved
    PersistAndResolve { state: Vec<u8>, resolution: Vec<u8> },  // Both
}
```

- **NoOp** — The mode has nothing to do. The message is accepted but no state changes.
- **PersistState** — The mode wants to update its internal state (e.g., record a vote, update a contribution). The bytes are stored in `session.mode_state`.
- **Resolve** — The mode has determined that the session should resolve. The resolution bytes are stored in `session.resolution` and the session transitions to `Resolved`.
- **PersistAndResolve** — Both state update and resolution in a single atomic operation.

### Mode Registration

The runtime registers modes by name in a `HashMap`:

| Key | Mode |
|-----|------|
| `"macp.mode.decision.v1"` | `DecisionMode` |
| `"macp.mode.proposal.v1"` | `ProposalMode` |
| `"macp.mode.task.v1"` | `TaskMode` |
| `"macp.mode.handoff.v1"` | `HandoffMode` |
| `"macp.mode.quorum.v1"` | `QuorumMode` |
| `"macp.mode.multi_round.v1"` | `MultiRoundMode` |
| `"decision"` | `DecisionMode` (alias) |
| `"multi_round"` | `MultiRoundMode` (alias) |

An empty `mode` field in the Envelope defaults to `"macp.mode.decision.v1"`.

---

## Decision Mode Specification

The Decision Mode (`macp.mode.decision.v1`) implements a structured decision-making lifecycle following RFC-0001. It models the flow from initial proposal through evaluation, optional objection, voting, and final commitment.

### Decision State

```rust
pub struct DecisionState {
    pub proposals: HashMap<String, Proposal>,  // proposal_id → Proposal
    pub evaluations: Vec<Evaluation>,
    pub objections: Vec<Objection>,
    pub votes: HashMap<String, Vote>,          // sender → Vote (last vote wins)
    pub phase: DecisionPhase,
}

pub enum DecisionPhase {
    Proposal,    // Initial phase — waiting for proposals
    Evaluation,  // At least one proposal exists — accepting evaluations
    Voting,      // Votes are being cast
    Committed,   // Terminal — commitment recorded
}
```

### Message Types and Lifecycle

The Decision Mode accepts five message types, each with a corresponding protobuf payload type defined in `decision.proto`:

#### 1. Proposal

Creates a new proposal within the session.

**Payload (JSON-encoded `ProposalPayload`):**
```json
{
  "proposal_id": "p1",
  "option": "Deploy to production",
  "rationale": "All tests pass and staging looks good",
  "supporting_data": "<base64-encoded bytes>"
}
```

**Validation:**
- `proposal_id` must be non-empty — rejected with `InvalidPayload` if empty.
- A proposal with the same `proposal_id` overwrites the previous one.

**Effect:**
- Records the proposal in `state.proposals`.
- Advances the phase to `Evaluation` (enabling evaluations and votes).
- Returns `PersistState` with the updated state.

#### 2. Evaluation

Evaluates an existing proposal with a recommendation.

**Payload (JSON-encoded `EvaluationPayload`):**
```json
{
  "proposal_id": "p1",
  "recommendation": "APPROVE",
  "confidence": 0.95,
  "reason": "Implementation looks solid"
}
```

**Validation:**
- `proposal_id` must reference an existing proposal — rejected with `InvalidPayload` if not found.

**Recommendations:** `APPROVE`, `REVIEW`, `BLOCK`, `REJECT`

**Effect:**
- Appends the evaluation to `state.evaluations`.
- Returns `PersistState`.

#### 3. Objection

Raises an objection against a proposal.

**Payload (JSON-encoded `ObjectionPayload`):**
```json
{
  "proposal_id": "p1",
  "reason": "Security review not completed",
  "severity": "high"
}
```

**Validation:**
- `proposal_id` must reference an existing proposal — rejected with `InvalidPayload` if not found.

**Severities:** `low`, `medium`, `high`, `critical`

**Effect:**
- Appends the objection to `state.objections`.
- Returns `PersistState`.

#### 4. Vote

Casts a vote on the current proposals.

**Payload (JSON-encoded `VotePayload`):**
```json
{
  "proposal_id": "p1",
  "vote": "approve",
  "reason": "Looks good to me"
}
```

**Validation:**
- At least one proposal must exist — rejected with `InvalidPayload` if no proposals.
- Cannot vote when phase is `Committed` — rejected with `InvalidPayload`.

**Votes:** `approve`, `reject`, `abstain`

**Effect:**
- Records the vote in `state.votes`, keyed by sender. If the same sender votes again, the previous vote is overwritten.
- Advances the phase to `Voting`.
- Returns `PersistState`.

#### 5. Commitment

Finalizes the decision and resolves the session.

**Payload (JSON-encoded `CommitmentPayload`):**
```json
{
  "commitment_id": "c1",
  "action": "deploy-v2.1",
  "authority_scope": "team-alpha",
  "reason": "Unanimous approval"
}
```

**Validation:**
- At least one vote must exist — rejected with `InvalidPayload` if no votes.
- Phase must not already be `Committed` — rejected with `InvalidPayload` if so.

**Effect:**
- Advances the phase to `Committed`.
- Returns `PersistAndResolve` with the commitment payload as resolution bytes and the updated state.
- The session transitions to `Resolved`.

### Backward Compatibility (Legacy Resolve)

For backward compatibility with v0.1 clients, the Decision Mode also supports the legacy resolution mechanism: if the `message_type` is `"Message"` and the `payload` equals the bytes `b"resolve"`, the session is immediately resolved with `"resolve"` as the resolution payload. This allows old clients to continue working without modification.

Any other `Message`-type payload returns `NoOp`.

### Phase Transitions

```
                  Proposal received
                       │
  ┌──────────┐         ▼          ┌──────────────┐
  │ Proposal │ ──────────────────→│  Evaluation   │
  └──────────┘                    └──────────────┘
                                   │  ↑
                          Vote     │  │ Evaluation/Objection
                          received │  │ received
                                   ▼  │
                              ┌────────┐
                              │ Voting │
                              └────────┘
                                   │
                          Commitment received
                                   │
                                   ▼
                            ┌───────────┐
                            │ Committed │ (terminal)
                            └───────────┘
```

---

## Proposal Mode Specification

The Proposal Mode (`macp.mode.proposal.v1`) implements a lightweight propose/accept/reject lifecycle for peer-to-peer coordination. Unlike the Decision Mode's formal multi-phase process, the Proposal Mode is designed for simpler scenarios where one agent proposes and peers respond with acceptance or rejection.

### Participant Model: Peer

All participants are equal peers. Any participant can propose, and any participant can accept or reject. There is no distinguished orchestrator role.

### Determinism: Semantic-Deterministic

The mode produces deterministic outcomes based on the semantic content of messages -- the same sequence of proposals and responses always produces the same resolution.

### Message Types

| message_type | Description | Effect |
|-------------|-------------|--------|
| `Propose` | Submit a proposal for peer review | Records the proposal, returns `PersistState` |
| `Accept` | Accept the current proposal | Records acceptance; if acceptance threshold is met, returns `PersistAndResolve` |
| `Reject` | Reject the current proposal with a reason | Records rejection, returns `PersistState` |

### Lifecycle

```
Propose → Accept/Reject → ... → (all peers accept) → Resolved
```

A proposal is resolved when all declared participants have accepted it. Any rejection is recorded but does not automatically terminate the session -- a new proposal can be submitted to restart the cycle.

### Payload Formats

**Propose:**
```json
{
  "proposal_id": "p1",
  "content": "Suggested approach for the task",
  "rationale": "Why this approach makes sense"
}
```

**Accept:**
```json
{
  "proposal_id": "p1",
  "reason": "Looks good"
}
```

**Reject:**
```json
{
  "proposal_id": "p1",
  "reason": "Does not address requirement X"
}
```

---

## Task Mode Specification

The Task Mode (`macp.mode.task.v1`) implements orchestrated task assignment and completion tracking. An orchestrator assigns tasks to agents, and agents report progress and completion.

### Participant Model: Orchestrated

A single orchestrator creates and assigns tasks. Assigned agents execute tasks and report back. The orchestrator controls the lifecycle.

### Determinism: Structural-Only

The mode enforces structural invariants (task states, assignment rules) but does not interpret the semantic content of task payloads. Two different task contents that follow the same structural flow produce structurally equivalent state transitions.

### Message Types

| message_type | Description | Effect |
|-------------|-------------|--------|
| `Assign` | Orchestrator assigns a task to an agent | Records the task assignment, returns `PersistState` |
| `Progress` | Agent reports progress on an assigned task | Updates task progress, returns `PersistState` |
| `Complete` | Agent marks a task as complete | Records completion; if all tasks are complete, returns `PersistAndResolve` |
| `Cancel` | Orchestrator cancels a task | Marks the task as cancelled, returns `PersistState` |

### Lifecycle

```
Assign → Progress (optional, repeatable) → Complete/Cancel
```

The session resolves when all assigned tasks reach a terminal state (completed or cancelled).

### Payload Formats

**Assign:**
```json
{
  "task_id": "t1",
  "assignee": "agent-beta",
  "description": "Run integration tests on staging",
  "priority": "high"
}
```

**Progress:**
```json
{
  "task_id": "t1",
  "status": "running",
  "percent_complete": 60,
  "details": "Tests 120/200 passed so far"
}
```

**Complete:**
```json
{
  "task_id": "t1",
  "result": "All 200 tests passed",
  "artifacts": ""
}
```

**Cancel:**
```json
{
  "task_id": "t1",
  "reason": "Superseded by new requirements"
}
```

---

## Handoff Mode Specification

The Handoff Mode (`macp.mode.handoff.v1`) implements delegated context transfer between agents. One agent hands off responsibility (and context) to another agent, with the context frozen at the point of transfer.

### Participant Model: Delegated

A delegating agent initiates the handoff and a receiving agent accepts it. The context is frozen at transfer time -- no further modifications to the handed-off context are permitted.

### Determinism: Context-Frozen

Once a handoff is initiated, the context payload is immutable. The receiving agent can acknowledge or reject the handoff, but cannot modify the transferred context.

### Message Types

| message_type | Description | Effect |
|-------------|-------------|--------|
| `Initiate` | Delegating agent initiates a handoff with context | Records the handoff context, returns `PersistState` |
| `Acknowledge` | Receiving agent acknowledges the handoff | Records acknowledgment; resolves the session with the frozen context as resolution, returns `PersistAndResolve` |
| `Reject` | Receiving agent rejects the handoff | Records rejection with reason, returns `PersistState` |

### Lifecycle

```
Initiate → Acknowledge (resolved) or Reject
```

A handoff session resolves when the receiving agent acknowledges the transfer. If rejected, the delegating agent may initiate a new handoff (to the same or different agent) within the same session.

### Payload Formats

**Initiate:**
```json
{
  "handoff_id": "h1",
  "from": "agent-alpha",
  "to": "agent-beta",
  "context": "<serialized context data>",
  "reason": "Transferring ownership of the deployment pipeline"
}
```

**Acknowledge:**
```json
{
  "handoff_id": "h1",
  "accepted_by": "agent-beta",
  "notes": "Ready to take over"
}
```

**Reject:**
```json
{
  "handoff_id": "h1",
  "rejected_by": "agent-beta",
  "reason": "Not authorized for this resource"
}
```

---

## Quorum Mode Specification

The Quorum Mode (`macp.mode.quorum.v1`) implements threshold-based voting where resolution requires a configurable quorum of participants to agree. Unlike the Decision Mode's full lifecycle, the Quorum Mode focuses purely on reaching a voting threshold.

### Participant Model: Quorum

All declared participants can vote. Resolution occurs when the number of agreeing votes meets or exceeds the configured quorum threshold.

### Determinism: Semantic-Deterministic

The mode produces deterministic outcomes based on the votes cast -- the same set of votes always produces the same resolution.

### Message Types

| message_type | Description | Effect |
|-------------|-------------|--------|
| `Vote` | Cast a vote (approve/reject) | Records the vote; if quorum is reached, returns `PersistAndResolve` |
| `Abstain` | Explicitly abstain from voting | Records abstention (does not count toward quorum), returns `PersistState` |

### Lifecycle

```
Vote/Abstain → ... → (quorum reached) → Resolved
```

The session resolves when the number of `approve` votes meets or exceeds the quorum threshold. The quorum threshold is configured in the session start payload. If not specified, it defaults to a simple majority (more than half of declared participants).

### Payload Formats

**Vote:**
```json
{
  "vote": "approve",
  "reason": "Implementation meets all acceptance criteria"
}
```

**Abstain:**
```json
{
  "reason": "Conflict of interest"
}
```

### Quorum Configuration

The quorum threshold is set via the session start payload's configuration. For example, a session with 5 participants and a quorum of 3 resolves as soon as 3 participants vote `approve`.

---

## Multi-Round Mode Specification

The Multi-Round Mode (`macp.mode.multi_round.v1`) implements participant-based convergence. A set of named participants each submit contributions, and the session resolves automatically when all participants agree on the same value.

### Multi-Round State

```rust
pub struct MultiRoundState {
    pub round: u64,                                // Current round number
    pub participants: Vec<String>,                  // Expected participant IDs
    pub contributions: BTreeMap<String, String>,    // sender → current value
    pub convergence_type: String,                   // "all_equal"
}
```

The `BTreeMap` is used instead of `HashMap` for deterministic serialization ordering.

### SessionStart

On `SessionStart`, the mode:

1. Reads the `participants` list from the session (populated from `SessionStartPayload.participants`).
2. Validates that the participant list is non-empty — returns `InvalidPayload` if empty.
3. Initializes the state with `round: 0`, `convergence_type: "all_equal"`, and empty contributions.
4. Returns `PersistState` with the serialized initial state.

### Contribute Messages

The mode processes messages with `message_type: "Contribute"`.

**Payload (JSON):**
```json
{"value": "option_a"}
```

**Processing:**

1. Deserializes the current `mode_state` into `MultiRoundState`.
2. Parses the JSON payload to extract the `value` field.
3. Checks if the sender's value has changed from their previous contribution:
   - If this is a new contribution or the value differs from the previous one → **increment the round counter** and update the contribution.
   - If the value is identical to the previous one → update without incrementing the round (no change in substance).
4. Checks convergence: **all** listed participants have submitted at least one contribution, **and** all contribution values are identical.
5. If converged → returns `PersistAndResolve` with:
   - `state`: the final `MultiRoundState` serialized to JSON.
   - `resolution`: a JSON payload containing:
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
6. If not converged → returns `PersistState` with the updated state.

Non-`Contribute` messages return `NoOp`.

### Convergence Strategy: `all_equal`

The only currently supported convergence strategy. Resolution triggers when:

1. Every participant in the session's participant list has made at least one contribution.
2. All contribution values are identical.

If any participant has not contributed, or if any two contributions differ, convergence has not been reached and the session remains open.

### Round Counting

- Round starts at `0`.
- Each time a participant submits a **new or changed** value, the round increments by 1.
- Re-submitting the **same** value does not increment the round — this prevents artificial round inflation.
- The final round number in the resolution tells you how many substantive value changes occurred across all participants.

---

## Validation Rules (Complete)

The following validation rules are applied in order. The first failing rule produces the error; subsequent rules are not checked.

### 1. Protocol Version

```
IF macp_version != "1.0"
THEN reject with UNSUPPORTED_PROTOCOL_VERSION
```

This is checked in the gRPC adapter before any runtime processing.

### 2. Required Fields

```
IF message_type != "Signal":
    IF session_id is empty OR message_id is empty
    THEN reject with INVALID_ENVELOPE

IF message_type == "Signal":
    IF message_id is empty
    THEN reject with INVALID_ENVELOPE
    (session_id may be empty)
```

### 3. Mode Resolution (SessionStart)

```
IF message_type == "SessionStart":
    Resolve mode name (empty → "macp.mode.decision.v1")
    IF mode not in registered modes
    THEN reject with MODE_NOT_SUPPORTED
```

### 4. SessionStart Payload Parsing

```
IF message_type == "SessionStart":
    Decode payload as protobuf SessionStartPayload
    IF decode fails THEN reject with INVALID_ENVELOPE

    Extract ttl_ms from payload
    IF ttl_ms < 0 THEN reject with INVALID_ENVELOPE
    IF ttl_ms > 86,400,000 THEN reject with INVALID_ENVELOPE
    IF ttl_ms == 0 THEN use default (60,000 ms)
```

### 5. Session Existence (SessionStart)

```
IF message_type == "SessionStart":
    IF session already exists:
        IF message_id matches existing session's seen_message_ids
        THEN return ok=true, duplicate=true (idempotent)
        ELSE reject with INVALID_ENVELOPE (duplicate session)
```

### 6. Session Existence (Regular Messages)

```
IF message_type is not "SessionStart" and not "Signal":
    IF session does not exist
    THEN reject with SESSION_NOT_FOUND
```

### 7. Message Deduplication (Regular Messages)

```
IF message_id is in session.seen_message_ids
THEN return ok=true, duplicate=true (idempotent)
```

### 8. TTL Expiry Check

```
IF session.state == Open AND current_time > session.ttl_expiry:
    Log internal TtlExpired entry
    Transition session to Expired
    reject with SESSION_NOT_OPEN
```

### 9. Session State Check

```
IF session.state != Open
THEN reject with SESSION_NOT_OPEN
```

### 10. Participant Validation

```
IF session.participants is non-empty AND sender not in session.participants
THEN reject with INVALID_ENVELOPE
```

### 11. Mode Dispatch

```
Call mode.on_message(session, envelope)
IF mode returns Err(e) THEN reject with corresponding error code
ELSE apply ModeResponse
```

---

## Error Codes (Complete)

| RFC Error Code | Internal Error | When It Occurs |
|----------------|---------------|----------------|
| `UNSUPPORTED_PROTOCOL_VERSION` | `InvalidMacpVersion` | `macp_version` is not `"1.0"` |
| `INVALID_ENVELOPE` | `InvalidEnvelope` | Missing required fields, or invalid payload encoding |
| `INVALID_ENVELOPE` | `DuplicateSession` | SessionStart for existing session (different message_id) |
| `INVALID_ENVELOPE` | `InvalidTtl` | TTL value out of range (< 0 or > 24h) |
| `INVALID_ENVELOPE` | `InvalidModeState` | Internal mode state cannot be deserialized |
| `INVALID_ENVELOPE` | `InvalidPayload` | Payload does not match mode's expected format |
| `SESSION_NOT_FOUND` | `UnknownSession` | Message for non-existent session |
| `SESSION_NOT_OPEN` | `SessionNotOpen` | Message to resolved or expired session |
| `SESSION_NOT_OPEN` | `TtlExpired` | Session TTL has elapsed |
| `MODE_NOT_SUPPORTED` | `UnknownMode` | Mode field references unregistered mode |
| `FORBIDDEN` | `Forbidden` | Operation not permitted |
| `UNAUTHENTICATED` | `Unauthenticated` | Authentication required |
| `DUPLICATE_MESSAGE` | `DuplicateMessage` | Explicit duplicate detection (distinct from idempotent dedup) |
| `PAYLOAD_TOO_LARGE` | `PayloadTooLarge` | Payload exceeds size limits |
| `RATE_LIMITED` | `RateLimited` | Too many requests |

Note that several internal error variants map to `INVALID_ENVELOPE` — this groups related validation failures under a single client-facing code while preserving distinct internal error variants for logging and debugging.

---

## Transport

The protocol uses **gRPC over HTTP/2**:

- **Binary protocol** — efficient serialization via protobuf.
- **Type-safe** — schema enforcement at compile time.
- **Streaming support** — bidirectional streaming via `StreamSession`.
- **Wide language support** — gRPC clients available for Python, JavaScript, Go, Java, C++, and more.
- **Built-in TLS** — secure transport via standard gRPC TLS configuration.

**Default address:** `127.0.0.1:50051` (hardcoded in `src/main.rs`).

---

## Best Practices

### For Clients

1. **Always call Initialize first** — Negotiate the protocol version and discover capabilities before sending session messages.

2. **Check `Ack.ok` and `Ack.error`** — Don't just check the boolean; inspect the `MACPError.code` for specific error handling.

3. **Use unique message IDs** — UUIDs are recommended. This enables safe retries via the deduplication mechanism.

4. **Handle duplicates gracefully** — If `Ack.duplicate` is `true`, the message was already processed. Treat this as success.

5. **Send SessionStart first** — Before any other messages for a session.

6. **Respect terminal states** — Once a session is `RESOLVED` or `EXPIRED`, don't send more messages. Cache the state locally.

7. **Use CancelSession for cleanup** — Don't let sessions hang until TTL expiry if you know the coordination is over.

8. **Use ListModes for discovery** — Query available modes and their message types before creating sessions.

9. **Use GetSession to check state** — Useful for resuming after disconnection or verifying session state.

10. **Declare participants when appropriate** — Use the `participants` field in `SessionStartPayload` to restrict who can contribute, especially for convergence-based modes.

---

## Future Extensions

### 1. Background TTL Cleanup
Currently, TTL is enforced lazily. Future versions will run a background eviction task.

### 2. Replay Engine
Replay session logs to reconstruct state for debugging and auditing.

### 3. GetSessionLog RPC
Query session event logs for audit trails.

### 4. Additional Convergence Strategies
- `majority` — resolve when a majority of participants agree.
- `threshold` — resolve when N participants agree.
- `weighted` — resolve based on weighted votes.

### 5. Persistent Storage
Durable session state and log storage (e.g., to SQLite or Postgres).

### 6. Authentication and Authorization
Token-based authentication and role-based access control for sessions.

---

## Next Steps

- Read **[architecture.md](./architecture.md)** to understand how this is implemented internally.
- Read **[examples.md](./examples.md)** for practical code examples with the new v0.2 RPCs.
