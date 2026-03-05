# Architecture

This document explains how the MACP Runtime is built internally. We'll explain each component in plain language, without assuming Rust knowledge.

## System Overview

```
┌─────────────────────────────────────────────────┐
│                  Clients                        │
│  (Other programs/agents wanting to coordinate)  │
└────────────┬────────────────────┬────────────────┘
             │                    │
             │ gRPC calls         │ gRPC calls
             │                    │
             ▼                    ▼
┌─────────────────────────────────────────────────┐
│            MACP Runtime Server                  │
│  ┌───────────────────────────────────────────┐  │
│  │     MacpServer (gRPC Adapter Layer)       │  │
│  │  - Receives messages                      │  │
│  │  - Validates transport-level fields       │  │
│  │  - Delegates to Runtime kernel            │  │
│  └──────────────┬────────────────────────────┘  │
│                 │                                │
│                 ▼                                │
│  ┌───────────────────────────────────────────┐  │
│  │         Runtime (Kernel)                  │  │
│  │  - Resolves mode                          │  │
│  │  - Enforces TTL / session invariants      │  │
│  │  - Dispatches to Mode implementations     │  │
│  │  - Applies ModeResponse                   │  │
│  └──────┬───────────────┬────────────────────┘  │
│         │               │                        │
│         ▼               ▼                        │
│  ┌─────────────┐ ┌─────────────────────┐        │
│  │ Mode        │ │ Mode                │        │
│  │ Dispatcher  │ │ Implementations     │        │
│  │             │ │ - DecisionMode      │        │
│  │             │ │ - MultiRoundMode    │        │
│  └─────────────┘ └─────────────────────┘        │
│         │                                        │
│         ▼                                        │
│  ┌───────────────────────────────────────────┐  │
│  │  SessionRegistry      LogStore            │  │
│  │  (Session State)      (Event Log)         │  │
│  │  HashMap: id->Session  HashMap: id->Vec   │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

## Core Components

### 1. Protocol Definitions (proto/macp.proto)

This file defines the "language" that clients and server use to communicate. It's like a contract that both sides agree to follow.

**What's defined:**
- **Envelope**: The wrapper for every message
  - Contains metadata (who, when, which session, which mode)
  - Contains the actual payload (the message content)
- **Ack**: The response the server sends back
  - `accepted`: true if the message was accepted, false if rejected
  - `error`: explanation if rejected
- **SessionQuery / SessionInfo**: For querying session state
- **MACPService**: The service interface
  - `SendMessage`: Send an Envelope, get an Ack
  - `GetSession`: Query session state by ID

**Why Protocol Buffers?**
Instead of JSON (text-based), Protocol Buffers use a binary format that's:
- Faster to send/receive
- Smaller in size
- Type-safe (can't accidentally send wrong data types)

### 2. Generated Code (build.rs + target/debug/build/)

The `build.rs` script runs before compilation and automatically generates Rust code from the `.proto` file. This generated code handles all the low-level serialization/deserialization.

**What you need to know:**
- You never edit the generated code
- Changes to `.proto` automatically update the generated code
- The generated code appears as the `pb` module (protocol buffers)

### 3. Main Server (src/main.rs)

This is the entry point — where the program starts.

**What it does:**
1. Creates a `SessionRegistry` (session state storage)
2. Creates a `LogStore` (event log storage)
3. Creates a `Runtime` (coordination kernel with registered modes)
4. Creates a `MacpServer` (gRPC adapter wrapping the runtime)
5. Starts a gRPC server on `127.0.0.1:50051`
6. Waits for incoming connections

### 4. Error Types (src/error.rs)

The system defines specific errors for each problem:

```rust
pub enum MacpError {
    InvalidMacpVersion,    // Version != "v1"
    InvalidEnvelope,       // Missing required fields or invalid TTL payload
    DuplicateSession,      // SessionStart for existing session
    UnknownSession,        // Message for non-existent session
    SessionNotOpen,        // Message sent to resolved/expired session
    TtlExpired,            // Session TTL has elapsed
    InvalidTtl,            // TTL value out of range (<=0 or >24h)
    UnknownMode,           // Mode not registered in runtime
    InvalidModeState,      // Mode state bytes can't be deserialized
    InvalidPayload,        // Payload doesn't match mode's expected format
}
```

**Error conversion:**
Errors are never "panics" (crashes). They're converted to Ack responses:
```rust
Ack {
    accepted: false,
    error: "SessionNotOpen"
}
```

### 5. Session Types (src/session.rs)

Each **Session** stores information about one coordination:

```rust
pub struct Session {
    pub session_id: String,           // e.g., "s1"
    pub state: SessionState,          // Open, Resolved, or Expired
    pub ttl_expiry: i64,              // Unix timestamp (milliseconds)
    pub resolution: Option<Vec<u8>>,  // Final result (if resolved)
    pub mode: String,                 // Mode name (e.g., "decision", "multi_round")
    pub mode_state: Vec<u8>,          // Mode-specific state (opaque bytes)
    pub participants: Vec<String>,    // Participant list (for multi_round)
}
```

**Session States:**
- **Open**: Session is active, can receive messages
- **Resolved**: Decision made (via mode), no more messages allowed
- **Expired**: TTL has elapsed, enforced on next message receipt

### 6. Session Registry (src/registry.rs)

The **SessionRegistry** is like a database in memory. It stores all active sessions.

**Data structure:**
```
SessionRegistry {
    sessions: HashMap<String, Session>
}
```

**Thread-safety:**
Multiple clients can connect at the same time. The `RwLock` ensures:
- Multiple readers can read simultaneously (efficient)
- Only one writer at a time (prevents corruption)
- Readers wait while someone is writing

### 7. Log Store (src/log_store.rs)

The **LogStore** maintains an append-only event log per session:

```rust
pub struct LogEntry {
    pub message_id: String,
    pub received_at_ms: i64,
    pub sender: String,
    pub message_type: String,
    pub raw_payload: Vec<u8>,
    pub entry_kind: EntryKind,  // Incoming or Internal
}
```

- **Incoming** entries: messages received from clients
- **Internal** entries: runtime-generated events (e.g., TtlExpired)
- Entries are always appended before state mutation (log-before-mutate ordering)

### 8. Mode System (src/mode/)

The **Mode** trait defines the interface for coordination logic:

```rust
pub trait Mode: Send + Sync {
    fn on_session_start(&self, session: &Session, env: &Envelope)
        -> Result<ModeResponse, MacpError>;
    fn on_message(&self, session: &Session, env: &Envelope)
        -> Result<ModeResponse, MacpError>;
}
```

Modes receive **immutable** session state and return a `ModeResponse`:
- `NoOp` — no state change
- `PersistState(bytes)` — update mode-specific state
- `Resolve(bytes)` — resolve the session
- `PersistAndResolve{state, resolution}` — both at once

**DecisionMode** (`src/mode/decision.rs`):
- `on_session_start()` → `NoOp`
- `on_message()` → if `payload == b"resolve"` then `Resolve` else `NoOp`

**MultiRoundMode** (`src/mode/multi_round.rs`):
- Tracks participants, contributions per round, and convergence
- `on_session_start()` → parses config, returns `PersistState` with initial state
- `on_message()` → updates contributions, checks convergence, returns `PersistState` or `PersistAndResolve`

### 9. Runtime Kernel (src/runtime.rs)

The **Runtime** is the coordination kernel that orchestrates everything:

```rust
pub struct Runtime {
    pub registry: Arc<SessionRegistry>,
    pub log_store: Arc<LogStore>,
    modes: HashMap<String, Box<dyn Mode>>,
}
```

**Processing flow:**
```
1. Receive Envelope (from MacpServer)
2. For SessionStart:
   a. Resolve mode name (empty → "decision")
   b. Look up mode implementation → error if unknown
   c. Parse TTL from payload
   d. Acquire write lock, check for duplicate session
   e. Create session log, append Incoming entry
   f. Call mode.on_session_start()
   g. Insert session, apply ModeResponse
3. For other messages:
   a. Acquire write lock, find session
   b. TTL check → if expired, log Internal entry, set Expired
   c. State check → if not Open, reject
   d. Append Incoming log entry
   e. Call mode.on_message()
   f. Apply ModeResponse
4. Return Ok/Err to MacpServer
```

**`apply_mode_response()`** is the single mutation point:
- `NoOp` → nothing
- `PersistState(s)` → `session.mode_state = s`
- `Resolve(r)` → `session.state = Resolved, session.resolution = Some(r)`
- `PersistAndResolve{s,r}` → both

### 10. MacpServer (src/server.rs)

The **MacpServer** is now a thin gRPC adapter:

**Responsibilities:**
1. Validate transport-level fields (version, required fields)
2. Delegate to `Runtime::process()`
3. Convert results to `Ack` responses
4. Handle `GetSession` queries

All coordination logic lives in the Runtime and Mode implementations.

## Data Flow Example

Let's trace what happens when a client sends a multi-round convergence message:

### Step 1: Client sends SessionStart
```
Client → gRPC → MacpServer::send_message()
         → validate()
         → Runtime::process()
         → resolve mode = "multi_round"
         → parse TTL
         → create session log
         → MultiRoundMode::on_session_start()
         → PersistState(initial_state)
         → insert session with mode_state
         ← Ack(accepted=true)
```

### Step 2: Client sends Contribute
```
Client → gRPC → MacpServer::send_message()
         → validate()
         → Runtime::process()
         → find session, check TTL, check Open
         → append Incoming log entry
         → MultiRoundMode::on_message()
         → update contributions, check convergence
         → PersistState(updated_state) or PersistAndResolve
         → apply response
         ← Ack(accepted=true)
```

### Step 3: Client queries state
```
Client → gRPC → MacpServer::get_session()
         → registry.get_session(id)
         ← SessionInfo(state, mode, resolution, ...)
```

## Concurrency Model

**Question:** What happens if two clients send messages at the same time?

**Answer:** They're handled safely:
1. Both enter `send_message()` simultaneously
2. Both validate independently (no conflicts here)
3. First one acquires the write lock
4. First one processes through the mode, modifies the registry
5. First one releases the lock
6. Second one acquires the write lock
7. Second one processes through the mode, modifies the registry
8. Second one releases the lock

The `RwLock` ensures they don't interfere with each other.

## File Structure

```
runtime/
├── proto/
│   └── macp.proto              # Protocol definition (Envelope, Ack, SessionQuery, SessionInfo)
├── src/
│   ├── main.rs                 # Entry point, wires up Runtime + gRPC server
│   ├── lib.rs                  # Shared library (pb module + public module exports)
│   ├── server.rs               # Thin gRPC adapter delegating to Runtime
│   ├── error.rs                # MacpError enum (all error variants)
│   ├── session.rs              # Session, SessionState, TTL parsing
│   ├── registry.rs             # SessionRegistry (thread-safe session store)
│   ├── log_store.rs            # Append-only LogStore for session event logs
│   ├── runtime.rs              # Runtime kernel (dispatch + apply ModeResponse)
│   ├── mode/
│   │   ├── mod.rs              # Mode trait + ModeResponse enum
│   │   ├── decision.rs         # DecisionMode (payload=="resolve" → Resolve)
│   │   └── multi_round.rs      # MultiRoundMode (convergence-based resolution)
│   └── bin/
│       ├── client.rs           # Test client (happy path)
│       ├── fuzz_client.rs      # Test client (error paths + multi-round)
│       └── multi_round_client.rs  # Multi-round convergence demo
├── build.rs                    # Generates code from .proto
├── Cargo.toml                  # Dependencies and project config
└── target/                     # Build output (binaries, generated code)
```

## Build Process

1. `build.rs` runs first
   - Reads `proto/macp.proto`
   - Generates Rust code in `target/debug/build/*/out/macp.v1.rs`

2. Rust compiler compiles:
   - `src/lib.rs` (library with all modules)
   - `src/main.rs` (server binary)
   - `src/bin/client.rs` (client binary)
   - `src/bin/fuzz_client.rs` (fuzz binary)
   - `src/bin/multi_round_client.rs` (multi-round demo binary)

3. Output binaries:
   - `target/debug/macp-runtime` (server)
   - `target/debug/client`
   - `target/debug/fuzz_client`
   - `target/debug/multi_round_client`

## Design Principles

### 1. Separation of Concerns
- **Protocol definition** (`.proto`) separate from implementation
- **State management** (`SessionRegistry`) separate from coordination logic
- **Mode logic** separate from runtime kernel
- **Validation** happens before state mutation
- **Logging** happens before mode dispatch

### 2. Pluggable Coordination
- Runtime provides "physics" (invariants, TTL, logging, routing)
- Modes provide "coordination logic" (when to resolve, what state to track)
- New modes can be added without modifying the runtime kernel

### 3. Fail-Safe
- Invalid messages are rejected, not ignored
- No partial state updates (atomic operations via single mutation point)
- Errors are explicit, not silent

### 4. Minimal Coordination
- Server doesn't interpret payloads (except through Mode implementations)
- Sessions are independent (no cross-session coordination)
- Modes receive immutable state and return responses

### 5. Structural Invariants
The system enforces structural rules:
- Can't start a session twice
- Can't send to non-existent session
- Can't send to resolved/expired session
- Must use correct version
- Must reference registered mode

These are **protocol-level** invariants, not domain-specific business rules.

## Next Steps

- Read [protocol.md](./protocol.md) for the full protocol specification
- Read [examples.md](./examples.md) for practical usage examples
