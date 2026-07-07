# Dev Quickstart

Assumes the one-time Postgres, OpenFGA, and Keycloak bootstrap has already been completed.

## Start everything (Docker, for normal use)

```powershell
docker compose -f infra/docker-compose.yml --env-file .env up -d
```

## Stop everything (keeps data)

```powershell
docker compose -f infra/docker-compose.yml down
```

## Backend-only, for Rust dev with fast iteration

Run from the repo root when Docker dependencies are running and you want the API on the host. Stop the Docker backend first if the full stack is up.

```powershell
docker compose -f infra/docker-compose.yml --env-file .env up -d postgres16 qdrant minio redis openfga keycloak ollama
docker compose -f infra/docker-compose.yml --env-file .env stop backend
$pgPassword=(Select-String '^POSTGRES_PASSWORD=' .env).Line.Split('=',2)[1]
$env:DATABASE_URL="postgres://gmrag:$pgPassword@localhost:5432/gmrag"
$env:QDRANT_URL="http://localhost:6334"
$env:REDIS_URL="redis://localhost:6379/0"
$env:S3_ENDPOINT="http://localhost:9000"
$env:KEYCLOAK_ISSUER="http://localhost:8080/realms/gmrag"
$env:OPENFGA_API_URL="http://localhost:8089"
$env:OLLAMA_HOST="http://localhost:11434"
$env:GMRAG_HTTP_BIND="127.0.0.1:8088"
cargo run --manifest-path backend/Cargo.toml -p gmrag-api
```

## Frontend-only, for Next.js dev with hot reload

Uses `frontend/.env.local`.

```powershell
cd frontend
pnpm dev
```

## Worker-only, for ingestion testing

Run from the repo root when Docker dependencies are running and you want the worker on the host. Stop the Docker worker first if the full stack is up.

```powershell
docker compose -f infra/docker-compose.yml --env-file .env up -d postgres16 qdrant minio redis openfga keycloak ollama
docker compose -f infra/docker-compose.yml --env-file .env stop worker
$pgPassword=(Select-String '^POSTGRES_PASSWORD=' .env).Line.Split('=',2)[1]
$env:DATABASE_URL="postgres://gmrag:$pgPassword@localhost:5432/gmrag"
$env:QDRANT_URL="http://localhost:6334"
$env:REDIS_URL="redis://localhost:6379/0"
$env:S3_ENDPOINT="http://localhost:9000"
$env:KEYCLOAK_ISSUER="http://localhost:8080/realms/gmrag"
$env:OPENFGA_API_URL="http://localhost:8089"
$env:OLLAMA_HOST="http://localhost:11434"
$env:GMRAG_WORKER_METRICS_BIND="127.0.0.1:9091"
cargo run --manifest-path backend/Cargo.toml -p gmrag-worker
```
