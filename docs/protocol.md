# Runtime protocol profile

This document describes the current implementation profile of `macp-runtime v0.4.0`.

The RFC/spec repository is the normative source for protocol semantics. This file is intentionally short and only highlights the implementation choices that matter to SDK authors and local operators.

## Supported protocol version

- `macp_version = "1.0"`

Clients should call `Initialize` before using the runtime.

## Implemented RPCs

- `Initialize`
- `Send`
- `StreamSession`
- `GetSession`
- `CancelSession`
- `GetManifest`
- `ListModes`
- `ListRoots`

## Still unimplemented

- `WatchModeRegistry` is unimplemented
- `WatchRoots` is unimplemented

## StreamSession profile

`StreamSession` is session-scoped and authoritative for accepted envelopes:

- one gRPC stream binds to one non-empty `session_id`
- the server emits only accepted canonical MACP envelopes
- stream attachment observes future accepted envelopes from the bind point; it does not backfill earlier history
- accepted envelope order matches runtime admission order for that session
- mixed-session streams are rejected with `FAILED_PRECONDITION`
- stream-level validation failures terminate the stream with a gRPC status; use `Send` if you need explicit per-message negative acknowledgements
- to attach to an existing session without mutating it, send a session-scoped `Signal` envelope with the correct `session_id` and `mode`

## Standards-track mode rules

For these modes:

- `macp.mode.decision.v1`
- `macp.mode.proposal.v1`
- `macp.mode.task.v1`
- `macp.mode.handoff.v1`
- `macp.mode.quorum.v1`

`SessionStartPayload` must bind:

- `participants`
- `mode_version`
- `configuration_version`
- `ttl_ms`

Empty payloads are rejected. Empty `mode` values are rejected. Duplicate participant IDs are rejected.

## Experimental mode rule

`macp.mode.multi_round.v1` remains available as an explicit experimental mode. It is not advertised by discovery RPCs and retains a more permissive bootstrap path for backward compatibility.

## Security profile

Production profile:

- TLS transport
- sender derived from authenticated identity
- per-request authorization
- payload size caps
- rate limiting

Local development profile:

- plaintext allowed only with `MACP_ALLOW_INSECURE=1`
- sender header shortcut allowed only with `MACP_ALLOW_DEV_SENDER_HEADER=1`
- clients attach `x-macp-agent-id`

## Persistence profile

By default the runtime persists state via `FileBackend` under `MACP_DATA_DIR`:

- per-session `session.json` and append-only `log.jsonl` files
- crash recovery reconciles dedup state from the log on startup
- atomic writes (tmp file + rename) prevent partial-write corruption

This gives restart recovery for session metadata, dedup state, and accepted-history inspection. Corrupt or incompatible files produce a warning on stderr; the runtime falls back to empty state instead of refusing to start.

## Commitment validation

For standards-track modes, `CommitmentPayload` must carry version fields that match the session-bound values.

## Discovery notes

`ListModes` returns the five standards-track modes. `GetManifest` exposes a manifest that matches the implemented unary and streaming capabilities.
