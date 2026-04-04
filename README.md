# macp-runtime v0.4.0

Reference runtime for the Multi-Agent Coordination Protocol (MACP).

This runtime implements the current MACP core/service surface, five standards-track modes, and one built-in extension mode. The focus of this release is freeze-readiness for SDKs and real-world unary and streaming integrations: strict `SessionStart`, mode-semantic correctness, authenticated senders, bounded resources, durable restart recovery, and extension mode lifecycle management.

## What changed in v0.4.0

- **Strict canonical `SessionStart` for standard modes**
  - no empty payloads
  - no implicit default mode
  - explicit `mode_version`, `configuration_version`, and positive `ttl_ms`
  - explicit unique participants for standards-track modes
- **Decision Mode authority clarified**
  - initiator/coordinator may emit `Proposal` and `Commitment`
  - participants emit `Evaluation`, `Objection`, and `Vote`
  - duplicate `proposal_id` values are rejected
  - votes are tracked per proposal, per sender
- **Proposal Mode commitment gating fixed**
  - `Commitment` is accepted only after acceptance convergence or a terminal rejection
- **Security boundary added**
  - TLS-capable startup
  - authenticated sender derivation via bearer token or dev header mode
  - per-request authorization
  - payload size limits
  - rate limiting
- **Durable local persistence**
  - per-session append-only log files and session snapshots via `FileBackend`
  - crash recovery with dedup state reconciliation
  - atomic writes (tmp file + rename) prevent partial-write corruption
- **Authoritative accepted history**
  - log append failures are now fatal — messages are not acknowledged without a durable record
  - session state is rebuilt from append-only logs on startup via replay (no snapshot dependency)
  - `LogEntry` enriched with `session_id`, `mode`, `macp_version` for self-describing replay
- **Session ID security policy**
  - session IDs must be UUID v4/v7 (hyphenated lowercase) or base64url tokens (22+ chars)
  - weak/human-readable IDs are rejected with `INVALID_SESSION_ID`
- **Signal enforcement**
  - Signals are strictly ambient — non-empty `session_id` or `mode` is rejected
- **StreamSession enabled**
  - `Initialize` advertises `stream: true`
  - `StreamSession` provides per-session bidirectional streaming of accepted envelopes
  - `WatchModeRegistry` fires live `RegistryChanged` events on mode register/unregister/promote
  - `WatchRoots` implemented (basic: send initial state, hold stream open)
- **Extension mode lifecycle**
  - `multi_round` demoted from standards-track to built-in extension (`ext.multi_round.v1`)
  - `ListExtModes` returns extension mode descriptors
  - `RegisterExtMode` dynamically registers new extension modes with a passthrough handler
  - `UnregisterExtMode` removes dynamically registered extensions (built-in modes protected)
  - `PromoteMode` promotes extensions to standards-track with optional identifier rename
- **Structured logging via `tracing`**
  - use `RUST_LOG` env var to control log level (e.g. `RUST_LOG=info`)
- **Per-mode metrics**
  - tracked via `src/metrics.rs`

## Implemented modes

Standards-track modes:

- `macp.mode.decision.v1`
- `macp.mode.proposal.v1`
- `macp.mode.task.v1`
- `macp.mode.handoff.v1`
- `macp.mode.quorum.v1`

Built-in extension modes:

- `ext.multi_round.v1`

## Runtime behavior that SDKs should assume

### Session bootstrap

For all standards-track modes and built-in extensions, `SessionStartPayload` must include:

- `participants`
- `mode_version`
- `configuration_version`
- `ttl_ms`

`policy_version` is optional unless your policy requires it. Empty `mode` is rejected. Empty `SessionStartPayload` is rejected.

### Security

In production, requests should be authenticated with a bearer token. The runtime derives `Envelope.sender` from the authenticated identity and rejects spoofed sender values.

For local development, you may opt into insecure/dev mode with:

```bash
MACP_ALLOW_INSECURE=1
MACP_ALLOW_DEV_SENDER_HEADER=1
```

When dev header mode is enabled, clients can set `x-macp-agent-id` metadata instead of bearer tokens.

### Persistence

Unless `MACP_MEMORY_ONLY=1` is set, the runtime persists session and log snapshots under `MACP_DATA_DIR` (default: `.macp-data`). If a persistence file contains corrupt or incompatible JSON on startup, the runtime logs a warning to stderr and starts with empty state rather than failing.

## Configuration

### Core server configuration

| Variable | Meaning | Default |
|---|---|---|
| `MACP_BIND_ADDR` | bind address | `127.0.0.1:50051` |
| `MACP_DATA_DIR` | persistence directory | `.macp-data` |
| `MACP_MEMORY_ONLY` | disable persistence when set to `1` | unset |
| `RUST_LOG` | `tracing` log level filter (e.g. `info`, `debug`) | unset |
| `MACP_ALLOW_INSECURE` | allow plaintext transport when set to `1` | unset |
| `MACP_TLS_CERT_PATH` | PEM certificate for TLS | unset |
| `MACP_TLS_KEY_PATH` | PEM private key for TLS | unset |

### Authentication and authorization

| Variable | Meaning | Default |
|---|---|---|
| `MACP_AUTH_TOKENS_JSON` | inline auth config JSON | unset |
| `MACP_AUTH_TOKENS_FILE` | path to auth config JSON | unset |
| `MACP_ALLOW_DEV_SENDER_HEADER` | allow `x-macp-agent-id` for local dev | unset |

Token JSON may be either a raw list or an object with a `tokens` array. Example:

```json
{
  "tokens": [
    {
      "token": "demo-coordinator-token",
      "sender": "coordinator",
      "allowed_modes": [
        "macp.mode.decision.v1",
        "macp.mode.quorum.v1"
      ],
      "can_start_sessions": true,
      "max_open_sessions": 25
    },
    {
      "token": "demo-worker-token",
      "sender": "worker",
      "allowed_modes": [
        "macp.mode.task.v1"
      ],
      "can_start_sessions": false,
      "can_manage_mode_registry": false
    }
  ]
}
```

### Resource limits

| Variable | Meaning | Default |
|---|---|---|
| `MACP_MAX_PAYLOAD_BYTES` | max envelope payload size | `1048576` |
| `MACP_SESSION_START_LIMIT_PER_MINUTE` | per-sender session start limit | `60` |
| `MACP_MESSAGE_LIMIT_PER_MINUTE` | per-sender message limit | `600` |

## Quick start

### Production-style startup with TLS

```bash
export MACP_TLS_CERT_PATH=/path/to/server.crt
export MACP_TLS_KEY_PATH=/path/to/server.key
export MACP_AUTH_TOKENS_FILE=/path/to/tokens.json
cargo run
```

### Local development startup

```bash
export MACP_ALLOW_INSECURE=1
export MACP_ALLOW_DEV_SENDER_HEADER=1
cargo run
```

### Running the example clients

The example clients in `src/bin` assume the local development startup shown above.

```bash
cargo run --bin client
cargo run --bin proposal_client
cargo run --bin task_client
cargo run --bin handoff_client
cargo run --bin quorum_client
cargo run --bin multi_round_client
cargo run --bin fuzz_client
```

## Freeze-profile capability summary

| RPC | Status |
|---|---|
| `Initialize` | implemented |
| `Send` | implemented |
| `GetSession` | implemented |
| `CancelSession` | implemented |
| `GetManifest` | implemented |
| `ListModes` | implemented |
| `ListRoots` | implemented |
| `StreamSession` | implemented |
| `WatchModeRegistry` | implemented |
| `WatchRoots` | implemented |
| `ListExtModes` | implemented |
| `RegisterExtMode` | implemented |
| `UnregisterExtMode` | implemented |
| `PromoteMode` | implemented |

## Architecture

```
Client Request
       |
  [Transport/gRPC] -- server.rs, security.rs
       |
  [Coordination Kernel] -- runtime.rs
       |
  [Mode Registry] -- mode_registry.rs
       |            \
  [Mode Logic]     [Discovery + Extension Lifecycle]
   mode/*.rs       ListModes, ListExtModes, GetManifest,
                   RegisterExtMode, UnregisterExtMode, PromoteMode
       |
  [Storage Layer] -- storage.rs, log_store.rs
       |
  [Replay] -- replay.rs
```

See `docs/architecture.md` for detailed layer descriptions.

## Project structure

```text
runtime/
├── proto/                  # protobuf schemas copied from the RFC/spec repository
├── src/
│   ├── main.rs             # server startup, TLS, persistence, auth wiring
│   ├── server.rs           # gRPC adapter and request authentication
│   ├── runtime.rs          # coordination kernel and mode dispatch
│   ├── mode_registry.rs    # single source of truth for mode registration
│   ├── security.rs         # auth config, sender derivation, rate limiting
│   ├── session.rs          # canonical SessionStart validation and session model
│   ├── registry.rs         # session store with optional persistence
│   ├── log_store.rs        # in-memory accepted-history log cache
│   ├── storage.rs          # storage backend trait, FileBackend, crash recovery
│   ├── replay.rs           # session rebuild from append-only log
│   ├── metrics.rs          # per-mode metrics counters
│   ├── mode/               # mode implementations (standards-track + extensions)
│   │   ├── passthrough.rs  # generic handler for dynamically registered extensions
│   │   └── ...
│   └── bin/                # local development example clients
├── tests/
│   ├── integration_mode_lifecycle.rs  # full-stack integration tests
│   ├── replay_round_trip.rs           # replay tests for all modes
│   ├── conformance_loader.rs          # JSON fixture runner
│   └── conformance/                   # per-mode conformance fixtures
├── docs/
└── build.rs
```

## Troubleshooting

**TLS required error on startup**
Set `MACP_ALLOW_INSECURE=1` for local development, or provide `MACP_TLS_CERT_PATH` and `MACP_TLS_KEY_PATH` for production.

**`InvalidSessionId` error**
Session IDs must be UUID v4/v7 in hyphenated lowercase form (36 chars) or base64url tokens (22+ chars). Short or human-readable IDs like `"s1"` or `"my-session"` are rejected.

**`InvalidPayload` on `SessionStart`**
For standards-track modes and built-in extensions (including `ext.multi_round.v1`), `SessionStartPayload` must include non-empty `participants`, `mode_version`, `configuration_version`, and a positive `ttl_ms`. Empty payloads are rejected.

**`Forbidden` error**
Check that the sender identity matches the session's participant list. For `Commitment` messages, only the session initiator is authorized. Verify your bearer token maps to the correct sender.

**`StorageFailed` error**
The runtime requires write access to `MACP_DATA_DIR`. Check directory permissions. Log append failures are fatal — the runtime will not acknowledge a message without a durable record.

**Proto drift / `make check-protos` failure**
Run `make sync-protos` to update local proto files from BSR.

## Testing

```bash
cargo test --all-targets          # Unit tests + Rust integration tests
make test-conformance             # JSON fixture-driven conformance suite
```

A separate integration test crate (`integration_tests/`) tests the runtime through the real gRPC boundary:

```bash
cargo build
cd integration_tests
MACP_TEST_BINARY=../target/debug/macp-runtime cargo test -- --test-threads=1
```

The integration suite has three tiers:

- **Tier 1 (Protocol)** — 47 scripted gRPC tests covering all modes, error paths, signals, version binding, dedup, and RFC cross-cutting features
- **Tier 2 (Rig Tools)** — 5 tests using [Rig](https://rig.rs) agent framework `Tool` implementations for all MACP operations
- **Tier 3 (E2E)** — 3 tests with real OpenAI GPT-4o-mini agents coordinating through the runtime (requires `OPENAI_API_KEY`)

See `docs/testing.md` for full details on running locally, in CI, or against a hosted runtime.

## Development notes

- The RFC/spec repository remains the normative source for protocol semantics.
- Five standards-track modes use the canonical `macp.mode.*` identifiers.
- `multi_round` is a built-in extension (`ext.multi_round.v1`) — not standards-track, but ships with the runtime and enforces strict `SessionStart`.
- Extension modes can be dynamically registered, unregistered, and promoted via `RegisterExtMode`, `UnregisterExtMode`, and `PromoteMode` RPCs.
- `StreamSession` is enabled and binds one gRPC stream to one session, emitting accepted envelopes in order.
- `WatchSignals` broadcasts ambient Signal envelopes to all subscribers in real time.

See `docs/README.md` and `docs/examples.md` for the updated local development and usage guidance.
