# Runtime architecture

`macp-runtime v0.4.0` is organized as a small set of explicit layers.

## 1. Transport adapter (`src/server.rs`)

Responsibilities:

- receive gRPC requests
- authenticate request metadata
- derive the runtime sender identity
- enforce payload limits and rate limits
- translate runtime errors into gRPC responses and MACP `Ack` values

## 2. Coordination kernel (`src/runtime.rs`)

Responsibilities:

- route envelopes by message type
- validate and create sessions
- apply mode authorization and mode transitions
- enforce accepted-history ordering
- enforce lazy TTL expiry on reads and writes
- persist updated session snapshots

## 3. Mode Registry (`src/mode_registry.rs`)

The `ModeRegistry` is the single source of truth for mode dispatch, replay, and discovery. It eliminates the previous pattern of hardcoded mode maps in `runtime.rs`, `main.rs`, and `replay.rs`.

Responsibilities:

- register all mode implementations (standards-track and experimental)
- provide mode lookup for dispatch and replay
- provide standards-track mode names for `ListModes`
- provide mode descriptors for `ListModes` and `GetManifest`
- classify modes as standards-track or experimental

Key methods:

- `build_default()` — constructs the canonical mode set
- `get_mode(name)` — mode lookup for dispatch
- `standard_mode_names()` — drives `ListModes` and `GetManifest`
- `standard_mode_descriptors()` — drives `ListModes` response

## 4. Mode layer (`src/mode/*`)

Responsibilities:

- encode coordination semantics per mode
- validate mode-specific payloads
- authorize mode-specific message types
- return declarative `ModeResponse` values for the kernel to apply

Implemented modes:

- Decision — enforced phase transitions (Proposal -> Evaluation -> Voting -> Committed)
- Proposal — negotiation with counterproposals, acceptance convergence, terminal rejections
- Task — delegated task with serial assignment, progress tracking, terminal reports
- Handoff — serial handoff offers with accept/decline disposition
- Quorum — threshold approval with ballots
- MultiRound (experimental) — iterative value convergence

## 5. Storage layer

### Storage backend (`src/storage.rs`)

Provides the `StorageBackend` trait with two implementations:

- `FileBackend` — per-session directories containing `session.json` and append-only `log.jsonl`, with crash recovery and atomic writes
- `MemoryBackend` — no-op backend for `MACP_MEMORY_ONLY=1`

### Session registry (`src/registry.rs`)

In-memory cache of all sessions, loaded from `FileBackend` on startup. Stores:

- session metadata
- bound versions
- participants
- dedup state
- current session state

Supports optional file-backed snapshot persistence for backward compatibility.

### Log store (`src/log_store.rs`)

In-memory cache of accepted-history logs. Stores:

- accepted incoming envelopes
- runtime-generated internal events such as TTL expiry and session cancellation

On-disk persistence is handled by `FileBackend`, not by LogStore.

## 6. Security layer (`src/security.rs`)

Responsibilities:

- load token-to-identity mappings
- derive sender identities from metadata
- enforce allowed-mode policy
- enforce session-start policy
- enforce per-sender rate limits

## Architecture diagram

```
Client Request
       |
  [Transport/gRPC] -- server.rs, security.rs
       |
  [Coordination Kernel] -- runtime.rs
       |
  [Mode Registry] -- mode_registry.rs
       |            \
  [Mode Logic]     [Discovery]
   mode/*.rs       ListModes, GetManifest
       |
  [Storage Layer] -- storage.rs, log_store.rs
       |
  [Replay] -- replay.rs
```

## Request path summary

1. gRPC request arrives in `MacpServer`
2. request metadata is authenticated
3. sender identity is derived and envelope spoofing is rejected
4. runtime processes the envelope
5. accepted messages mutate state and log history
6. updated session snapshots are persisted
7. an `Ack` is returned to the caller

## Freeze-profile design choice

The runtime now exposes `StreamSession` as a per-session accepted-envelope stream. Each gRPC stream binds to one session and receives canonical MACP envelopes in runtime acceptance order. Unary `Send` remains the path for explicit per-message acknowledgement semantics.
