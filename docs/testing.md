# Testing

The runtime has three levels of tests, plus a separate integration test crate that exercises the gRPC boundary with real agents.

## Unit tests and conformance

```bash
cargo test --all-targets          # unit tests + Rust integration tests
make test-conformance             # JSON fixture-driven conformance suite
make test-all                     # fmt → clippy → test → integration → conformance
```

Unit tests live inside `src/` modules (`#[cfg(test)]`). Conformance fixtures are in `tests/conformance/` and exercise each mode's happy path and reject paths from JSON definitions.

## Integration test suite

A separate Rust crate at `integration_tests/` tests the runtime through the real gRPC transport boundary. It is **not** part of the main Cargo build — `cargo build --release` ignores it entirely.

### Architecture

```
integration_tests/
  Cargo.toml              # Depends on macp-runtime (lib) + rig-core + tonic
  src/
    config.rs             # Test target configuration (local / CI / hosted)
    server_manager.rs     # Start/stop runtime as a subprocess on a free port
    helpers.rs            # Envelope builders, payload helpers, gRPC wrappers
    macp_tools/           # Rig Tool implementations for all MACP operations
  tests/
    tier1.rs → tier1_protocol/    # Scripted gRPC protocol tests
    tier2.rs → tier2_agents/      # Rig agent tool tests (no LLM)
    tier3.rs → tier3_e2e/         # Real OpenAI LLM agent tests
```

### Three tiers

| Tier | What | LLM | Tests | Speed |
|------|------|-----|-------|-------|
| **Tier 1: Protocol** | Scripted gRPC calls testing all modes, error paths, RFC cross-cutting features (signals, dedup, version binding, cancel auth) | None | 47 | <1s |
| **Tier 2: Rig Tools** | MACP operations as Rig `Tool` trait implementations, invoked via `ToolSet::call()` | None | 5 | <1s |
| **Tier 3: E2E** | Real GPT-4o-mini agents coordinating through the runtime. Orchestrator as plain code, specialists as LLM. Parallel execution. Signals on ambient plane. | OpenAI | 3 | ~15s |

### Running integration tests

```bash
# Build the runtime first
cargo build

# Run Tier 1 + 2 (no API keys needed)
cd integration_tests
MACP_TEST_BINARY=../target/debug/macp-runtime cargo test -- --test-threads=1

# Run individual tiers
MACP_TEST_BINARY=../target/debug/macp-runtime cargo test --test tier1 -- --test-threads=1
MACP_TEST_BINARY=../target/debug/macp-runtime cargo test --test tier2 -- --test-threads=1

# Run Tier 3 E2E (requires OPENAI_API_KEY)
OPENAI_API_KEY=sk-... MACP_TEST_BINARY=../target/debug/macp-runtime cargo test --test tier3 -- --ignored --test-threads=1

# Run against a hosted runtime (no local server started)
MACP_TEST_ENDPOINT=host:50051 cargo test -- --test-threads=1
```

Or use Makefile targets from the project root:

```bash
make test-integration-grpc      # Tier 1
make test-integration-agents    # Tier 2
make test-integration-e2e       # Tier 3 (needs OPENAI_API_KEY)
make test-integration-hosted    # All tiers against MACP_TEST_ENDPOINT
```

### Configuration

| Variable | Purpose | Default |
|----------|---------|---------|
| `MACP_TEST_BINARY` | Path to runtime binary (skip cargo build) | Builds from parent crate |
| `MACP_TEST_ENDPOINT` | Connect to hosted runtime (skip server start) | Start local server |
| `MACP_TEST_TLS` | Use TLS for hosted connection | `0` |
| `MACP_TEST_AUTH_TOKEN` | Bearer token for hosted runtime | Dev headers |
| `OPENAI_API_KEY` | Required for Tier 3 E2E tests | Tier 3 tests skip if unset |

### Tier 1 coverage

Protocol tests exercise every mode through gRPC:

- **Initialize**: protocol negotiation, version rejection, runtime info
- **Decision mode**: happy path, duplicate dedup, non-initiator commit rejection
- **Proposal mode**: happy path, premature commitment rejection
- **Task mode**: happy path, non-initiator request rejection, duplicate task rejection
- **Handoff mode**: happy path, accept-without-offer rejection
- **Quorum mode**: happy path, approve-before-request, premature commitment
- **Multi-round mode**: happy path, pre-convergence commit rejection
- **Signals**: valid signal accepted, session_id/mode violations rejected, WatchSignals broadcast
- **Version binding**: commitment with wrong mode_version/config_version rejected
- **Deduplication**: rejected messages don't consume dedup slots, duplicate SessionStart rejected
- **CancelSession**: non-initiator rejection
- **Session lifecycle**: TTL expiry, concurrent sessions, parallel session independence
- **Mode registry**: list/register/unregister extension modes
- **Discovery**: GetManifest returns all modes, Initialize rejects unsupported version

### Tier 2: Rig agent tools

Each MACP operation (start session, propose, vote, commit, etc.) is implemented as a Rig `Tool` trait. Tier 2 tests validate these tools work correctly by calling them through `ToolSet::call()` — the same interface an LLM agent would use. Tests cover all 5 standard modes.

### Tier 3: E2E with real LLM

Three tests use real OpenAI GPT-4o-mini agents:

1. **Decision with signals**: Orchestrator (code) proposes → 3 specialist LLMs evaluate in parallel → each sends progress/completed Signals on the ambient plane → orchestrator commits. Demonstrates both coordination plane and ambient plane simultaneously.

2. **Decision**: Same as above without signals — simpler version.

3. **Task delegation**: Planner (code) creates task → Worker (LLM) accepts and completes → planner commits.

Architecture follows the RFC:
- Orchestrator/planner operations are **plain code** (deterministic, no LLM needed)
- Specialist/worker reasoning uses **real LLM** (where domain expertise matters)
- Agents run **in parallel** (runtime serializes by acceptance order)
- LLM reasoning happens **outside the session** (ambient plane)
- Only the resulting Envelope enters the session

### CI/CD

Integration tests run via manual GitHub Actions dispatch (not on every PR):

```
Actions → "Integration Tests" → Run workflow → optionally check "Run Tier 3 E2E"
```

Tier 3 E2E requires the `OPENAI_API_KEY` repository secret.
