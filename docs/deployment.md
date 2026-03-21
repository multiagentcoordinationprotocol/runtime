# Deployment Guide

## Production checklist

1. **TLS certificates** — Set `MACP_TLS_CERT_PATH` and `MACP_TLS_KEY_PATH` to valid PEM files
2. **Auth tokens** — Create a `tokens.json` file mapping bearer tokens to agent identities and set `MACP_AUTH_TOKENS_FILE`
3. **Data directory** — Ensure `MACP_DATA_DIR` points to a directory with write permissions
4. **Bind address** — Set `MACP_BIND_ADDR` to the desired listen address (default: `127.0.0.1:50051`)

## Environment variable reference

| Variable | Required | Default | Description |
|---|---|---|---|
| `MACP_BIND_ADDR` | No | `127.0.0.1:50051` | gRPC listen address |
| `MACP_TLS_CERT_PATH` | Yes* | — | Path to TLS certificate PEM |
| `MACP_TLS_KEY_PATH` | Yes* | — | Path to TLS private key PEM |
| `MACP_AUTH_TOKENS_FILE` | No | — | Path to `tokens.json` for bearer token auth |
| `MACP_DATA_DIR` | No | `.macp-data` | Directory for session persistence |
| `MACP_MEMORY_ONLY` | No | — | Set to `1` to disable persistence |
| `MACP_ALLOW_INSECURE` | No | — | Set to `1` to allow plaintext (dev only) |
| `MACP_ALLOW_DEV_SENDER_HEADER` | No | — | Set to `1` to trust `x-macp-sender` header (dev only) |
| `MACP_MAX_OPEN_SESSIONS` | No | — | Per-initiator open session limit |
| `MACP_MAX_PAYLOAD_BYTES` | No | `1048576` | Maximum envelope payload size |

*TLS is required unless `MACP_ALLOW_INSECURE=1`.

## Persistence and crash recovery

When `MACP_MEMORY_ONLY` is not set:

- Each session gets a directory under `MACP_DATA_DIR/sessions/<session_id>/`
- An append-only `log.jsonl` records every accepted message and internal event
- A `session.json` snapshot is written on each state change (best-effort)
- On startup, all sessions are rebuilt from their `log.jsonl` files via `replay_session()`
- Log append failures are **fatal** — the runtime rejects the message rather than acknowledging without a durable record
- Atomic writes (tmp file + rename) prevent partial-write corruption

## Monitoring

- **stderr warnings** — Failed persistence operations, replay errors, and recovered session counts are logged to stderr
- **Session counts** — On startup, the runtime prints the number of replayed sessions
- **TTL expiry** — Sessions are lazily expired on next read or write; no background reaper
- **Rate limiting** — Per-sender rate limits for `SessionStart` and in-session messages

## Container deployment

```dockerfile
FROM rust:1.85 AS builder
WORKDIR /app
COPY . .
RUN apt-get update && apt-get install -y protobuf-compiler
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/macp-runtime /usr/local/bin/
EXPOSE 50051
VOLUME /data
ENV MACP_DATA_DIR=/data
CMD ["macp-runtime"]
```

Key considerations:

- Mount a persistent volume at `MACP_DATA_DIR` for session durability
- Expose port 50051 (or the configured `MACP_BIND_ADDR` port)
- Provide TLS certificates via mounted secrets
- Set `MACP_AUTH_TOKENS_FILE` to a mounted secrets file for production auth

## Dev tool prerequisites

For development, install these additional tools:

- `cargo-tarpaulin` — Coverage reporting (`cargo install cargo-tarpaulin`)
- `cargo-audit` — Dependency security auditing (`cargo install cargo-audit`)
- `buf` — Protocol buffer tooling (for proto sync and lint)
