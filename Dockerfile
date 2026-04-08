# Stage 1: Build
FROM rust:1.89-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies: copy manifests first, build a dummy, then copy real source
COPY Cargo.toml Cargo.lock build.rs ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && \
    mkdir -p src/bin && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Copy full source and build for real
COPY src/ src/
COPY tests/ tests/
RUN cargo build --release

# Stage 2: Minimal runtime image
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash macp
USER macp
WORKDIR /home/macp

COPY --from=builder /app/target/release/macp-runtime /usr/local/bin/macp-runtime

ENV MACP_BIND_ADDR=0.0.0.0:50051
ENV MACP_ALLOW_INSECURE=1
ENV MACP_DATA_DIR=/home/macp/.macp-data

EXPOSE 50051

ENTRYPOINT ["macp-runtime"]
