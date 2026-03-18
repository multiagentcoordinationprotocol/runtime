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

## 3. Mode layer (`src/mode/*`)

Responsibilities:

- encode coordination semantics per mode
- validate mode-specific payloads
- authorize mode-specific message types
- return declarative `ModeResponse` values for the kernel to apply

Implemented modes:

- Decision
- Proposal
- Task
- Handoff
- Quorum
- MultiRound (experimental)

## 4. Storage layer

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

## 5. Security layer (`src/security.rs`)

Responsibilities:

- load token-to-identity mappings
- derive sender identities from metadata
- enforce allowed-mode policy
- enforce session-start policy
- enforce per-sender rate limits

## Request path summary

1. gRPC request arrives in `MacpServer`
2. request metadata is authenticated
3. sender identity is derived and envelope spoofing is rejected
4. runtime processes the envelope
5. accepted messages mutate state and log history
6. updated session snapshots are persisted
7. an `Ack` is returned to the caller

## Freeze-profile design choice

The runtime intentionally prioritizes unary correctness over streaming completeness. `StreamSession` is therefore disabled in this release profile so SDKs can target a stable, explicit surface.
