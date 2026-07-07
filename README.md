# 🧠 GMRAG 2.0

<div align="center">

![Build](https://img.shields.io/badge/build-unverified-lightgrey?style=for-the-badge&logo=rust)
![Version](https://img.shields.io/badge/version-2.0.0--T84D-blue?style=for-the-badge)
![Rust](https://img.shields.io/badge/Rust-1.78+-orange?style=for-the-badge&logo=rust)
![PostgreSQL](https://img.shields.io/badge/PostgreSQL-16-336791?style=for-the-badge&logo=postgresql&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)
![Status](https://img.shields.io/badge/status--in--development-yellow?style=for-the-badge)

**Hệ thống Hỏi-Đáp Thông Minh đa người dùng thế hệ mới**

*Kết hợp GraphRAG · Phân quyền ReBAC Zanzibar-style · Citation chính xác tới từng trang PDF*

[📚 Tài liệu API](#-tài-liệu-liên-quan) · [🚀 Quick Start](#-quick-start) · [⚙️ Cấu hình](#️-biến-môi-trường) · [🗺️ Roadmap](#️-roadmap-p2)

</div>

---

## 📖 Giới thiệu

**GMRAG 2.0** là backend cho hệ thống RAG (Retrieval-Augmented Generation) thế hệ mới. Dự án tích hợp **đồ thị tri thức (Knowledge Graph)**, kiểm soát quyền truy cập cấp tài liệu theo mô hình **Zanzibar**, và trích xuất **citation chính xác tới từng trang PDF** — tất cả được xây dựng trên nền tảng Rust với hiệu năng cao và độ tin cậy.

> Trạng thái: đang phát triển (in-development). Badge build/status chưa được xác thực tự động trong môi trường hiện tại — xem `docs/progress/V2_PHASE_0.md` cho kết quả kiểm tra thực tế.

Phiên bản **T84D** là cột mốc refactor toàn diện: giải quyết các race condition nghiêm trọng, hoàn thiện pipeline ingest với Transactional Outbox Pattern, và bổ sung đầy đủ page metadata cho citation.

---

## ✨ Tính năng nổi bật

| # | Tính năng | Mô tả |
|---|-----------|-------|
| 🕸️ | **GraphRAG** | Tích hợp đồ thị tri thức (graph nodes + edges) vào pipeline RAG, cải thiện chất lượng trả lời nhờ ngữ cảnh quan hệ giữa các thực thể |
| 🔐 | **Phân quyền ReBAC (OpenFGA)** | Mô hình Zanzibar-style qua OpenFGA (runtime engine duy nhất), relation tuples, kiểm soát quyền truy cập cấp tài liệu và workspace; fail-closed 503 |
| 📬 | **Ingest Outbox chống mất job** | Transactional Outbox Pattern đảm bảo không mất job khi upload; relay + sweeper tự động recover stuck jobs |
| 📄 | **Citation chính xác tới trang PDF** | Trích xuất `page_start`/`page_end` cho mỗi chunk; Frontend nhận citation kèm số trang để nhảy trang PDF viewer |
| 🏢 | **Multi-tenant hoàn chỉnh** | Mỗi tenant có collection Qdrant riêng, RLS PostgreSQL cô lập dữ liệu tuyệt đối |
| 💬 | **Chat History trong LLM** | Backend tự động load lịch sử hội thoại vào LLM context (configurable qua `GMRAG_CHAT_HISTORY_LIMIT`) |
| 📊 | **Graph API phân trang cursor** | Hỗ trợ dataset lớn hàng trăm nghìn graph nodes mà không bị timeout |
| ⚡ | **Async Rust + Axum** | Tokio async runtime, zero-cost abstractions, memory-safe, tối ưu cho I/O-bound workloads |

---

## 🛠️ Tech Stack

| Thành phần | Công nghệ | Ghi chú |
|------------|-----------|---------|
| **API Server** | Rust + Axum | Tokio async runtime |
| **Database** | PostgreSQL 16 | Row-Level Security, `sqlx` |
| **Vector DB** | Qdrant | 768-dim, cosine similarity |
| **Message Queue** | Redis | `LPUSH`/`BRPOP` pattern |
| **Object Storage** | MinIO / AWS S3 | Tương thích S3 API |
| **LLM (local)** | Ollama | Local inference, `llama3.1:8b` default |
| **LLM (remote)** | DeepSeek | Remote BYOK (Bring Your Own Key) |
| **Auth** | OIDC / Keycloak | JWT validation, JWKS endpoint |
| **Authorization** | OpenFGA v1.18.1 | Zanzibar-style ReBAC, runtime engine duy nhất (Check/ListObjects), fail-closed 503 |
| **Worker** | Rust binary | `gmrag-worker` — ingest pipeline |

---

## 📁 Cấu trúc dự án

```
gm_rag_2.0/
├── backend/
│   ├── Cargo.toml                        # Rust workspace
│   ├── migrations/                       # SQLx migrations (chronological)
│   └── crates/
│       ├── api/                          # HTTP API server (gmrag-api)
│       │   └── src/
│       │       ├── routes/               # Axum route handlers
│       │       ├── chat/                 # RAG retrieval + streaming SSE
│       │       ├── openapi/              # OpenAPI schema tự sinh
│       │       └── middleware/           # RLS context, CORS, auth JWT
│       ├── worker/                       # Background worker (gmrag-worker)
│       │   └── src/
│       │       ├── relay.rs              # Outbox relay (T84D)
│       │       ├── sweeper.rs            # Stuck job sweeper (T84D)
│       │       └── job.rs                # Ingest job processor
│       └── core/                         # Shared types, config, Qdrant client
├── infra/
│   ├── docker-compose.yml                # Full stack local dev
│   └── backend.Dockerfile                # Multi-stage Rust build
└── docs/
    ├── FRONTEND_API_CONTRACT.md          # API contract cho Frontend
    └── SYSTEM_OVERVIEW.md                # Kiến trúc tổng quan
```

---

## 🚀 Quick Start

### Bước 1 — Cài đặt prerequisites

```bash
# Docker & Docker Compose (xem https://docs.docker.com/get-docker/)

# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# SQLx CLI
cargo install sqlx-cli --no-default-features --features postgres

# OpenFGA CLI, required by scripts/openfga-bootstrap.ps1
# Download/install from: https://github.com/openfga/cli/releases
```

### Bước 2 — Copy & cấu hình môi trường

```bash
cp .env.example .env
# Chỉnh sửa .env với các thông tin kết nối của bạn
# (xem bảng biến môi trường bên dưới)
```

### Bước 3 — Khởi động infrastructure

```bash
docker compose -f infra/docker-compose.yml up -d \
  postgres16 qdrant redis minio keycloak
```

> ⏳ Chờ khoảng 10–15 giây để các service sẵn sàng trước khi tiếp tục.

> ⚠️ **Lưu ý volume reuse:** `infra/postgres/init.sql` chỉ chạy ở lần đầu
> khởi tạo volume `gmrag-pgdata`. Khi reuse volume, database `openfga` sẽ
> KHÔNG tồn tại và `openfga-migrate` sẽ fail. Chạy idempotent script
> `pwsh ./scripts/ensure-openfga-db.ps1` **trước** khi start openfga-migrate
> (xem Bước 3b).

### Bước 3b — Bootstrap OpenFGA (database + store/model)

Thứ tự chính xác (quan trọng cho fresh clone HOẶC reused volume):

```bash
# 1. Đảm bảo database `openfga` tồn tại (idempotent — chạy mọi lúc được)
pwsh ./scripts/ensure-openfga-db.ps1

# 2. Chạy OpenFGA migrations + start OpenFGA
docker compose -f infra/docker-compose.yml up -d openfga-migrate openfga ollama

# 3. Bootstrap store + authorization model (in ra STORE_ID / MODEL_ID)
pwsh ./scripts/openfga-bootstrap.ps1
# → OPENFGA_STORE_ID=01...
# → OPENFGA_AUTHORIZATION_MODEL_ID=01...
# Paste 2 ID này vào .env, rồi restart API/worker để pick up.
```

### Bước 3c — Pull Ollama models (bắt buộc cho indexing + chat)

Ollama ship *empty* — không có model nào được cài sẵn. Không pull thì worker
embedding call và chat/graph LLM call sẽ 404. One-shot:

```bash
pwsh ./scripts/setup-ollama.ps1
# Pull mặc định: nomic-embed-text (bắt buộc) + llama3.1:8b (nếu không có DeepSeek).
# Nếu set DEEPSEEK_API_KEY trong .env, chat+graph dùng DeepSeek, chỉ cần embed model.
```

### Bước 3d — Bootstrap Keycloak realm + audience mapper

```bash
docker cp infra/keycloak/bootstrap.sh gmrag-keycloak:/tmp/bootstrap.sh
docker exec -e KEYCLOAK_ADMIN=admin -e KEYCLOAK_ADMIN_PASSWORD=<pw> \
  gmrag-keycloak bash /tmp/bootstrap.sh
# (optional) seed 5 demo users:
docker cp infra/keycloak/seed-users.sh gmrag-keycloak:/tmp/seed-users.sh
docker exec -e KEYCLOAK_ADMIN=admin -e KEYCLOAK_ADMIN_PASSWORD=<pw> \
  gmrag-keycloak bash /tmp/seed-users.sh
```

`bootstrap.sh` tự tạo `aud=gmrag-backend` audience mapper trên backend client
— không có mapper này, service-account token sẽ bị reject với `InvalidAudience`.

### Bước 4 — Chạy database migrations

```bash
cd backend
sqlx migrate run --database-url "$DATABASE_URL"
```

### Bước 5 — Build & chạy backend

```bash
cd backend

# Build release (lần đầu mất ~2–3 phút)
cargo build --release --bin gmrag-api --bin gmrag-worker

# Terminal 1 — API Server
./target/release/gmrag-api

# Terminal 2 — Background Worker
./target/release/gmrag-worker
```

**Hoặc** chạy toàn bộ stack bằng Docker Compose:

```bash
docker compose -f infra/docker-compose.yml up --build
```

### Bước 6 — Kiểm tra health

```bash
# Liveness check
curl http://localhost:8080/health
# → {"status":"ok","service":"gmrag-api","uptime_ms":1234}

# Readiness check (kiểm tra DB connection)
curl http://localhost:8080/healthz
# → {"status":"ok","db":"ok"}
```

### Bước 7 — Xem OpenAPI Docs

```
http://localhost:8080/openapi.json
```

Import vào [Swagger UI](https://editor.swagger.io/) hoặc **Postman** để khám phá toàn bộ API.

---

## ⚙️ Biến môi trường

### Biến bắt buộc

| Biến | Mô tả |
|------|-------|
| `DATABASE_URL` | PostgreSQL connection string |
| `KEYCLOAK_ISSUER` | OIDC issuer URL (ví dụ: `http://keycloak:8080/realms/gmrag`) |
| `KEYCLOAK_CLIENT_ID` | Client ID trong Keycloak realm |
| `KEYCLOAK_CLIENT_SECRET` | Client secret |
| `S3_ENDPOINT` | MinIO hoặc AWS S3 endpoint |
| `S3_ACCESS_KEY` | S3 access key |
| `S3_SECRET_KEY` | S3 secret key |
| `S3_BUCKET` | Tên bucket lưu trữ tài liệu |
| `REDIS_URL` | Redis connection (default: `redis://localhost:6379`) |

### Biến T84D — Mới trong phiên bản này

| Biến | Default | Mô tả |
|------|---------|-------|
| `GMRAG_OCR_ENABLED` | `false` | Bật OCR cho PDF scan (yêu cầu `libpdfium`) |
| `DATABASE_MAX_CONNECTIONS` | `10` | Pool size PostgreSQL |
| `GMRAG_OUTBOX_POLL_INTERVAL_SECS` | `3` | Chu kỳ polling của Outbox relay (giây) |
| `GMRAG_SWEEP_INTERVAL_SECS` | `60` | Chu kỳ Sweeper kiểm tra stuck jobs (giây) |
| `GMRAG_CHAT_HISTORY_LIMIT` | `10` | Số tin nhắn lịch sử đưa vào LLM context |

### Biến LLM

| Biến | Default | Mô tả |
|------|---------|-------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama server endpoint |
| `OLLAMA_EMBED_MODEL` | `nomic-embed-text` | Model sinh embedding (768-dim) |
| `OLLAMA_LLM_MODEL` | `llama3.1:8b` | Model sinh câu trả lời |
| `DEEPSEEK_API_KEY` | — | Nếu dùng DeepSeek thay vì Ollama |
| `DEEPSEEK_MODEL` | `deepseek-v4-flash` | Model DeepSeek |

### Biến OpenFGA (phân quyền runtime)

| Biến | Default | Mô tả |
|------|---------|-------|
| `OPENFGA_API_URL` | — (bắt buộc) | Host-side OpenFGA HTTP endpoint for bootstrap scripts (vd: `http://localhost:8089`) |
| `OPENFGA_INTERNAL_API_URL` | `http://openfga:8080` | Docker-internal OpenFGA HTTP endpoint injected into backend/worker |
| `OPENFGA_STORE_ID` | — (bắt buộc) | ID của OpenFGA store — output của `scripts/openfga-bootstrap.ps1` |
| `OPENFGA_AUTHORIZATION_MODEL_ID` | — (bắt buộc) | ID của authorization model — output của bootstrap script |
| `OPENFGA_API_TOKEN` | — | Bearer token cho OpenFGA (nếu bật preshared key) |
| `OPENFGA_REQUEST_TIMEOUT_MS` | `1500` | Timeout mỗi lệnh Check/ListObjects (ms) |
| `OPENFGA_HIGHER_CONSISTENCY_WINDOW_SECS` | `5` | Cửa sổ nhất quán cao (giây) |
| `OPENFGA_DATASTORE_URI` | — | Postgres URI cho OpenFGA migrate container |
| `OPENFGA_DATASTORE_ENGINE` | `postgres` | OpenFGA datastore engine |
| `OPENFGA_HTTP_PORT` | `8089` | Port HTTP của OpenFGA service |
| `OPENFGA_STORE_NAME` | `gmrag-v2` | Tên store — dùng bởi bootstrap script |

---

## 🧪 Chạy Tests

```bash
cd backend

# ✅ Unit tests — Không cần Postgres hay Qdrant
SQLX_OFFLINE=true cargo test --workspace --lib

# ✅ Integration tests — Cần Postgres đang chạy
cargo test -p gmrag-api --test openapi       # 5/5 tests
cargo test -p gmrag-worker --lib             # 58/58 tests

# ✅ Full test suite — Cần Postgres + Qdrant
cargo test --workspace
```

> 💡 **Tip:** Chạy `SQLX_OFFLINE=true` để test nhanh trong CI mà không cần database thật.

---

## 🗃️ Database Migrations (T84D)

Phiên bản T84D bổ sung **4 migrations** mới:

| File | Mục đích |
|------|---------|
| `20260623100000_ingest_outbox.sql` | Transactional outbox table + RLS policy |
| `20260623101000_ingest_jobs_claim.sql` | Thêm cột `claimed_at` cho sweeper phát hiện stuck jobs |
| `20260623102000_graph_node_documents.sql` | Bảng join provenance giữa graph nodes và documents |
| `20260623103000_document_chunks_pages.sql` | Thêm cột `page_start`/`page_end` cho citation |

Để xem trạng thái migrations:

```bash
cd backend
sqlx migrate info --database-url "$DATABASE_URL"
```

---

## ✅ Trạng thái T84D — Production Checklist

### Đã hoàn thành

| Ưu tiên | Hạng mục | Trạng thái |
|---------|----------|-----------|
| 🔴 P0 | Race condition upload-enqueue đã fix (Outbox Pattern) | ✅ Done |
| 🔴 P0 | Graph ACL provenance đã implement | ✅ Done |
| 🔴 P0 | Tenant delete teardown (Qdrant + S3) | ✅ Done |
| 🟠 P1 | Page metadata trong citation (`page_start`/`page_end`) | ✅ Done |
| 🟠 P1 | Messages API endpoint mới | ✅ Done |
| 🟠 P1 | Chat history trong LLM context | ✅ Done |
| 🟠 P1 | Graph cursor pagination | ✅ Done |
| 🟠 P1 | Configurable DB pool size | ✅ Done |
| ⭐ Extra | OpenAPI spec tự sinh | ✅ Done |
| ⭐ Extra | CORS middleware cấu hình được | ✅ Done |

---

## 🗺️ Roadmap P2

Các tính năng dưới đây chưa được triển khai trong T84D, dự kiến cho sprint tiếp theo:

| Hạng mục | Mô tả |
|----------|-------|
| ⏳ Token count on ingest | Đếm và lưu token count khi xử lý tài liệu |
| ⏳ Citation snippet/score trong SSE | Trả về đoạn trích và relevance score kèm citation qua streaming |
| ⏳ Prometheus metrics | Endpoint `/metrics` với các counter/histogram quan trọng |
| ⏳ Rate limiting | Per-user / per-tenant rate limit trên API |

---

## 📚 Tài liệu liên quan

| Tài liệu | Mô tả |
|----------|-------|
| [`docs/FRONTEND_API_CONTRACT.md`](docs/FRONTEND_API_CONTRACT.md) | API contract đầy đủ cho Frontend — endpoint, request/response schema, SSE format |
| [`docs/SYSTEM_OVERVIEW.md`](docs/SYSTEM_OVERVIEW.md) | Kiến trúc tổng quan hệ thống, data flow, mô hình phân quyền |

---

<div align="center">

**GMRAG 2.0** — *Built with ❤️ and Rust*

</div>
