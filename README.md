# macp-runtime v0.4.0

Reference runtime for the Multi-Agent Coordination Protocol (MACP).

This runtime implements the current MACP core/service surface, the five standards-track modes in the main RFC repository, and one experimental `multi_round` mode that remains available only by explicit canonical name. The focus of this release is freeze-readiness for SDKs and real-world unary integrations: strict `SessionStart`, mode-semantic correctness, authenticated senders, bounded resources, and durable restart recovery.

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
- **Unary freeze profile**
  - `StreamSession` is intentionally disabled in this profile
  - `WatchModeRegistry` and `WatchRoots` remain unimplemented

## Implemented modes

Standards-track modes:

- `macp.mode.decision.v1`
- `macp.mode.proposal.v1`
- `macp.mode.task.v1`
- `macp.mode.handoff.v1`
- `macp.mode.quorum.v1`

Experimental mode:

- `macp.mode.multi_round.v1`

## Runtime behavior that SDKs should assume

### Session bootstrap

For the five standards-track modes, `SessionStartPayload` must include:

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
      "can_start_sessions": false
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
| `StreamSession` | intentionally disabled in freeze profile |
| `WatchModeRegistry` | unimplemented |
| `WatchRoots` | unimplemented |

## Project structure

```text
runtime/
├── proto/                # protobuf schemas copied from the RFC/spec repository
├── src/
│   ├── main.rs           # server startup, TLS, persistence, auth wiring
│   ├── server.rs         # gRPC adapter and request authentication
│   ├── runtime.rs        # coordination kernel and mode dispatch
│   ├── security.rs       # auth config, sender derivation, rate limiting
│   ├── session.rs        # canonical SessionStart validation and session model
│   ├── registry.rs       # session store with optional persistence
│   ├── log_store.rs      # in-memory accepted-history log cache
│   ├── storage.rs        # storage backend trait, FileBackend persistence, crash recovery
│   ├── mode/             # mode implementations
│   └── bin/              # local development example clients
├── docs/
└── build.rs
```

## Development notes

- The RFC/spec repository remains the normative source for protocol semantics.
- This runtime only accepts the canonical standards-track mode identifiers for the five main modes.
- `multi_round` remains experimental and is not advertised by discovery RPCs.
- `StreamSession` is intentionally not part of the freeze surface for the first SDKs.

See `docs/README.md` and `docs/examples.md` for the updated local development and usage guidance.
