# Stage 1: Build
FROM rust:1.88-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libpq-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependency compilation separately from application code
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    echo "" > src/lib.rs && \
    cargo build --release --locked --bin hivemind --features shared-backend-postgres && \
    rm -rf src

COPY src ./src
COPY tests ./tests
COPY schemas ./schemas

# Force rebuild of application code (touch all .rs so lib.rs isn't stale vs stub)
RUN find src -name "*.rs" -exec touch {} + && \
    cargo build --release --locked --bin hivemind --features shared-backend-postgres

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libpq5 \
    libssl3 \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hivemind /usr/local/bin/hivemind

ENV HIVEMIND_DIR=/data
ENV HIVEMIND_PORT=8080

EXPOSE 8080

VOLUME ["/data"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- "http://localhost:${HIVEMIND_PORT}/v1/health" | grep -q '"ok"' || exit 1

ENTRYPOINT ["hivemind"]
CMD ["serve"]
