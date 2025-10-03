# Multi-stage build for minimal production image
FROM rust:1.82-slim as builder

WORKDIR /app

# Install dependencies
RUN apt-get update && \
    apt-get install -y pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source
COPY src ./src

# Build for release (without dev-srs)
RUN cargo build --release --bin tinyzkp_api

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/tinyzkp_api /usr/local/bin/

# Create non-root user
RUN useradd -m -u 1000 tinyzkp && \
    chown -R tinyzkp:tinyzkp /app

USER tinyzkp

# Railway sets PORT automatically
ENV TINYZKP_ADDR=0.0.0.0:${PORT:-8080}

EXPOSE ${PORT:-8080}

CMD ["tinyzkp_api"]