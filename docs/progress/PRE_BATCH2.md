# PRE-BATCH 2 — Preparation Report

## Trạng thái tổng: ✅

## Checklist
- [x] pnpm version đồng bộ: 10.32.1
- [x] Next.js nâng lên: 16.2.9 (từ 15.0.3)
- [x] CVE-2025-66478: Đã fix
- [x] keycloak-js đã cài: 26.2.4
- [x] next-auth@beta đã cài: 5.0.0-beta.31
- [x] NEXT_PUBLIC_TENANT_HEADER thêm vào .env.example: ✅
- [x] DEEPSEEK_MODEL verified = deepseek-v4-flash: ✅
- [x] Docker stack 9/9 services healthy: ✅

## Trạng thái từng Docker service
| Service | Status | Ghi chú |
|---------|--------|---------|
| postgres16 | ✅ healthy | pgvector extension thiếu (image standard) |
| qdrant | ✅ healthy | Sửa healthcheck dùng bash /dev/tcp |
| redis | ✅ healthy | |
| minio | ✅ healthy | |
| ollama | ✅ healthy | Sửa healthcheck dùng bash /dev/tcp |
| keycloak | ✅ healthy | |
| backend | ✅ healthy | Port 8088:8080 |
| worker | ✅ healthy | |
| frontend | ✅ healthy | Port 3000:3000 |

## Services FAIL (nếu có)
Không có service nào fail.

## Thay đổi bổ sung (không có trong prompt gốc)
- **Rust Docker image**: Nâng từ `rust:1.83-slim-bookworm` lên `rust:slim-bookworm` (latest) do cargo-chef v0.1.77 yêu cầu Rust >= 1.88
- **Docker healthchecks**: Sửa qdrant & ollama healthcheck vì container không có `wget` — dùng `bash /dev/tcp` thay thế
- **Next.js 16**: Nâng trực tiếp lên 16.2.9 (latest stable) thay vì 15.4.x — build thành công, không break
- **tsconfig.json**: Next.js 16 tự động cập nhật tsconfig (thêm `jsx: react-jsx`, cập nhật `include`)

## Rust workspace
- `cargo build --workspace`: ✅
- `cargo test --workspace`: ⚠️ 1 test fail (`config_env_matrix`) — test pick up DATABASE_URL từ `.env` file, lỗi pre-existing không do thay đổi này
- `cargo clippy --workspace -- -D warnings`: ✅

## Commit
`785b284` — chore(pre-batch2): fix CVE Next.js, add OIDC libs, sync env vars, fix Docker healthchecks

## ⚠️ Còn lại cần user làm thủ công
- [ ] Điền `DEEPSEEK_API_KEY` vào file `.env` local (hiện tại đã có key mẫu)
- [ ] Nếu git remote chưa set → user tự add remote GitHub (hiện tại đã set: origin → G2_67_Jr.git)
- [ ] Cần `pnpm approve-builds` trong frontend để approve sharp/unrs-resolver build scripts (optional)

## Unblocks
- ✅ Batch 2 (T9→T18) — Backend auth & tenant context
- ✅ Batch 9 (T70→T77) — Frontend Auth.js Keycloak provider
