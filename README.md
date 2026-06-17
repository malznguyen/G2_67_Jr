# G2_67_Jr — GraphRAG Multi-tenant Self-host

Self-hosted GraphRAG platform with strict tenant isolation (RLS + TenantContext), built on Rust (axum/sqlx), Qdrant, Keycloak, Redis, MinIO, and Next.js.

## Architecture (9 self-host services)

| # | Service   | Role                                                |
|---|-----------|-----------------------------------------------------|
| 1 | postgres16 | Primary OLTP store with Row-Level Security (RLS)    |
| 2 | qdrant     | Vector store for embeddings                         |
| 3 | minio      | S3-compatible object storage for uploads            |
| 4 | redis      | Cache / queue / rate-limit                          |
| 5 | ollama     | Local embedding / completion model runtime          |
| 6 | keycloak   | OIDC identity provider (multi-tenant realm)         |
| 7 | backend    | Rust (axum/sqlx) API — entry for business queries   |
| 8 | worker     | Rust background worker — ingestion / GraphRAG jobs  |
| 9 | frontend   | Next.js admin & tenant console                      |

## Invariants (do not break)

1. Every business query MUST go through `TenantContext` — no tenant UUID from URL feeds business logic directly.
2. PostgreSQL Row-Level Security is the source of truth for tenant isolation; the backend enforces it as a second layer.
3. TDD is mandatory: red test → FAIL → minimal implementation → PASS → dedicated commit.

## Quick start

```bash
cp .env.example .env
docker compose -f infra/docker-compose.yml up -d
```

## Repository layout

```
.
├── infra/
│   ├── docker-compose.yml   # 9 self-host services + healthchecks
│   ├── postgres/init.sql    # roles, RLS scaffolding
│   └── minio/init.sh        # bucket bootstrap (gmrag-uploads)
├── docs/progress/           # task reports
├── .env.example             # environment template
└── README.md
```

## License

Internal — team use only.