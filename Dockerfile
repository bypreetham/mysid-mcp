# Stage 1: Build native Rust binary
FROM rust:1.80-slim-bookworm AS builder
WORKDIR /app

COPY rust-mcp/Cargo.toml ./
COPY rust-mcp/src ./src

RUN cargo build --release

# Stage 2: Lean runtime container for Glama & Docker MCP
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app

COPY --from=builder /app/target/release/mysid /usr/local/bin/mysid

# stdio mode for standard Model Context Protocol (MCP) clients
ENTRYPOINT ["mysid"]
