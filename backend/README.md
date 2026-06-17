# Backend (Rust)

Rust workspace for the gmrag backend. Multi-crate layout:

```
backend/
├── Cargo.toml                  # workspace manifest
├── crates/
│   ├── core/   (gmrag-core)    # config, error, db pool — shared by api+worker
│   ├── api/    (gmrag-api)     # axum HTTP entry point
│   └── worker/ (gmrag-worker)  # background job runner
└── migrations/                 # sqlx migrations (shared by api+worker)
```

## Quick start (local, no docker)

```bash
# 1. copy env template and edit values (DATABASE_URL points at your local pg)
cp ../.env.example .env

# 2. run the API (boot sequence: tracing → config → pool → migrate → serve)
cargo run -p gmrag-api

# 3. probe
curl -v http://localhost:8080/health    # 200 OK  (liveness, no DB)
curl -v http://localhost:8080/healthz   # 200 OK  (readiness, DB ping)
```

## Build both binaries

```bash
cargo build --release -p gmrag-api -p gmrag-worker
```

## Tests

```bash
cargo test --workspace
```
