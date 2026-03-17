# macp-runtime v0.3

**Minimal Coordination Runtime (MCR)** — an RFC-0001-compliant gRPC server implementing the Multi-Agent Coordination Protocol (MACP).

The MACP Runtime provides session-based message coordination between autonomous agents. It manages session lifecycles, enforces protocol invariants, routes messages through a pluggable Mode system, and ensures deterministic state transitions — so that agents can focus on coordination logic rather than infrastructure plumbing.

## Features

- **RFC-0001 Compliant Protocol** — Structured protobuf schema with versioned envelope, typed errors, and capability negotiation
- **Initialize Handshake** — Protocol version negotiation and capability discovery before any session work begins
- **Pluggable Mode System** — Coordination logic is decoupled from runtime physics; ship new modes without touching the kernel
- **Decision Mode** — Full Proposal → Evaluation → Objection → Vote → Commitment workflow with declared participant model and mode-aware authorization
- **Proposal Mode** — Lightweight propose/accept/reject lifecycle with peer participant model
- **Task Mode** — Orchestrated task assignment and completion tracking with structural-only determinism
- **Handoff Mode** — Delegated context transfer between agents with context-frozen semantics
- **Quorum Mode** — Threshold-based voting with quorum participant model and semantic-deterministic resolution
- **Multi-Round Convergence Mode (Experimental)** — Participant-based `all_equal` convergence strategy with automatic resolution (not advertised via discovery RPCs)
- **Session Cancellation** — Explicit `CancelSession` RPC to terminate sessions with a recorded reason
- **Message Deduplication** — Idempotent message handling via `seen_message_ids` tracking
- **Mode-Aware Authorization** — Sender authorization delegated to modes; Decision Mode allows orchestrator Commitment bypass per RFC
- **Participant Validation** — Sender membership enforcement when a participant list is configured
- **Signal Messages** — Ambient, session-less messages for out-of-band coordination signals
- **Mode & Manifest Discovery** — `ListModes` and `GetManifest` RPCs for runtime introspection
- **Structured Errors** — `MACPError` with RFC error codes, session/message correlation, and detail payloads
- **Append-Only Audit Log** — Log-before-mutate ordering for every session event
- **CI/CD Pipeline** — GitHub Actions workflow with formatting, linting, and test gates

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable toolchain)
- [Protocol Buffers compiler (`protoc`)](https://grpc.io/docs/protoc-installation/)

## Quick Start

```bash
# Build the project
cargo build

# Run the server (listens on 127.0.0.1:50051)
cargo run

# Run test clients (server must be running in another terminal)
cargo run --bin client                # basic decision mode demo
cargo run --bin fuzz_client           # all error paths + multi-round + new RPCs
cargo run --bin multi_round_client    # multi-round convergence demo
cargo run --bin proposal_client       # proposal mode demo
cargo run --bin task_client           # task mode demo
cargo run --bin handoff_client        # handoff mode demo
cargo run --bin quorum_client         # quorum mode demo
```

## Build & Development Commands

```bash
cargo build          # compile the project
cargo run            # start the runtime server
cargo test           # run the test suite
cargo check          # type-check without building
cargo fmt            # format all code
cargo clippy         # run the linter

# Or use the Makefile:
make setup           # configure git hooks
make build           # cargo build
make test            # cargo test
make fmt             # cargo fmt
make clippy          # cargo clippy with -D warnings
make check           # fmt + clippy + test
```

## Project Structure

```
runtime/
├── proto/
│   ├── buf.yaml                              # Buf linter configuration
│   └── macp/
│       ├── v1/
│       │   ├── envelope.proto                # Envelope, Ack, MACPError, SessionState
│       │   └── core.proto                    # Full service definition + all message types
│       └── modes/
│           ├── decision/
│           │   └── v1/
│           │       └── decision.proto        # Decision mode payload types
│           ├── proposal/
│           │   └── v1/
│           │       └── proposal.proto        # Proposal mode payload types
│           ├── task/
│           │   └── v1/
│           │       └── task.proto            # Task mode payload types
│           ├── handoff/
│           │   └── v1/
│           │       └── handoff.proto         # Handoff mode payload types
│           └── quorum/
│               └── v1/
│                   └── quorum.proto          # Quorum mode payload types
├── src/
│   ├── main.rs                               # Entry point — wires Runtime + gRPC server
│   ├── lib.rs                                # Library root — proto modules + re-exports
│   ├── server.rs                             # gRPC adapter (MacpRuntimeService impl)
│   ├── error.rs                              # MacpError enum + RFC error codes
│   ├── session.rs                            # Session struct, SessionState, TTL parsing
│   ├── registry.rs                           # SessionRegistry (thread-safe session store)
│   ├── log_store.rs                          # Append-only LogStore for audit trails
│   ├── runtime.rs                            # Runtime kernel (dispatch + apply ModeResponse)
│   ├── mode/
│   │   ├── mod.rs                            # Mode trait + ModeResponse enum
│   │   ├── util.rs                           # Shared mode utilities
│   │   ├── decision.rs                       # DecisionMode (RFC lifecycle)
│   │   ├── proposal.rs                       # ProposalMode (peer propose/accept/reject)
│   │   ├── task.rs                           # TaskMode (orchestrated task tracking)
│   │   ├── handoff.rs                        # HandoffMode (delegated context transfer)
│   │   ├── quorum.rs                         # QuorumMode (threshold-based voting)
│   │   └── multi_round.rs                    # MultiRoundMode (convergence)
│   └── bin/
│       ├── client.rs                         # Basic decision mode demo client
│       ├── fuzz_client.rs                    # Comprehensive error-path test client
│       ├── multi_round_client.rs             # Multi-round convergence demo client
│       ├── proposal_client.rs                # Proposal mode demo client
│       ├── task_client.rs                    # Task mode demo client
│       ├── handoff_client.rs                 # Handoff mode demo client
│       └── quorum_client.rs                  # Quorum mode demo client
├── build.rs                                  # tonic-build proto compilation
├── Cargo.toml                                # Dependencies and project config
├── Makefile                                  # Development shortcuts
└── .github/
    └── workflows/
        └── ci.yml                            # CI/CD pipeline
```

## gRPC Service

The runtime exposes `MACPRuntimeService` on `127.0.0.1:50051` with the following RPCs:

| RPC | Description |
|-----|-------------|
| `Initialize` | Protocol version negotiation and capability exchange |
| `Send` | Send an Envelope, receive an Ack |
| `StreamSession` | Bidirectional streaming for session events (not yet fully implemented) |
| `GetSession` | Query session metadata by ID |
| `CancelSession` | Cancel an active session with a reason |
| `GetManifest` | Retrieve agent manifest and supported modes |
| `ListModes` | Discover registered mode descriptors |
| `ListRoots` | List resource roots |
| `WatchModeRegistry` | Stream mode registry change notifications |
| `WatchRoots` | Stream root change notifications |

## Documentation

- **[docs/README.md](./docs/README.md)** — Getting started guide and key concepts
- **[docs/protocol.md](./docs/protocol.md)** — Full MACP v1.0 protocol specification
- **[docs/architecture.md](./docs/architecture.md)** — Internal architecture and design principles
- **[docs/examples.md](./docs/examples.md)** — Step-by-step usage examples and common patterns

## License

See the repository root for license information.
