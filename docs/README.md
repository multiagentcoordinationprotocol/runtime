# MACP Runtime Documentation

Welcome to the Multi-Agent Coordination Protocol (MACP) Runtime documentation. This guide explains everything about the system in plain language, even if you don't know Rust.

## What Is This Project?

The MACP Runtime (also called Minimal Coordination Runtime or MCR) is a **server** that helps multiple AI agents or programs coordinate with each other. Think of it as a traffic controller for conversations between different agents.

### Real-World Analogy

Imagine you're organizing a meeting:
1. Someone starts the meeting (SessionStart)
2. People send messages back and forth
3. Eventually, the meeting reaches a decision (Resolved state)
4. Once resolved, no more messages can be sent

The MACP Runtime manages this entire lifecycle automatically and enforces the rules.

## What Problem Does It Solve?

When multiple AI agents or programs need to work together, they need a way to:
- **Start a conversation** (create a session)
- **Exchange messages** safely
- **Track the state** of the conversation
- **Know when it's done** (resolved)
- **Prevent messages after it's done** (enforce invariants)

Without a coordination runtime, each agent would need to implement all this logic themselves, leading to bugs and inconsistencies.

## Key Concepts

### Sessions
A **session** is like a conversation thread. Each session has:
- A unique ID
- A current state (Open, Resolved, or Expired)
- A time-to-live (TTL) - how long before it expires
- Optional resolution data (the final outcome)

### Messages
**Messages** are sent within a session. Each message includes:
- Which session it belongs to
- A unique message ID
- Who sent it
- When it was sent
- The actual content (payload)

### States
Sessions go through different **states**:
1. **Open** - Active, accepting messages
2. **Resolved** - Decision made, no more messages allowed
3. **Expired** - TTL expired (planned feature)

### The Protocol
The **MACP protocol** defines the rules for:
- How to format messages
- What fields are required
- How sessions transition between states
- What errors can occur

## How It Works (High Level)

```
Client                          MACP Runtime
  |                                  |
  |--SessionStart("s1")------------->|
  |<-------Ack(accepted=true)--------|
  |                                  |
  |--Message("hello")--------------->|
  |<-------Ack(accepted=true)--------|
  |                                  |
  |--Message("resolve")------------->|  (session now RESOLVED)
  |<-------Ack(accepted=true)--------|
  |                                  |
  |--Message("more")---------------->|
  |<---Ack(accepted=false, ----------|
  |     error="SessionNotOpen")      |
```

## What's Built With

- **gRPC**: A high-performance communication protocol (like HTTP but faster)
- **Protocol Buffers**: A way to define structured data (like JSON but more efficient)
- **Rust**: A programming language known for safety and speed

You don't need to know Rust to understand the concepts - the documentation explains everything in plain language.

## Components

This runtime consists of:

1. **Server** (`macp-runtime`) - The main runtime that manages sessions
2. **Client** (`client`) - A test client demonstrating basic usage
3. **Fuzz Client** (`fuzz_client`) - A test client that tries to break the rules

## Documentation Structure

- **[architecture.md](./architecture.md)** - How the system is designed internally
- **[protocol.md](./protocol.md)** - The MACP protocol specification
- **[examples.md](./examples.md)** - Step-by-step usage examples

## Quick Start

**Terminal 1** - Start the server:
```bash
cargo run
```

**Terminal 2** - Run a test client:
```bash
cargo run --bin client
```

You'll see the client send messages and the server respond with acknowledgments.

## Next Steps

1. Read [protocol.md](./protocol.md) to understand the MACP protocol
2. Read [architecture.md](./architecture.md) to understand how it's built
3. Read [examples.md](./examples.md) to see practical usage
