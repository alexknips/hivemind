# Stage 1: SPA build (Node.js)
FROM node:22-slim AS spa-builder

WORKDIR /app/website
COPY website/package.json website/package-lock.json ./
RUN npm ci --prefer-offline

COPY website/ ./
RUN npm run build

# Stage 2: Rust build
# Debian trixie (glibc 2.40) required: fastembed-rs/ort pre-built ONNX Runtime
# binaries use glibc 2.38+ symbols (__isoc23_strtol etc.) unavailable in bookworm.
FROM rust:1.88-slim-trixie AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libpq-dev \
    libssl-dev \
    g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Exact commit this image is built from. .dockerignore excludes .git from
# the build context, so build.rs can't `git rev-parse` here — CI and
# scripts/cell-update.sh both pass it explicitly instead (hivemind-zdsh.7).
ARG GIT_SHA=unknown
ENV HIVEMIND_BUILD_SHA=$GIT_SHA

# Cache dependency compilation separately from application code
COPY Cargo.toml Cargo.lock build.rs ./
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

# Stage 3: Runtime (trixie to match builder glibc ≥ 2.38 for ort/ONNX binaries)
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libpq5 \
    libssl3 \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hivemind /usr/local/bin/hivemind
COPY --from=spa-builder /app/website/dist /app/dist

ENV HIVEMIND_DIR=/data
# Inside the container the server must listen on every interface so the
# published port can reach it; who can reach that port is decided by the
# `ports:` mapping (docker-compose.yml publishes on 127.0.0.1). With no
# HIVEMIND_API_KEY and no HIVEMIND_DATABASE_URL `serve` refuses to start on
# this bind — an image run unauthenticated fails closed.
ENV HIVEMIND_BIND=0.0.0.0
ENV HIVEMIND_PORT=8080
ENV HIVEMIND_SPA_DIR=/app/dist

EXPOSE 8080

VOLUME ["/data"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- "http://localhost:${HIVEMIND_PORT}/v1/health" | grep -q '"ok"' || exit 1

ENTRYPOINT ["hivemind"]
CMD ["serve"]
