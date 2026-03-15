# Architecture

This document explains how the MACP Runtime v0.3 is built internally. It walks through every component, every data structure, every flow, and every design decision in narrative detail. You do not need to know Rust to follow along — the documentation explains concepts in plain language, with code excerpts for precision where it matters.

---

## Table of Contents

1. [System Overview](#system-overview)
2. [Protobuf Schema Layer](#protobuf-schema-layer)
3. [Build System](#build-system)
4. [Entry Point (main.rs)](#entry-point-mainrs)
5. [Library Root (lib.rs)](#library-root-librs)
6. [Error Types (error.rs)](#error-types-errorrs)
7. [Session Types (session.rs)](#session-types-sessionrs)
8. [Session Registry (registry.rs)](#session-registry-registryrs)
9. [Log Store (log_store.rs)](#log-store-log_storers)
10. [Mode System (mode/)](#mode-system-mode)
11. [Runtime Kernel (runtime.rs)](#runtime-kernel-runtimers)
12. [gRPC Server Adapter (server.rs)](#grpc-server-adapter-serverrs)
13. [Data Flow: Complete Message Processing](#data-flow-complete-message-processing)
14. [Data Flow: Session Cancellation](#data-flow-session-cancellation)
15. [Data Flow: Initialize Handshake](#data-flow-initialize-handshake)
16. [Concurrency Model](#concurrency-model)
17. [File Structure](#file-structure)
18. [Build Process](#build-process)
19. [CI/CD Pipeline](#cicd-pipeline)
20. [Design Principles](#design-principles)

---

## System Overview

```
┌──────────────────────────────────────────────────────────────┐
│                        Clients                               │
│  (AI agents, test programs, any gRPC-capable application)    │
└─────────┬──────────────────┬──────────────────┬──────────────┘
          │ Initialize       │ Send / Stream    │ GetSession /
          │                  │                  │ CancelSession /
          │                  │                  │ ListModes / ...
          ▼                  ▼                  ▼
┌──────────────────────────────────────────────────────────────┐
│               MACP Runtime Server (v0.2)                     │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │        MacpServer (gRPC Adapter Layer)                 │  │
│  │  - Implements MACPRuntimeService (10 RPCs)             │  │
│  │  - Validates transport-level fields (version, IDs)     │  │
│  │  - Builds structured Ack responses with MACPError      │  │
│  │  - Delegates all coordination logic to Runtime         │  │
│  └───────────────────────┬────────────────────────────────┘  │
│                          │                                    │
│                          ▼                                    │
│  ┌────────────────────────────────────────────────────────┐  │
│  │            Runtime (Coordination Kernel)               │  │
│  │  - Routes messages by type (SessionStart/Signal/other) │  │
│  │  - Resolves mode names to implementations              │  │
│  │  - Enforces TTL, session state, participant validation │  │
│  │  - Handles message deduplication                       │  │
│  │  - Dispatches to Mode implementations                  │  │
│  │  - Applies ModeResponse as single mutation point       │  │
│  │  - Manages session cancellation                        │  │
│  └──────┬────────────────────────────┬────────────────────┘  │
│         │                            │                        │
│         ▼                            ▼                        │
│  ┌──────────────────┐  ┌──────────────────────────────┐      │
│  │ Mode Registry    │  │ Mode Implementations         │      │
│  │ HashMap<name,    │  │                              │      │
│  │   Box<dyn Mode>> │  │ DecisionMode                 │      │
│  │                  │  │  - RFC lifecycle              │      │
│  │ 4 entries:       │  │  - Proposal/Eval/Vote/Commit │      │
│  │  decision (x2)   │  │  - Phase tracking            │      │
│  │  multi_round(x2) │  │                              │      │
│  │                  │  │ MultiRoundMode               │      │
│  │                  │  │  - Convergence checking       │      │
│  │                  │  │  - Round counting             │      │
│  └──────────────────┘  └──────────────────────────────┘      │
│         │                                                     │
│         ▼                                                     │
│  ┌────────────────────────────────────────────────────────┐  │
│  │  SessionRegistry              LogStore                │  │
│  │  RwLock<HashMap<              RwLock<HashMap<          │  │
│  │    String, Session>>            String, Vec<LogEntry>>>│  │
│  │                                                        │  │
│  │  Thread-safe session          Append-only per-session  │  │
│  │  storage with read/write      event log with Incoming  │  │
│  │  locking                      and Internal entries     │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

The architecture follows a strict layered design:

1. **Transport layer** (`MacpServer`) — handles gRPC protocol concerns, validates transport-level fields, builds structured responses.
2. **Coordination layer** (`Runtime`) — enforces protocol invariants, routes messages, manages session lifecycle, dispatches to modes.
3. **Logic layer** (`Mode` implementations) — provides coordination-specific behavior, returns declarative `ModeResponse` values.
4. **Storage layer** (`SessionRegistry`, `LogStore`) — provides thread-safe state persistence.

Each layer has a single responsibility and communicates through well-defined interfaces.

---

## Protobuf Schema Layer

### Schema Organization

The protocol schema has been restructured from a single `macp.proto` file (v0.1) into a modular, concern-separated layout:

```
proto/
├── buf.yaml                                    # Linting and breaking-change config
└── macp/
    ├── v1/
    │   ├── envelope.proto                      # Foundational types
    │   └── core.proto                          # Service + all message types
    └── modes/
        └── decision/
            └── v1/
                └── decision.proto              # Decision mode payloads
```

**`envelope.proto`** defines the four foundational types that everything builds on:

- **`Envelope`** — the universal message wrapper with 8 fields (macp_version, mode, message_type, message_id, session_id, sender, timestamp_unix_ms, payload).
- **`Ack`** — the structured acknowledgment with 7 fields (ok, duplicate, message_id, session_id, accepted_at_unix_ms, session_state, error).
- **`MACPError`** — the structured error type with 5 fields (code, message, session_id, message_id, details).
- **`SessionState`** — the enum with 4 values (UNSPECIFIED, OPEN, RESOLVED, EXPIRED).

**`core.proto`** imports `envelope.proto` and defines everything else:

- **Capability messages** — `ClientInfo`, `RuntimeInfo`, `Capabilities` (with sub-capabilities for sessions, cancellation, progress, manifest, mode registry, roots, and experimental features).
- **Initialize** — `InitializeRequest` and `InitializeResponse` for protocol handshake.
- **Session payloads** — `SessionStartPayload` (with intent, participants, versions, ttl_ms, context, roots), `SessionCancelPayload`, `CommitmentPayload`.
- **Coordination payloads** — `SignalPayload`, `ProgressPayload`.
- **Session metadata** — `SessionMetadata` with typed state and version fields.
- **Introspection** — `AgentManifest`, `ModeDescriptor`.
- **Request/Response wrappers** — `SendRequest`/`SendResponse`, `GetSessionRequest`/`GetSessionResponse`, `CancelSessionRequest`/`CancelSessionResponse`, etc.
- **Streaming types** — `StreamSessionRequest`/`StreamSessionResponse`.
- **Watch types** — `RegistryChanged`, `RootsChanged`.
- **Service definition** — `MACPRuntimeService` with 10 RPCs.

**`decision.proto`** defines mode-specific payload types:

- **`ProposalPayload`** — proposal_id, option, rationale, supporting_data.
- **`EvaluationPayload`** — proposal_id, recommendation, confidence, reason.
- **`ObjectionPayload`** — proposal_id, reason, severity.
- **`VotePayload`** — proposal_id, vote, reason.

These types are not referenced by the core proto — they exist as domain schemas for clients and the Decision Mode implementation. The `CommitmentPayload` is defined in `core.proto` because it is reused across modes.

### Buf Configuration

The `buf.yaml` file configures:

- **Lint rules:** `STANDARD` — enforces naming conventions, field numbering, and other best practices.
- **Breaking-change detection:** `FILE` — detects breaking changes at the file level, ensuring backward compatibility as the schema evolves.

---

## Build System

### build.rs

The `build.rs` script runs before compilation and uses `tonic-build` to generate Rust code from the protobuf files:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_server(true).compile(
        &[
            "macp/v1/envelope.proto",
            "macp/v1/core.proto",
            "macp/modes/decision/v1/decision.proto",
        ],
        &["proto"],
    )?;
    Ok(())
}
```

This generates two Rust modules:
- `macp.v1` — all core types and the gRPC service server/client stubs.
- `macp.modes.decision.v1` — decision mode payload types.

The generated code is included in the binary via `tonic::include_proto!()` macros in `lib.rs`.

### Cargo.toml Dependencies

| Dependency | Purpose |
|------------|---------|
| `tokio` | Async runtime (full features) |
| `tonic` | gRPC framework |
| `prost` | Protobuf serialization/deserialization |
| `prost-types` | Well-known protobuf types |
| `uuid` | UUID generation (v4) |
| `thiserror` | Ergonomic error type derivation |
| `chrono` | Date/time handling |
| `serde` + `serde_json` | JSON serialization for mode state and payloads |
| `tokio-stream` | Stream utilities for async streaming |
| `futures-core` | Core future/stream traits |
| `async-stream` | Macro for creating async streams |

### Makefile

The `Makefile` provides development shortcuts:

```makefile
setup:    # Configure git hooks (points to .githooks/)
build:    # cargo build
test:     # cargo test
fmt:      # cargo fmt --all
clippy:   # cargo clippy --all-targets -- -D warnings
check:    # fmt + clippy + test (full CI check locally)
```

---

## Entry Point (main.rs)

The `main.rs` file is deliberately minimal — it wires up the three core components and starts the gRPC server:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:50051".parse()?;
    let registry = Arc::new(SessionRegistry::new());
    let log_store = Arc::new(LogStore::new());
    let runtime = Arc::new(Runtime::new(registry, log_store));
    let svc = MacpServer::new(runtime);

    println!("macp-runtime v0.2 (RFC-0001) listening on {}", addr);

    Server::builder()
        .add_service(pb::macp_runtime_service_server::MacpRuntimeServiceServer::new(svc))
        .serve(addr)
        .await?;

    Ok(())
}
```

**What happens on startup:**

1. The address `127.0.0.1:50051` is parsed.
2. A `SessionRegistry` is created — an empty, thread-safe hashmap for storing sessions.
3. A `LogStore` is created — an empty, thread-safe hashmap for storing per-session event logs.
4. A `Runtime` is created — it takes ownership of the registry and log store (via `Arc`), and registers the built-in modes (DecisionMode and MultiRoundMode with both RFC names and backward-compatible aliases).
5. A `MacpServer` is created — the gRPC adapter wrapping the runtime.
6. Tonic's gRPC `Server` is started, listening on the configured address.

The `Arc` wrapper allows the runtime to be shared across all async tasks spawned by the Tokio runtime — each gRPC handler receives a clone of the `Arc<Runtime>`.

---

## Library Root (lib.rs)

```rust
pub mod pb {
    tonic::include_proto!("macp.v1");
}

pub mod decision_pb {
    tonic::include_proto!("macp.modes.decision.v1");
}

pub mod error;
pub mod log_store;
pub mod mode;
pub mod registry;
pub mod runtime;
pub mod session;
```

The library root serves two purposes:

1. **Proto module inclusion** — The `pb` module contains all generated code from `envelope.proto` and `core.proto`. The `decision_pb` module contains generated code from `decision.proto`. These are available to both the server binary and the client binaries.

2. **Module re-exports** — All internal modules are made public so that client binaries (in `src/bin/`) can import types like `SessionStartPayload`, `Envelope`, etc.

---

## Error Types (error.rs)

The error system is designed to provide both internal precision (distinct Rust error variants for each failure mode) and external clarity (RFC-compliant error codes for clients).

### MacpError Enum

```rust
pub enum MacpError {
    InvalidMacpVersion,    // Protocol version mismatch
    InvalidEnvelope,       // Missing required fields or invalid encoding
    DuplicateSession,      // SessionStart for existing session
    UnknownSession,        // Message for non-existent session
    SessionNotOpen,        // Message to resolved/expired session
    TtlExpired,            // Session TTL has elapsed
    InvalidTtl,            // TTL value out of range
    UnknownMode,           // Mode not registered
    InvalidModeState,      // Mode state deserialization failure
    InvalidPayload,        // Payload doesn't match mode's expectations
    Forbidden,             // Operation not permitted
    Unauthenticated,       // Authentication required
    DuplicateMessage,      // Explicit duplicate detection
    PayloadTooLarge,       // Payload exceeds size limits
    RateLimited,           // Too many requests
}
```

### RFC Error Code Mapping

Each variant maps to an RFC-compliant string code via the `error_code()` method:

| Variant | RFC Code |
|---------|----------|
| `InvalidMacpVersion` | `"UNSUPPORTED_PROTOCOL_VERSION"` |
| `InvalidEnvelope` | `"INVALID_ENVELOPE"` |
| `DuplicateSession` | `"INVALID_ENVELOPE"` |
| `UnknownSession` | `"SESSION_NOT_FOUND"` |
| `SessionNotOpen` | `"SESSION_NOT_OPEN"` |
| `TtlExpired` | `"SESSION_NOT_OPEN"` |
| `InvalidTtl` | `"INVALID_ENVELOPE"` |
| `UnknownMode` | `"MODE_NOT_SUPPORTED"` |
| `InvalidModeState` | `"INVALID_ENVELOPE"` |
| `InvalidPayload` | `"INVALID_ENVELOPE"` |
| `Forbidden` | `"FORBIDDEN"` |
| `Unauthenticated` | `"UNAUTHENTICATED"` |
| `DuplicateMessage` | `"DUPLICATE_MESSAGE"` |
| `PayloadTooLarge` | `"PAYLOAD_TOO_LARGE"` |
| `RateLimited` | `"RATE_LIMITED"` |

**Design rationale:** Multiple internal variants map to `INVALID_ENVELOPE` because, from a client's perspective, these are all "your request was malformed." The distinct internal variants allow for precise logging, metrics, and debugging. The `Display` trait implementation uses the variant names directly (e.g., `"InvalidMacpVersion"`), providing human-readable error messages in log output.

---

## Session Types (session.rs)

### SessionState Enum

```rust
pub enum SessionState {
    Open,      // Active — accepting messages
    Resolved,  // Terminal — mode resolved the session
    Expired,   // Terminal — TTL elapsed or cancelled
}
```

### Session Struct

```rust
pub struct Session {
    pub session_id: String,
    pub state: SessionState,
    pub ttl_expiry: i64,                    // Unix ms when session expires
    pub started_at_unix_ms: i64,            // Unix ms when session was created
    pub resolution: Option<Vec<u8>>,        // Final outcome (if Resolved)
    pub mode: String,                       // Mode name
    pub mode_state: Vec<u8>,                // Mode-specific serialized state
    pub participants: Vec<String>,          // Allowed senders (empty = open)
    pub seen_message_ids: HashSet<String>,  // For deduplication

    // RFC version fields from SessionStartPayload
    pub intent: String,
    pub mode_version: String,
    pub configuration_version: String,
    pub policy_version: String,
}
```

**Key fields explained:**

- **`mode_state`** — Opaque bytes owned by the mode. The runtime never inspects these; it simply stores whatever the mode returns in `PersistState`. Each mode serializes/deserializes its own state format (JSON for both built-in modes).

- **`seen_message_ids`** — A `HashSet<String>` tracking every `message_id` that has been accepted for this session. Used for deduplication — if a message arrives with a `message_id` already in this set, it is returned as a duplicate without re-processing.

- **`participants`** — If non-empty, only senders in this list may send messages to the session. This is populated from `SessionStartPayload.participants`.

- **`started_at_unix_ms`** — Records when the session was created (server-side timestamp). Used in `GetSession` responses.

- **Version fields** (`intent`, `mode_version`, `configuration_version`, `policy_version`) — Carried from the `SessionStartPayload` and returned in `GetSession` responses. The runtime does not interpret these; they exist for client-side versioning and policy tracking.

### TTL Parsing

Two functions handle TTL extraction from the protobuf payload:

**`parse_session_start_payload(payload: &[u8])`** — Decodes the raw bytes as a protobuf `SessionStartPayload`. If the payload is empty, returns a default `SessionStartPayload` (all fields at their protobuf defaults).

**`extract_ttl_ms(payload: &SessionStartPayload)`** — Returns the `ttl_ms` field, or the default `60,000 ms` if the field is `0`. Validates that the value is in range `[1, 86,400,000]`. Returns `Err(MacpError::InvalidTtl)` if out of range.

**Constants:**
- `DEFAULT_TTL_MS: i64 = 60_000` (60 seconds)
- `MAX_TTL_MS: i64 = 86_400_000` (24 hours)

---

## Session Registry (registry.rs)

```rust
pub struct SessionRegistry {
    pub(crate) sessions: RwLock<HashMap<String, Session>>,
}
```

The `SessionRegistry` is the in-memory session store. It wraps a `HashMap<String, Session>` in a Tokio `RwLock` for thread-safe concurrent access.

**Methods:**

- **`new()`** — Creates an empty registry.
- **`get_session(session_id: &str) -> Option<Session>`** — Acquires a read lock and returns a clone of the session if found.
- **`insert_session_for_test(session: Session)`** — Acquires a write lock and inserts a session. Used only in tests.

**Important note:** The registry's `RwLock` is also directly accessed by the `Runtime` for atomic read-modify-write operations. During `process_session_start()` and `process_message()`, the runtime acquires a **write lock** on the registry and holds it for the entire processing sequence (including mode dispatch). This ensures atomicity but creates a potential concurrency bottleneck for high-throughput scenarios.

---

## Log Store (log_store.rs)

The `LogStore` maintains an append-only audit log per session, providing a complete history of every event that occurred within a session.

### LogEntry

```rust
pub struct LogEntry {
    pub message_id: String,
    pub received_at_ms: i64,
    pub sender: String,
    pub message_type: String,
    pub raw_payload: Vec<u8>,
    pub entry_kind: EntryKind,
}

pub enum EntryKind {
    Incoming,   // Message from a client
    Internal,   // Runtime-generated event
}
```

**Entry kinds:**

- **Incoming** — Every client message (SessionStart, Proposal, Vote, Contribute, etc.) is logged as an `Incoming` entry before the mode processes it. This is the "log-before-mutate" guarantee.
- **Internal** — Runtime-generated events like `TtlExpired` (when TTL expiry is detected) and `SessionCancel` (when `CancelSession` is called) are logged as `Internal` entries. These have synthetic `message_id` values like `"__ttl_expired__"` or `"__session_cancel__"`.

### LogStore Structure

```rust
pub struct LogStore {
    logs: RwLock<HashMap<String, Vec<LogEntry>>>,
}
```

**Methods:**

- **`new()`** — Creates an empty store.
- **`create_session_log(session_id: &str)`** — Creates an empty log vector for a session. Idempotent — if the log already exists, this is a no-op.
- **`append(session_id: &str, entry: LogEntry)`** — Appends an entry to the session's log. If no log exists for the session, one is auto-created.
- **`get_log(session_id: &str) -> Option<Vec<LogEntry>>`** — Returns a cloned copy of the session's log entries.

**Design properties:**
- Entries are never deleted or modified — the log is strictly append-only.
- Entries are appended in strict chronological order per session.
- The log persists for the lifetime of the server process (no cleanup).
- Future extensions (replay engine, GetSessionLog RPC) will build on this foundation.

---

## Mode System (mode/)

### Mode Trait (mode/mod.rs)

```rust
pub trait Mode: Send + Sync {
    fn on_session_start(&self, session: &Session, env: &Envelope)
        -> Result<ModeResponse, MacpError>;
    fn on_message(&self, session: &Session, env: &Envelope)
        -> Result<ModeResponse, MacpError>;
}
```

The `Mode` trait is the extension point for coordination logic. It is designed around three principles:

1. **Immutability** — Modes receive `&Session` (immutable reference). They cannot directly mutate state.
2. **Declarative responses** — Modes return `ModeResponse` values that describe *what should change*, not *how to change it*.
3. **Thread safety** — The `Send + Sync` bounds ensure modes can be shared across async tasks.

### ModeResponse (mode/mod.rs)

```rust
pub enum ModeResponse {
    NoOp,
    PersistState(Vec<u8>),
    Resolve(Vec<u8>),
    PersistAndResolve { state: Vec<u8>, resolution: Vec<u8> },
}
```

The runtime's `apply_mode_response()` is the single mutation point that interprets these responses:

- **`NoOp`** — Nothing happens. The message was accepted but produced no state change.
- **`PersistState(bytes)`** — `session.mode_state = bytes`. The mode's internal state is updated.
- **`Resolve(bytes)`** — `session.state = Resolved` and `session.resolution = Some(bytes)`. The session terminates with a resolution.
- **`PersistAndResolve { state, resolution }`** — Both of the above in a single atomic operation.

### DecisionMode (mode/decision.rs)

The Decision Mode implements the RFC-0001 decision lifecycle. It maintains a `DecisionState` serialized as JSON in `session.mode_state`.

**Internal state:**

```rust
pub struct DecisionState {
    pub proposals: HashMap<String, Proposal>,
    pub evaluations: Vec<Evaluation>,
    pub objections: Vec<Objection>,
    pub votes: HashMap<String, Vote>,  // sender → Vote
    pub phase: DecisionPhase,
}

pub enum DecisionPhase {
    Proposal,     // Initial — waiting for proposals
    Evaluation,   // At least one proposal exists
    Voting,       // Votes being cast
    Committed,    // Terminal — decision finalized
}
```

**Message routing in `on_message()`:**

The mode inspects `envelope.message_type` and dispatches accordingly:

| message_type | Handler | Returns |
|-------------|---------|---------|
| `"Proposal"` | Parse `ProposalPayload`, validate `proposal_id`, store proposal, advance to `Evaluation` | `PersistState` |
| `"Evaluation"` | Parse `EvaluationPayload`, validate `proposal_id` exists, append evaluation | `PersistState` |
| `"Objection"` | Parse `ObjectionPayload`, validate `proposal_id` exists, append objection | `PersistState` |
| `"Vote"` | Parse `VotePayload`, validate proposals exist, store vote (keyed by sender — overwrites), advance to `Voting` | `PersistState` |
| `"Commitment"` | Parse `CommitmentPayload`, validate votes exist, advance to `Committed` | `PersistAndResolve` |
| `"Message"` with payload `b"resolve"` | Legacy backward compatibility | `Resolve(b"resolve")` |
| Anything else | Ignored | `NoOp` |

**Key behaviors:**

- Proposals are stored in a `HashMap<String, Proposal>` keyed by `proposal_id`. Submitting a new proposal with the same ID overwrites the previous one.
- Votes are stored in a `HashMap<String, Vote>` keyed by sender. If the same sender votes again, the previous vote is replaced.
- Phase transitions are one-way: `Proposal → Evaluation → Voting → Committed`.
- The `Commitment` message is the terminal message — it resolves the session.

### MultiRoundMode (mode/multi_round.rs)

The Multi-Round Mode implements participant-based convergence. It maintains a `MultiRoundState` serialized as JSON in `session.mode_state`.

**Internal state:**

```rust
pub struct MultiRoundState {
    pub round: u64,
    pub participants: Vec<String>,
    pub contributions: BTreeMap<String, String>,  // sender → value
    pub convergence_type: String,                 // "all_equal"
}
```

The `BTreeMap` is used instead of `HashMap` for **deterministic serialization ordering** — this ensures that the same state always produces the same JSON bytes, enabling reliable comparison and replay.

**`on_session_start()` flow:**

1. Reads `session.participants` (populated from `SessionStartPayload.participants`).
2. If participants is empty, returns `Err(MacpError::InvalidPayload)` — multi-round mode requires at least one participant.
3. Creates initial `MultiRoundState` with `round: 0`, the participant list, empty contributions, and `convergence_type: "all_equal"`.
4. Serializes and returns `PersistState`.

**`on_message()` flow for `Contribute` messages:**

1. Deserializes `mode_state` into `MultiRoundState`.
2. Parses the JSON payload `{"value": "<string>"}`.
3. Checks if the sender's value has changed:
   - **New contribution** (sender not in `contributions`) → insert and increment round.
   - **Changed value** (sender's previous value differs) → update and increment round.
   - **Same value** (sender resubmits identical value) → update without incrementing round.
4. Checks convergence:
   - All participants have contributed (every participant in the list has an entry in `contributions`).
   - All contribution values are identical.
5. If converged → `PersistAndResolve` with resolution `{"converged_value": "...", "round": N, "final_values": {...}}`.
6. If not converged → `PersistState` with updated state.

**Participant extraction note:** The mode tries to read participants from the session first. If the session's participant list is empty (which shouldn't normally happen for multi_round), the runtime attempts to extract participants from the `mode_state` as a fallback.

---

## Runtime Kernel (runtime.rs)

The `Runtime` is the coordination kernel — the central orchestrator that ties everything together. It holds the session registry, the log store, and the registered modes.

### Structure

```rust
pub struct Runtime {
    pub registry: Arc<SessionRegistry>,
    pub log_store: Arc<LogStore>,
    modes: HashMap<String, Box<dyn Mode>>,
}

pub struct ProcessResult {
    pub session_state: SessionState,
    pub duplicate: bool,
}
```

### Mode Registration

On construction, the runtime registers four entries:

```rust
modes.insert("macp.mode.decision.v1", DecisionMode);
modes.insert("macp.mode.multi_round.v1", MultiRoundMode);
modes.insert("decision", DecisionMode);          // backward-compatible alias
modes.insert("multi_round", MultiRoundMode);      // backward-compatible alias
```

The `mode_names()` method returns the canonical mode names (the RFC-compliant ones, not the aliases).

### Message Routing: process()

The `process()` method is the main entry point. It inspects `envelope.message_type` and routes to the appropriate handler:

```rust
pub async fn process(&self, env: &Envelope) -> Result<ProcessResult, MacpError> {
    match env.message_type.as_str() {
        "SessionStart" => self.process_session_start(env).await,
        "Signal"       => self.process_signal(env).await,
        _              => self.process_message(env).await,
    }
}
```

### process_session_start()

This is the most complex handler. Here is the complete flow:

1. **Resolve mode** — Empty mode field → `"macp.mode.decision.v1"`. Look up mode in registry → `MODE_NOT_SUPPORTED` if not found.
2. **Parse payload** — Decode bytes as protobuf `SessionStartPayload` → `INVALID_ENVELOPE` if decode fails.
3. **Validate TTL** — Extract `ttl_ms`, validate range → `INVALID_ENVELOPE` if out of range.
4. **Compute TTL expiry** — `current_time + ttl_ms`.
5. **Acquire write lock** on session registry.
6. **Check for duplicate session:**
   - If session exists and `message_id` is in `seen_message_ids` → return `ProcessResult { state, duplicate: true }`.
   - If session exists with different `message_id` → return `Err(MacpError::DuplicateSession)`.
7. **Create session log** — `log_store.create_session_log(session_id)`.
8. **Log incoming entry** — Append `Incoming` entry with the SessionStart details.
9. **Create Session object** — state=Open, computed TTL expiry, participants, version metadata, `message_id` in `seen_message_ids`.
10. **Call mode.on_session_start()** — Mode may return `PersistState` with initial state.
11. **Apply ModeResponse** — Mutate session according to the response.
12. **Insert session** into registry.
13. **Return ProcessResult** — state=Open (or Resolved if mode immediately resolved), duplicate=false.

### process_message()

Handles all non-SessionStart, non-Signal messages:

1. **Acquire write lock** on session registry.
2. **Find session** → `SESSION_NOT_FOUND` if not found.
3. **Deduplication check** — If `message_id` in `seen_message_ids` → return `ProcessResult { state, duplicate: true }`.
4. **TTL check** — If session is Open and `now > ttl_expiry`:
   - Log internal `TtlExpired` entry.
   - Transition session to `Expired`.
   - Return `Err(MacpError::TtlExpired)`.
5. **State check** — If session is not `Open` → `Err(MacpError::SessionNotOpen)`.
6. **Participant check** — If `participants` is non-empty and `sender` not in list → `Err(MacpError::InvalidEnvelope)`.
7. **Record message_id** in `seen_message_ids`.
8. **Log incoming entry**.
9. **Look up mode** → `MODE_NOT_SUPPORTED` if not found (should not happen for valid sessions).
10. **Participant extraction fallback** — For multi_round mode, if session participants is empty, try to extract from mode_state.
11. **Call mode.on_message()**.
12. **Apply ModeResponse**.
13. **Return ProcessResult** with current session state.

### process_signal()

Signal handling is deliberately simple:

1. Accept the signal (no session lookup, no state mutation).
2. Return `ProcessResult { state: Open, duplicate: false }`.

### apply_mode_response()

The single mutation point for all session state changes:

```rust
fn apply_mode_response(session: &mut Session, response: ModeResponse) {
    match response {
        ModeResponse::NoOp => {}
        ModeResponse::PersistState(s) => {
            session.mode_state = s;
        }
        ModeResponse::Resolve(r) => {
            session.state = SessionState::Resolved;
            session.resolution = Some(r);
        }
        ModeResponse::PersistAndResolve { state, resolution } => {
            session.mode_state = state;
            session.state = SessionState::Resolved;
            session.resolution = Some(resolution);
        }
    }
}
```

### cancel_session()

Session cancellation flow:

1. **Acquire write lock** on session registry.
2. **Find session** → `Err(MacpError::UnknownSession)` if not found.
3. **Check state:**
   - If already `Resolved` or `Expired` → idempotent, return `Ok(())`.
   - If `Open` → log internal `SessionCancel` entry, transition to `Expired`.
4. **Return `Ok(())`**.

---

## gRPC Server Adapter (server.rs)

The `MacpServer` struct implements the `MACPRuntimeService` gRPC trait generated by tonic. It is a thin adapter layer that translates between gRPC types and the runtime's internal types.

### Structure

```rust
pub struct MacpServer {
    runtime: Arc<Runtime>,
}
```

### Key Methods

**`validate(env: &Envelope)`** — Transport-level validation:
- Checks `macp_version == "1.0"` → `UNSUPPORTED_PROTOCOL_VERSION`.
- For non-Signal messages: checks `session_id` and `message_id` are non-empty → `INVALID_ENVELOPE`.
- For Signal messages: checks only `message_id` is non-empty.

**`session_state_to_pb(state: &SessionState) -> i32`** — Maps internal session state to protobuf enum values.

**`make_error_ack(err: &MacpError, env: &Envelope) -> Ack`** — Constructs a structured `Ack` with:
- `ok: false`
- `message_id` and `session_id` from the envelope
- `accepted_at_unix_ms` set to current time
- `error` containing a `MACPError` with the RFC error code and the error's display string

### RPC Implementations

| RPC | Behavior |
|-----|----------|
| **Initialize** | Checks `supported_protocol_versions` for `"1.0"`, returns `RuntimeInfo`, `Capabilities`, supported modes, and instructions. Returns `INVALID_ARGUMENT` gRPC status if no supported version. |
| **Send** | Validates envelope, delegates to `runtime.process()`, constructs `Ack` with ok/duplicate/session_state/error. All protocol errors are in-band (gRPC status is always OK). |
| **GetSession** | Looks up session in registry, returns `SessionMetadata` with state, timestamps, and version fields. Returns `NOT_FOUND` gRPC status if session doesn't exist. |
| **CancelSession** | Delegates to `runtime.cancel_session()`, returns `Ack`. |
| **GetManifest** | Returns `AgentManifest` with runtime identity and supported modes. |
| **ListModes** | Returns two `ModeDescriptor` entries (decision and multi_round) with message types, determinism class, and participant model. |
| **StreamSession** | Bidirectional streaming — processes each incoming envelope and echoes an envelope with updated message_type. |
| **ListRoots** | Returns empty roots list. |
| **WatchModeRegistry** | Returns `UNIMPLEMENTED`. |
| **WatchRoots** | Returns `UNIMPLEMENTED`. |

---

## Data Flow: Complete Message Processing

Let's trace the complete path of a `Vote` message through the system:

```
1. Client sends SendRequest { envelope: Vote message }
        │
        ▼
2. MacpServer::send() receives the gRPC request
        │
        ▼
3. MacpServer::validate(&envelope)
   - Checks macp_version == "1.0"           ✓
   - Checks session_id non-empty             ✓
   - Checks message_id non-empty             ✓
        │
        ▼
4. runtime.process(&envelope)
        │
        ▼
5. message_type == "Vote" → process_message()
        │
        ▼
6. Acquire write lock on session registry
        │
        ▼
7. Find session by session_id               ✓ found
        │
        ▼
8. Check seen_message_ids for message_id    ✓ not a duplicate
        │
        ▼
9. Check TTL: now < ttl_expiry              ✓ not expired
        │
        ▼
10. Check state == Open                      ✓
        │
        ▼
11. Check sender in participants             ✓ (or empty list)
        │
        ▼
12. Add message_id to seen_message_ids
        │
        ▼
13. log_store.append(session_id, Incoming entry)
        │
        ▼
14. Look up mode by session.mode → DecisionMode
        │
        ▼
15. DecisionMode::on_message(&session, &envelope)
    - Deserialize mode_state → DecisionState
    - Parse payload as VotePayload
    - Validate: proposals exist, phase != Committed
    - Store vote in state.votes (keyed by sender)
    - Advance phase to Voting
    - Serialize updated state
    - Return PersistState(serialized_state)
        │
        ▼
16. apply_mode_response(&mut session, PersistState(bytes))
    - session.mode_state = bytes
        │
        ▼
17. Return ProcessResult { state: Open, duplicate: false }
        │
        ▼
18. MacpServer builds Ack {
      ok: true,
      duplicate: false,
      message_id: "...",
      session_id: "...",
      accepted_at_unix_ms: <now>,
      session_state: SESSION_STATE_OPEN,
      error: None
    }
        │
        ▼
19. Client receives SendResponse { ack }
```

---

## Data Flow: Session Cancellation

```
1. Client sends CancelSessionRequest { session_id, reason }
        │
        ▼
2. MacpServer::cancel_session() receives the gRPC request
        │
        ▼
3. runtime.cancel_session(session_id, reason)
        │
        ▼
4. Acquire write lock on session registry
        │
        ▼
5. Find session by session_id
   - Not found → return Err(UnknownSession)
   - Found, state == Resolved or Expired → idempotent, return Ok
   - Found, state == Open → continue
        │
        ▼
6. log_store.append(session_id, Internal {
     message_id: "__session_cancel__",
     message_type: "SessionCancel",
     sender: "",
     raw_payload: reason.as_bytes()
   })
        │
        ▼
7. session.state = Expired
        │
        ▼
8. Return Ok(())
        │
        ▼
9. MacpServer builds Ack { ok: true, ... }
```

---

## Data Flow: Initialize Handshake

```
1. Client sends InitializeRequest {
     supported_protocol_versions: ["1.0"],
     client_info: { name: "my-agent", ... },
     capabilities: { ... }
   }
        │
        ▼
2. MacpServer::initialize() receives the request
        │
        ▼
3. Check if "1.0" is in supported_protocol_versions
   - Not found → return INVALID_ARGUMENT gRPC status
   - Found → continue
        │
        ▼
4. Build InitializeResponse {
     selected_protocol_version: "1.0",
     runtime_info: { name: "macp-runtime", version: "0.2.0", ... },
     capabilities: { sessions, cancellation, progress, manifest, ... },
     supported_modes: ["macp.mode.decision.v1", "macp.mode.multi_round.v1"],
     instructions: "MACP Runtime v0.2 ..."
   }
        │
        ▼
5. Client receives the response, caches capabilities and modes
```

---

## Concurrency Model

**Question:** What happens when two clients send messages to the same session simultaneously?

**Answer:** They are serialized safely through the `RwLock`:

1. Both gRPC handlers enter `send()` simultaneously.
2. Both call `validate()` independently — no shared state here.
3. Both call `runtime.process()` which attempts to acquire the write lock.
4. **First request** acquires the write lock.
5. First request processes through the full pipeline (dedup check, TTL check, state check, participant check, logging, mode dispatch, apply response).
6. First request releases the write lock.
7. **Second request** acquires the write lock.
8. Second request processes through the same pipeline, but now sees the state changes from the first request (e.g., if the first request resolved the session, the second will get `SessionNotOpen`).

**Read concurrency:** The `RwLock` allows multiple concurrent readers. `GetSession` calls acquire read locks and can execute simultaneously without blocking each other.

**Write serialization:** All write operations (SessionStart, regular messages, CancelSession) acquire exclusive write locks. This ensures atomicity but means that high write throughput to the same registry will be serialized. In practice, this is acceptable for the coordination use case where message rates are modest.

**No background tasks:** There are no background cleanup threads. Expired sessions remain in memory until the server restarts. This is a deliberate simplification — the coordination use case typically involves a bounded number of sessions, and memory pressure from expired sessions is negligible.

---

## File Structure

```
runtime/
├── proto/
│   ├── buf.yaml                              # Buf linter configuration
│   └── macp/
│       ├── v1/
│       │   ├── envelope.proto                # Envelope, Ack, MACPError, SessionState
│       │   └── core.proto                    # Service + all message types
│       └── modes/
│           └── decision/
│               └── v1/
│                   └── decision.proto        # Decision mode payload types
├── src/
│   ├── main.rs                               # Entry point — server startup
│   ├── lib.rs                                # Library root — proto modules + exports
│   ├── server.rs                             # gRPC adapter (MacpRuntimeService)
│   ├── error.rs                              # MacpError enum + RFC error codes
│   ├── session.rs                            # Session struct, SessionState, TTL parsing
│   ├── registry.rs                           # SessionRegistry (RwLock<HashMap>)
│   ├── log_store.rs                          # Append-only LogStore
│   ├── runtime.rs                            # Runtime kernel
│   ├── mode/
│   │   ├── mod.rs                            # Mode trait + ModeResponse
│   │   ├── decision.rs                       # DecisionMode (RFC lifecycle)
│   │   └── multi_round.rs                    # MultiRoundMode (convergence)
│   └── bin/
│       ├── client.rs                         # Basic demo client
│       ├── fuzz_client.rs                    # Comprehensive error-path client
│       └── multi_round_client.rs             # Multi-round convergence demo
├── build.rs                                  # tonic-build proto compilation
├── Cargo.toml                                # Dependencies
├── Makefile                                  # Development shortcuts
├── .githooks/
│   └── pre-commit                            # Pre-commit hook (fmt + clippy)
├── .github/
│   ├── workflows/
│   │   └── ci.yml                            # CI/CD pipeline
│   ├── ISSUE_TEMPLATE/
│   │   ├── bug_report.yml                    # Bug report template
│   │   └── rfc_proposal.yml                  # RFC proposal template
│   └── PULL_REQUEST_TEMPLATE.md              # PR template
└── docs/
    ├── README.md                             # Getting started guide
    ├── protocol.md                           # Protocol specification
    ├── architecture.md                       # This document
    └── examples.md                           # Usage examples
```

---

## Build Process

1. **`build.rs` runs first** — reads the three `.proto` files from the `proto/` directory and generates Rust code via `tonic-build`. The generated code appears in `target/debug/build/macp-runtime-*/out/`.

2. **Rust compiler compiles:**
   - `src/lib.rs` — the library crate with all modules.
   - `src/main.rs` — the server binary.
   - `src/bin/client.rs` — the basic demo client.
   - `src/bin/fuzz_client.rs` — the comprehensive test client.
   - `src/bin/multi_round_client.rs` — the convergence demo client.

3. **Output binaries:**
   - `target/debug/macp-runtime` — the server.
   - `target/debug/client` — the basic client.
   - `target/debug/fuzz_client` — the test client.
   - `target/debug/multi_round_client` — the convergence demo.

---

## CI/CD Pipeline

The GitHub Actions workflow (`.github/workflows/ci.yml`) runs on every push and pull request:

1. **Checkout** — fetches the code.
2. **Install protoc** — installs the Protocol Buffers compiler.
3. **Install Buf** — installs the Buf CLI for proto linting.
4. **Buf lint** — lints the proto files against the `STANDARD` rules.
5. **Cargo fmt** — checks that all code is formatted.
6. **Cargo clippy** — runs the linter with `-D warnings` (warnings are errors).
7. **Cargo test** — runs the full test suite.
8. **Cargo build** — verifies the project compiles cleanly.

---

## Design Principles

### 1. Separation of Concerns

Each layer has exactly one job:
- **Proto schema** — defines the wire format.
- **MacpServer** — handles gRPC transport concerns.
- **Runtime** — enforces protocol invariants.
- **Modes** — provide coordination logic.
- **Registry/LogStore** — provide storage.

### 2. Pluggable Coordination

The runtime provides "physics" (invariants, TTL, logging, routing, deduplication, participant validation). Modes provide "coordination logic" (when to resolve, what state to track). New modes can be added by implementing the `Mode` trait and registering them in the runtime — no changes to the kernel or transport layer required.

### 3. Fail-Safe Design

- Invalid messages are rejected, never ignored.
- No partial state updates — `apply_mode_response()` is atomic.
- Errors are explicit and structured — every failure has an RFC error code.
- Validation occurs before state mutation — the system never enters an inconsistent state.
- Log-before-mutate ordering — events are logged before the mode processes them.

### 4. Idempotent Operations

- Duplicate messages (same `message_id`) are safely handled as no-ops.
- Duplicate SessionStart with the same `message_id` returns success without re-creating.
- CancelSession on an already-terminal session is idempotent.

### 5. Minimal Coordination

- The runtime does not interpret payload contents (that's the mode's job).
- Sessions are independent — no cross-session coordination.
- Modes receive immutable state and return declarative responses.
- The server is stateless except for the in-memory registry and log store.

### 6. Structural Invariants

The system enforces protocol-level rules that cannot be violated:
- Cannot start a session twice (with a different message_id).
- Cannot send to a non-existent session.
- Cannot send to a resolved or expired session.
- Must use the correct protocol version.
- Must reference a registered mode.
- Must be a listed participant (if participant list is configured).

These are **protocol-level** invariants, not domain-specific business rules.

---

## Next Steps

- Read **[protocol.md](./protocol.md)** for the full protocol specification.
- Read **[examples.md](./examples.md)** for practical usage examples with the new v0.2 RPCs.
