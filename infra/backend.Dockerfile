# =========================================================
# G2_67_Jr — Backend (Rust) multi-stage Dockerfile
#
# Build context is the repository root (`.`), so this file
# references `./backend` for sources and `./backend/Cargo.toml`
# as the workspace manifest. The same image is reused by the
# `worker` compose service (different `command:`).
#
# Image tag built from this Dockerfile: gmrag/backend:dev
# Matches `infra/docker-compose.yml` services `backend` and `worker`.
# =========================================================

# ---------- Stage 1: dependencies (cargo-chef) ----------
FROM rust:slim-bookworm AS chef
RUN cargo install --locked cargo-chef
WORKDIR /app

FROM chef AS planner
COPY backend/Cargo.toml backend/Cargo.lock* ./
COPY backend/crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# ---------- Stage 2: build ----------
FROM chef AS builder
WORKDIR /app

# Native build deps for ring/rustls/sqlx-postgres.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY backend/Cargo.toml backend/Cargo.lock* ./
COPY backend/crates ./crates
COPY backend/migrations ./migrations

# Build only the binaries we need. Adding new bins here is fine; this
# keeps the image slim and the build deterministic.
RUN cargo build --release --bin gmrag-api --bin gmrag-worker

# ---------- Stage 3: runtime ----------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates libssl3 wget tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1001 gmrag \
 && useradd  --system --uid 1001 --gid gmrag --create-home gmrag

WORKDIR /app
COPY --from=builder /app/target/release/gmrag-api    /usr/local/bin/gmrag-api
COPY --from=builder /app/target/release/gmrag-worker /usr/local/bin/gmrag-worker
COPY --from=builder /app/migrations                  /app/migrations

ENV RUST_LOG=info,gmrag_core=debug,gmrag_api=debug,gmrag_worker=debug \
    GMRAG_HTTP_BIND=0.0.0.0:8080

USER gmrag
EXPOSE 8080

# Compose overrides `command:` for the worker service.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["gmrag-api"]
