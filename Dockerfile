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

# Build for release (API + SRS generator)
RUN cargo build --release --bin tinyzkp_api --bin generate_production_srs

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /app/target/release/tinyzkp_api /usr/local/bin/
COPY --from=builder /app/target/release/generate_production_srs /usr/local/bin/

# Create SRS directory (SRS will be generated directly on Railway)
RUN mkdir -p /app/srs

# Copy entrypoint script
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Create non-root user
RUN useradd -m -u 1000 tinyzkp && \
    mkdir -p /app/srs && \
    chown -R tinyzkp:tinyzkp /app

# Don't switch to tinyzkp user yet - entrypoint needs root to access volume
# USER tinyzkp will be set in entrypoint after copying files

# Railway sets PORT automatically
ENV TINYZKP_ADDR=0.0.0.0:${PORT:-8080}

EXPOSE ${PORT:-8080}

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]