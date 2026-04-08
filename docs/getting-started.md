# Getting Started

This guide walks you from a fresh checkout to your first coordination session. By the end, you will have the runtime running locally and will have completed a full Decision Mode session through the gRPC API.

For protocol concepts like sessions, modes, and the two-plane model, see the [protocol documentation](https://www.multiagentcoordinationprotocol.io/docs).

## Prerequisites

You need a Rust stable toolchain (1.75 or later) and the Protocol Buffers compiler.

```bash
# macOS
brew install protobuf

# Ubuntu / Debian
sudo apt-get install -y protobuf-compiler

# Verify
protoc --version
rustc --version
```

## Build and run

Clone the repository and build:

```bash
git clone https://github.com/multiagentcoordinationprotocol/runtime.git
cd runtime
cargo build
```

### Starting a development server

For local development, two environment variables disable TLS and enable a header-based identity shortcut. This lets you send requests without configuring tokens or certificates:

```bash
export MACP_ALLOW_INSECURE=1
export MACP_ALLOW_DEV_SENDER_HEADER=1
cargo run
```

The server listens on `127.0.0.1:50051` and trusts the `x-macp-agent-id` gRPC metadata header as the authenticated sender identity.

### Starting a production server

In production, the runtime requires TLS and token-based authentication:

```bash
export MACP_TLS_CERT_PATH=/path/to/server.crt
export MACP_TLS_KEY_PATH=/path/to/server.key
export MACP_AUTH_TOKENS_FILE=/path/to/tokens.json
cargo run
```

See the [Deployment Guide](deployment.md) for the full environment variable reference.

## Your first session

A coordination session has four steps: negotiate the protocol version, create a session, exchange mode-specific messages, and bind the terminal outcome with a commitment.

### Step 1: Initialize

The client sends its supported protocol versions and the runtime selects one. This also exchanges capability information so the client knows which features are available.

```
-> InitializeRequest {
     supported_protocol_versions: ["1.0"],
     client_info: { name: "my-agent", version: "0.1.0" }
   }

<- InitializeResponse {
     selected_protocol_version: "1.0",
     runtime_info: { name: "macp-runtime", version: "0.4.0" },
     supported_modes: [
       "macp.mode.decision.v1",
       "macp.mode.proposal.v1",
       "macp.mode.task.v1",
       "macp.mode.handoff.v1",
       "macp.mode.quorum.v1",
       "ext.multi_round.v1"
     ]
   }
```

### Step 2: Create a session

Send a `SessionStart` envelope to create a Decision Mode session with two participants. Every standards-track session requires four fields in the payload: `participants`, `mode_version`, `configuration_version`, and a positive `ttl_ms`.

```
-> Send(Envelope {
     macp_version: "1.0",
     mode: "macp.mode.decision.v1",
     message_type: "SessionStart",
     message_id: "msg-001",
     session_id: "550e8400-e29b-41d4-a716-446655440000",
     sender: "",
     timestamp_unix_ms: 1712500000000,
     payload: SessionStartPayload {
       intent: "Decide whether to deploy v2.0",
       participants: ["agent://analyst", "agent://reviewer"],
       mode_version: "1.0.0",
       configuration_version: "config.default",
       policy_version: "",
       ttl_ms: 60000
     }
   })

<- Ack { ok: true, session_state: OPEN }
```

The `sender` field is left empty because the runtime overrides it with the authenticated identity. The empty `policy_version` resolves to the built-in `policy.default`, which imposes no governance constraints beyond the mode's own rules.

Session IDs must be either UUID v4/v7 in hyphenated lowercase form or base64url tokens of at least 22 characters. Short or human-readable IDs like `"my-session"` are rejected.

### Step 3: Exchange messages

In Decision Mode, participants propose options, evaluate them, and vote. The session initiator and declared participants can all send proposals. Evaluations and votes reference proposals by ID.

```
-> Send(Envelope { message_type: "Proposal", payload: ProposalPayload {
     proposal_id: "p1",
     option: "deploy-v2",
     rationale: "All tests passing, metrics stable"
   }})
<- Ack { ok: true }

-> Send(Envelope { sender: "agent://analyst", message_type: "Vote", payload: VotePayload {
     proposal_id: "p1",
     vote: "APPROVE",
     reason: "Risk assessment passed"
   }})
<- Ack { ok: true }
```

### Step 4: Commit

The session initiator binds the terminal outcome. The commitment payload must echo the session's bound `mode_version` and `configuration_version` -- the runtime rejects mismatches.

```
-> Send(Envelope { message_type: "Commitment", payload: CommitmentPayload {
     commitment_id: "c1",
     action: "decision.selected",
     authority_scope: "deployment",
     reason: "Unanimous approval for deploy-v2",
     mode_version: "1.0.0",
     configuration_version: "config.default",
     policy_version: "policy.default",
     outcome_positive: true
   }})
<- Ack { ok: true, session_state: RESOLVED }
```

The session is now terminal. Any subsequent messages targeting it are rejected with `SESSION_NOT_OPEN`.

## Authentication

### Development mode

In development mode, clients set the `x-macp-agent-id` gRPC metadata header to declare their identity:

```
metadata: { "x-macp-agent-id": "agent://my-agent" }
```

The runtime trusts this header directly. This is only available when `MACP_ALLOW_DEV_SENDER_HEADER=1` is set.

### Production mode

Create a `tokens.json` file that maps bearer tokens to agent identities and capabilities:

```json
[
  {
    "token": "secret-token-for-analyst",
    "sender": "agent://analyst",
    "allowed_modes": ["macp.mode.decision.v1", "macp.mode.task.v1"],
    "can_start_sessions": true,
    "max_open_sessions": 10,
    "can_manage_mode_registry": false
  },
  {
    "token": "secret-token-for-reviewer",
    "sender": "agent://reviewer",
    "allowed_modes": [],
    "can_start_sessions": true,
    "can_manage_mode_registry": false
  }
]
```

Setting `allowed_modes` to an empty array grants access to all modes. The runtime derives the sender identity from the token, so agents cannot spoof their identity. Clients authenticate by sending `Authorization: Bearer <token>` in the gRPC metadata.

## Running the example clients

The repository includes example clients in `src/bin` that demonstrate each mode. Start the development server in one terminal, then run any example in another:

```bash
# Terminal 1: start the server
export MACP_ALLOW_INSECURE=1 && export MACP_ALLOW_DEV_SENDER_HEADER=1 && cargo run

# Terminal 2: run examples
cargo run --bin client              # Decision mode
cargo run --bin proposal_client     # Proposal mode
cargo run --bin task_client         # Task mode
cargo run --bin handoff_client      # Handoff mode
cargo run --bin quorum_client       # Quorum mode
cargo run --bin multi_round_client  # Multi-round extension
cargo run --bin fuzz_client         # Error path testing
```

## Common errors

| Error | Cause | Fix |
|-------|-------|-----|
| `UNAUTHENTICATED` | No valid credential provided | Set `MACP_ALLOW_DEV_SENDER_HEADER=1` and send the `x-macp-agent-id` header |
| `INVALID_ENVELOPE` | Missing required SessionStart fields | Ensure `participants`, `mode_version`, `configuration_version`, and `ttl_ms > 0` are all present |
| `SESSION_NOT_OPEN` | Session already resolved or expired | Use `GetSession` to check state; start a new session |
| `INVALID_SESSION_ID` | Session ID format not accepted | Use UUID v4/v7 or base64url (22+ characters) |
| `FORBIDDEN` | Sender not authorized for this message | Check the mode's authority rules; ensure the sender is in the participants list |
| `RATE_LIMITED` | Too many requests per minute | Wait for the rate window to expire, or increase the limit via environment variables |

## Next steps

- [**Examples**](examples.md) for worked examples of every mode
- [**API Reference**](API.md) for the full gRPC surface
- [**Modes**](modes.md) for runtime implementation details
- [**Deployment Guide**](deployment.md) for production setup
