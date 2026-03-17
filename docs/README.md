# MACP Runtime documentation

This directory documents the runtime implementation profile for `macp-runtime v0.4.0`.

The RFC/spec repository is still the normative source for MACP semantics. These runtime docs focus on how this implementation behaves today: startup configuration, security model, persistence profile, mode surface, and local-development examples.

## What is in this runtime profile

- unary-first MACP server over gRPC
- five standards-track modes from the main RFC repository
- one experimental `macp.mode.multi_round.v1` mode kept off discovery surfaces
- strict canonical `SessionStart` for standards-track modes
- authenticated sender derivation
- payload limits and rate limiting
- optional file-backed persistence for sessions and accepted-history logs

## Standards-track modes

- `macp.mode.decision.v1`
- `macp.mode.proposal.v1`
- `macp.mode.task.v1`
- `macp.mode.handoff.v1`
- `macp.mode.quorum.v1`

## Freeze profile

The current runtime is intended to be the freeze candidate for unary SDKs and reference examples.

Implemented and supported:

- `Initialize`
- `Send`
- `GetSession`
- `CancelSession`
- `GetManifest`
- `ListModes`
- `ListRoots`

Not part of the freeze surface:

- `StreamSession` is intentionally disabled in this profile
- `WatchModeRegistry` is unimplemented
- `WatchRoots` is unimplemented

## Security model

Production expectations:

- TLS transport
- bearer-token authentication
- runtime-derived `Envelope.sender`
- per-request authorization
- payload size limits
- rate limiting

Local development shortcut:

```bash
export MACP_ALLOW_INSECURE=1
export MACP_ALLOW_DEV_SENDER_HEADER=1
cargo run
```

In dev mode, example clients attach `x-macp-agent-id` metadata and may use plaintext transport.

## Persistence model

By default the runtime persists snapshots under `.macp-data/`:

- `sessions.json`
- `logs.json`

Disable persistence with:

```bash
export MACP_MEMORY_ONLY=1
```

## Document map

- `../README.md` — root-level quick start and configuration reference
- `examples.md` — updated local-development examples and canonical message patterns
- `protocol.md` — implementation notes and protocol surface summary
- `architecture.md` — runtime component layout
