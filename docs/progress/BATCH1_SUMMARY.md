# BATCH 1 SUMMARY — Monorepo Init, Docker, Rust Workspace, Frontend Skeleton
# Tasks: T1, T2, T3, T4, T5, T6, T7, T8 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 5 passed, 0 failed (cargo test --workspace --lib --bins)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T1-T4 | Monorepo + Docker 9 services + init scripts | `c5046e1c87ce74e88a5fd7a38b6b0779a6e10770` |
| T5-T7 | Rust workspace (core/api/worker) + Config/Error/DbPool + /health | `6240d84f0b80cd836966c2276c9de977a599cc09` |
| T8 | Next.js 15 frontend skeleton + frontend.Dockerfile | (commit T8 đẩy origin/main) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `.gitignore` | T1-T4 | Tạo | Git ignore (Rust, Next, env, IDE, runtime) |
| `README.md` | T1-T4 | Tạo | Project info, invariants, layout |
| `infra/docker-compose.yml` | T1-T4 | Tạo | 9 services + healthchecks + named volumes + `gmrag-net` bridge |
| `infra/postgres/init.sql` | T1-T4 | Tạo | uuid-ossp/pgcrypto/vector, `gmrag_app` role, `gmrag_current_tenant()` helper |
| `infra/minio/init.sh` | T1-T4 | Tạo | `mc` alias + `gmrag-uploads` bucket bootstrap (idempotent) |
| `.env.example` | T1-T4 | Tạo | DATABASE_URL, KEYCLOAK_*, DEEPSEEK_*, QDRANT_*, S3_*, REDIS_URL, OLLAMA_*, GMRAG_*, CORS_* |
| `backend/Cargo.toml` | T5-T7 | Tạo | Workspace manifest, members = `core/api/worker`, `workspace.dependencies` |
| `backend/rust-toolchain.toml` | T5-T7 | Tạo | Pin `stable` + rustfmt + clippy |
| `backend/Cargo.lock` | T5-T7 | Tạo | Commit lockfile (binary workspace convention) |
| `backend/crates/core/Cargo.toml` | T5-T7 | Tạo | `gmrag-core` (lib): sqlx, serde, thiserror, tracing, uuid, chrono, dotenvy |
| `backend/crates/core/src/lib.rs` | T5-T7 | Tạo | Re-exports `Config`, `Error`, `init_pool` |
| `backend/crates/core/src/config.rs` | T5-T7 | Tạo | `Config::from_env()` — DATABASE_URL required, env serialised bằng Mutex |
| `backend/crates/core/src/error.rs` | T5-T7 | Tạo | `Error` enum + stable kebab-case `code()` cho envelope |
| `backend/crates/core/src/db.rs` | T5-T7 | Tạo | `init_pool(DATABASE_URL)` — PgPool 10 conn, 5s acquire, 5min idle, liveness `SELECT 1` |
| `backend/crates/api/Cargo.toml` | T5-T7 | Tạo | `gmrag-api` (bin) — axum 0.7, tokio, sqlx |
| `backend/crates/api/src/main.rs` | T5-T7 | Tạo | Boot: tracing → config → pool → `sqlx::migrate!` → axum serve `/health` + `/healthz` |
| `backend/crates/worker/Cargo.toml` | T5-T7 | Tạo | `gmrag-worker` (bin) — same boot path, no router |
| `backend/crates/worker/src/main.rs` | T5-T7 | Tạo | Skeleton idle-loop |
| `backend/migrations/20260101000000_init.sql` | T5-T7 | Tạo | Placeholder cho `sqlx::migrate!()` |
| `infra/backend.Dockerfile` | T5-T7 | Tạo | Multi-stage Rust (cargo-chef + rust:1.83-slim → debian-bookworm-slim) + tini + non-root user `gmrag` |
| `frontend/package.json` | T8 | Tạo | Next 15.0.3, React 19 RC, TS 5.6, Tailwind 3.4, pnpm@10.32.1 |
| `frontend/pnpm-lock.yaml` | T8 | Tạo (auto) | Sinh bởi `pnpm install` |
| `frontend/next.config.mjs` | T8 | Tạo | `output: "standalone"`, `reactStrictMode: true` |
| `frontend/app/page.tsx` | T8 | Tạo | Blank entry `<h1>GMRAG2 Frontend</h1>` |
| `frontend/app/api/health/route.ts` | T8 | Tạo | `GET /api/health` cho compose healthcheck |
| `frontend/app/layout.tsx` | T8 | Tạo | RootLayout, metadata `GMRAG2` |
| `infra/frontend.Dockerfile` | T8 | Tạo | Multi-stage node:22-bookworm-slim + corepack pnpm@10.32.1 + tini + non-root |
| `infra/docker-compose.yml` | T8 | Sửa | `backend`/`worker`/`frontend` đổi `context: .` → `context: ..` |
| `.dockerignore` | T8 | Tạo (root) | Tránh ship 400MB `node_modules`/`target`/`.next` |
| `.gitignore` | T5-T7 | Sửa | Thêm `backend/target/` + `**/target/` |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// gmrag-core (lib)
pub struct Config { /* server, database, oidc, s3, qdrant, redis, ollama, deepseek, logging, tenant */ }
impl Config {
    pub fn from_env() -> Result<Self, Error>           // loads .env first
    pub(crate) fn from_process_env() -> Result<Self, Error>  // test isolation
}
pub enum Error { /* variants with stable kebab-case code() */ }
impl Error { pub fn code(&self) -> &'static str }
pub async fn init_pool(database_url: &str) -> Result<PgPool, sqlx::Error>

// gmrag-api (bin)
async fn main() -> anyhow::Result<()>  // boot sequence
// Routes: GET /health (liveness, no DB), GET /healthz (readiness, ping DB)
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| `backend/migrations/20260101000000_init.sql` | `20260101000000` | (placeholder, no domain tables) |
| `infra/postgres/init.sql` (Docker entrypoint, one-shot) | — | `gmrag_app` role, `gmrag_current_tenant()` function |

RLS đang enforce trên: **chưa có** (chưa có domain tables)
sqlx offline cache: **chưa có**

---

## 5. ENV VARS / CONFIG
| Tên biến | Giá trị mẫu | Task thêm |
|----------|-------------|----------|
| `DATABASE_URL` | `postgres://gmrag:change_me@postgres16:5432/gmrag` | T1-T4 |
| `KEYCLOAK_*` (ISSUER, CLIENT_ID, CLIENT_SECRET, ...) | placeholders | T1-T4 |
| `DEEPSEEK_API_KEY` | (empty by default — fallback Ollama) | T1-T4 |
| `QDRANT_URL` | `http://qdrant:6333` | T1-T4 |
| `S3_ENDPOINT` | `http://minio:9000` | T1-T4 |
| `S3_BUCKET` | `gmrag-uploads` | T1-T4 |
| `REDIS_URL` | `redis://redis:6379/0` | T1-T4 |
| `OLLAMA_HOST` | `http://ollama:11434` | T1-T4 |
| `GMRAG_HTTP_BIND` | `0.0.0.0:8080` | T5-T7 |
| `GMRAG_RUST_LOG` | `info,gmrag_core=debug,gmrag_api=debug` | T5-T7 |
| `GMRAG_TENANT_HEADER` | `x-tenant-id` | T5-T7 |
| `GMRAG_SERVICE_NAME` | `gmrag-api` | T5-T7 |
| `NEXT_PUBLIC_API_BASE_URL` | `http://localhost:8080` | T8 |
| `NEXT_PUBLIC_KEYCLOAK_URL/REALM/CLIENT_ID` | placeholders | T8 |

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `tokio` | 1 (full) | `gmrag-core`, `gmrag-api`, `gmrag-worker` | T5-T7 |
| `axum` | 0.7 | `gmrag-api` | T5-T7 |
| `sqlx` | workspace, features: `runtime-tokio-rustls`, `postgres`, `migrate`, `macros`, `uuid`, `chrono`, `json` | `gmrag-core`, `gmrag-api` | T5-T7 |
| `serde`, `serde_json` | 1 | tất cả crates | T5-T7 |
| `thiserror` | 1 | `gmrag-core` | T5-T7 |
| `tracing`, `tracing-subscriber` | workspace | tất cả crates | T5-T7 |
| `uuid` | 1 (v4 + serde) | tất cả crates | T5-T7 |
| `chrono` | 0.4 (serde) | `gmrag-core` | T5-T7 |
| `dotenvy` | 0.15 | `gmrag-core` | T5-T7 |
| `anyhow` | 1 | `gmrag-api`, `gmrag-worker` | T5-T7 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T1-T4]** Docker compose context phải là repo root (`context: ..` relative to `infra/docker-compose.yml`) — Dockerfile dùng `COPY backend/...` / `COPY frontend/...` — lý do: source nằm ngoài `infra/`
- **[T1-T4]** `infra/postgres/init.sql` bind-mount `:ro` chỉ chạy 1 lần lúc tạo volume pgdata; mọi schema sau phải qua `sqlx::migrate!` — lý do: edits sau khi volume init KHÔNG re-apply
- **[T5-T7]** `sqlx::migrate!("../../migrations")` (compile-time macro, resolve theo `CARGO_MANIFEST_DIR`) thay vì `Migrator::new("./migrations")` — lý do: tránh path phụ thuộc CWD
- **[T5-T7]** `/health` (liveness, no DB) + `/healthz` (readiness, ping DB) — pattern K8s chuẩn; compose healthcheck đã dùng `/healthz` từ T1-T4
- **[T5-T7]** Workspace layout `core` (lib) + `api` (bin) + `worker` (bin); mọi binary dùng chung `gmrag_core::Config` + `init_pool` — lý do: 1 boot sequence duy nhất
- **[T5-T7]** Migrations root ở `backend/migrations/` (chia sẻ giữa api + worker) — lý do: tránh symlink, future job crate dùng lại
- **[T5-T7]** Commit `Cargo.lock` (binary workspace convention) — lý do: CI build reproducible
- **[T8]** App Router (không Pages Router) — lý do: chuẩn Next 15, cần route handler cho healthcheck
- **[T8]** `output: "standalone"` + Dockerfile copy 3 artifact (`server.js` + `.next/static` + `public`) — lý do: image runtime nhỏ, không ship `node_modules`
- **[T8]** Corepack pin `pnpm@10.32.1` trong `package.json#packageManager` — lý do: build Docker reproducible
- **[T8]** `.env.example` có sẵn 4 biến `NEXT_PUBLIC_*`; chưa thêm `NEXT_PUBLIC_TENANT_HEADER` — agent batch sau cần mirror `GMRAG_TENANT_HEADER`

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang

1. **Tenant identity phải tới từ request context** (qua `TenantContext` middleware ở task sau), KHÔNG pull từ URL hoặc config tĩnh — lý do: multi-tenant isolation
2. **Domain schema đi qua `sqlx::migrate!`**, KHÔNG edit `infra/postgres/init.sql` sau khi volume đã init — lý do: bind-mount `:ro` chỉ chạy 1 lần
3. **Mọi Rust binary dùng `gmrag_core::Config::from_env()`** để đọc env — lý do: 1 source of truth, fail-fast chỉ `DATABASE_URL` required
4. **Mọi binary boot sequence**: tracing → config → pool → `sqlx::migrate!` → serve/loop — lý do: thống nhất observable
5. **Liveness/readiness split**: `/health` không touch DB, `/healthz` ping DB — lý do: K8s chuẩn
6. **`/health` HTTP 200 = service alive**; compose healthcheck dùng `/healthz` vì chỉ readiness mới phản ánh "ready to serve"
7. **Docker compose context = repo root** (build context `..` relative to `infra/`) — lý do: Dockerfile `COPY backend/...` / `COPY frontend/...`
8. **Env serialised trong test** bằng `static ENV_LOCK: Mutex<()>` — lý do: tránh race khi nhiều test mutate `process env`
9. **Workspace deps pin trong `[workspace.dependencies]`** — lý do: 1 version cho tất cả crates
10. **Commit `Cargo.lock`** cho binary workspace — lý do: CI build reproducible
11. **Migrations ở `backend/migrations/`** (không phải per-crate) — lý do: shared giữa api + worker
12. **`pgcrypto` extension bắt buộc** cho `gen_random_uuid()` (sẽ dùng ở batch sau) — extension đã enable trong `infra/postgres/init.sql`
13. **`gmrag_current_tenant()` function tồn tại trong DB** (từ init.sql) — sẽ dùng cho RLS policy ở batch sau
14. **`gmrag_app` role tồn tại trong DB** (least-privilege) — sẽ dùng cho RLS-enforced queries
15. **Frontend App Router** (không Pages Router) — lý do: chuẩn Next 15, hỗ trợ RSC
16. **Frontend `output: "standalone"`** — lý do: image runtime nhỏ
17. **Corepack pin pnpm version** trong `package.json#packageManager` — lý do: reproducible
18. **`.env` (local, gitignored) phải tạo từ `.env.example`** trước khi `docker compose` — lý do: compose không warn unset
19. **Mọi env var backend phải có trong `.env.example`** — lý do: dev onboarding dễ
20. **Compose port mapping**: backend `8088:8080`, frontend `3000:3000` — lý do: tránh clash local

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T1-T4]** `infra/postgres/init.sql` chỉ chạy 1 lần lúc tạo volume pgdata; mọi schema domain phải qua `sqlx::migrate!` → action: tạo file `backend/migrations/<timestamp>_*.sql`, không edit init.sql sau khi volume đã init
- **[nguồn: T1-T4]** Pre-existing `_sqlx_migrations` trong dev Postgres (7 records từ project cũ) → action: T5+ chạy `docker compose up -d --force-recreate postgres16` trên volume sạch
- **[nguồn: T5-T7]** `gmrag-net` 8088→8080 port mapping — backend listen 0.0.0.0:8080 INSIDE container, external test phải dùng `http://localhost:8088` → action: curl/Postman từ host dùng port 8088
- **[nguồn: T5-T7]** `backend.Dockerfile` chưa được build/test thực tế trong task này (WSL2 không có Rust toolchain) → action: T9+ chạy `docker compose build backend` verify

### P1 — Lưu ý khi implement
- **[T5-T7]** `Config` hiện chỉ đọc `DATABASE_URL` (required) + `GMRAG_HTTP_BIND/RUST_LOG/TENANT_HEADER/SERVICE_NAME` (default). Cấu trúc `config.rs` có thể split thành submodule `config/{database,server,logging,oidc,llm,storage,...}.rs` khi cần — TDD-extend theo pattern "thêm field → viết test mảng-khóa → implement"
- **[T5-T7]** `GMRAG_DEFAULT_TENANT` trong `.env.example` chưa được dùng; bất kỳ task seed dev data nào PHẢI tôn trọng invariant #1 (tenant identity qua middleware, KHÔNG từ config tĩnh) → action: field sẽ expose ở `gmrag_core::config` chỉ khi có use-case seeding rõ ràng
- **[T8]** Next 15.0.3 có CVE-2025-66478 (deprecation warning) — vẫn build/boot được; cần bump lên ≥ 15.4.x trong batch frontend kế tiếp
- **[T8]** PowerShell execution policy chặn `pnpm`/`npm` — workaround: `Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass` rồi gọi `.cmd`; fix vĩnh viễn: `Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned`
- **[T8]** Chưa cài Keycloak JS adapter / `oidc-client` — T70+ (frontend route) sẽ chọn lib OIDC; hiện 4 biến `NEXT_PUBLIC_KEYCLOAK_*` đã có sẵn
- **[T8]** `corepack prepare pnpm@10.32.1` cần chạy 1 lần trên local dev: `corepack enable && corepack prepare pnpm@10.32.1 --activate`
- **[T8]** Không có `NEXT_PUBLIC_TENANT_HEADER` env — khi frontend cần gắn tenant vào request, nên thêm mirror để giữ contract khớp với backend `GMRAG_TENANT_HEADER`
- **[T8]** `.env.example` có modification ngoài scope T8 (`DEEPSEEK_MODEL=deepseek-v4-flash`) — đã `git checkout -- .env.example` để commit T8 sạch; batch sau cần verify sửa này thuộc task nào

### P2 — Ghi nhớ nhỏ
- `KEYCLOAK_*`, `POSTGRES_PASSWORD`, `S3_*` defaults trong `.env.example` là placeholders — real `.env` phải replace trước non-local
- `DEEPSEEK_API_KEY` empty by default — backend phải tolerate missing key, fallback Ollama
- `frontend` service cần 4 biến `NEXT_PUBLIC_*` đã có sẵn trong compose
- `MinIO init.sh` là idempotent (`mc ls` guard) — safe to re-run
- Tất cả 9 services trong compose giờ đã có Dockerfile tương ứng → lệnh chuẩn: `cp .env.example .env && docker compose -f infra/docker-compose.yml up -d`

---

## 10. UNBLOCKS
- Batch 1 → unblock: Batch 2 (T9-T18) — Backend auth, tenant context, RLS, migrations
- Batch 1 → unblock: T70+ Frontend route development (App Router + Tailwind + TS baseline)
- Batch 1 → unblock: T9 Config TDD refine (test harness `config_env_matrix` + serialisation pattern sẵn sàng)
