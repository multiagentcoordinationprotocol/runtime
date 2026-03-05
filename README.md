# macp-runtime v0.1

Minimal Coordination Runtime (MCR)

## Run

Install protoc first.

Then:

    cargo build
    cargo run

Server runs on 127.0.0.1:50051

Send SessionStart, then Message.
If payload == "resolve", session transitions to RESOLVED.
