# Handoff — ReBAC Authorization Phase (sau T60)

**Repo:** `gm_rag_2.0`  
**Commit HEAD mới nhất:** `5517cb6` — `docs(T60): record commit hash in progress report`  
**Toàn bộ work ReBAC (T64–T67, T83–T85) + sửa xlsx: chưa commit** — nằm trong working tree (modified + untracked).

---

## 1. Bối cảnh / quyết định

PM đổi hướng phân quyền **T61+**:
- **Chọn Option B:** ReBAC kiểu Zanzibar (`docs/5068.pdf`), native Rust trên bảng `resource_acl` có sẵn.
- **Loại:** OPA/Rego, OpenFGA, SpiceDB.
- **MVP:** chỉ API `Check` (không zookies, Leopard, Expand).
- **Plan file:** `rebac_authorization_(t61+)_55f68080.plan.md` — **không sửa**.

Tên plan “T61+” = **reshape task trong PM từ T61 trở đi**, không có nghĩa implement tuần tự bắt đầu từ T61.

---

## 2. Đã làm gì (code + test + docs)

### Sprint 7 — task cuối đã commit
| Task | Trạng thái | Ghi chú |
|------|------------|---------|
| **T60** | ✅ Committed | Document preview + chunk fetch |

### Sprint 7 — chưa làm (vẫn trong xlsx, “Chưa bắt đầu”)
| Task | Nội dung |
|------|----------|
| **T61** | `routes/chat.rs` — POST chat SSE + session ACL |
| **T62** | Chat sessions/messages CRUD + ACL |
| **T63** | `routes/graph.rs` — GET graph ACL-filtered |

**Không có file** `routes/chat.rs`, `routes/graph.rs`.

### Sprint 8 — ReBAC engine (T64–T67) — ✅ implement + test xanh, **chưa commit**

| Task | Deliverable |
|------|-------------|
| **T64** | Migration `backend/migrations/20260622000000_rebac_relation_tuples.sql` + `tests/schema_rebac.rs` (6 tests) |
| **T65** | `backend/crates/api/src/rbac/model.rs` — namespaces, relations, userset-rewrite |
| **T66** | `backend/crates/api/src/rbac/check.rs` — `check_relation()` đệ quy, bounded depth, RLS-scoped + `tests/rbac_check.rs` (10 tests) |
| **T67** | `backend/crates/api/src/routes/acl.rs` — GET/POST/DELETE grants + audit_log + owner guard + `tests/acl_routes.rs` |

**API routes mới (trong `lib.rs`):**
- `GET/POST /tenants/:tid/acl`
- `DELETE /tenants/:tid/acl/:grant_id`

### Tích hợp + FE + E2E (T83–T85) — ✅ implement, **chưa commit**

| Task | Deliverable |
|------|-------------|
| **T83** | `documents.rs` dùng `check_relation` (preview/delete) + list mở rộng predicate `resource_acl`; `tests/documents_acl.rs` (4 tests). **Chỉ documents** — chat/graph chưa có route. |
| **T84** | `frontend/lib/acl.ts` + `frontend/components/AclShareDialog.tsx` — **chưa mount** vào UI chính |
| **T85** | `tests/rebac_e2e.rs` (5 tests E2E/pentest) |

### Verify đã chạy (local, trước handoff)
- `cargo test -p gmrag-api` — pass
- `cargo clippy -p gmrag-api --all-targets -- -D warnings` (SQLX_OFFLINE=true) — pass

### Progress docs (untracked)
- `docs/progress/T64.md` … `T67.md`, `T83.md` … `T85.md`

---

## 3. Chưa làm gì

### Task PM chưa implement
- **T61–T63** — chat SSE, chat CRUD, graph API
- **T68–T69** — BYOK settings, metering
- **T70+** — frontend baseline (Documents UI, v.v.)

### Phần còn lại của ReBAC plan
- **T83 (chat/graph):** khi T61–T63 land → gọi `check_relation(chat_session, …)` thay ACL ad-hoc
- **T84 mount UI:** gắn `AclShareDialog` vào document/chat detail view
- **FE E2E share flow** (mở rộng T78/T79) — backend E2E đã có trong `rebac_e2e.rs`
- **Git commit** toàn bộ batch ReBAC
- **Cập nhật xlsx status** T64–T67, T83–T85 → “Hoàn thành” + cột Test đỏ/xanh/Commit (hiện sheet vẫn “Chưa bắt đầu” theo arithmetic dashboard 85 task / 25 chưa bắt đầu)

### Out of scope MVP (đã ghi trong plan)
- Zookies, Leopard index, Expand API, nested group-in-group, OPA/external engines

---

## 4. Sửa đổi cụ thể (file / schema / PM)

### Database
**Migration mới:** `20260622000000_rebac_relation_tuples.sql`
- `resource_acl.permission` default → `'viewer'`
- CHECK relation: `owner | editor | viewer`
- CHECK `principal_type`: `user | workspace`
- Index: `idx_resource_acl_check (resource_type, resource_id, permission)`
- **Không** tạo bảng mới; **không** đụng RLS (đã bật từ T25)

**Semantic mapping (Zanzibar):**
```
object   = (resource_type, resource_id)
relation = permission
user     = (principal_type, principal_id)   # user hoặc workspace userset
```
`owner` / `member` workspace **không** lưu trong `resource_acl` — lấy từ `documents.owner_id`, `chat_sessions.user_id`, `workspace_members`.

### Backend — file mới
```
backend/crates/api/src/rbac/mod.rs
backend/crates/api/src/rbac/model.rs
backend/crates/api/src/rbac/check.rs
backend/crates/api/src/routes/acl.rs
backend/migrations/20260622000000_rebac_relation_tuples.sql
backend/crates/api/tests/schema_rebac.rs
backend/crates/api/tests/rbac_check.rs
backend/crates/api/tests/acl_routes.rs
backend/crates/api/tests/documents_acl.rs
backend/crates/api/tests/rebac_e2e.rs
```

### Backend — file sửa
```
backend/crates/api/src/lib.rs          # pub mod rbac; wire ACL routes
backend/crates/api/src/routes/mod.rs   # pub mod acl;
backend/crates/api/src/routes/documents.rs  # check_relation integration (T83)
```

### Frontend — file mới
```
frontend/lib/acl.ts
frontend/components/AclShareDialog.tsx
```

### PM spreadsheet — **đã sửa** `docs/GMRAG2_Project_Management.xlsx`
| Hạng mục | Thay đổi |
|----------|----------|
| T64–T67 | Viết lại mô tả ReBAC; status vẫn **“Chưa bắt đầu”** |
| T83–T85 | Thêm rows 88–90 (Sprint 8/9/10) |
| Table_1 | `A5:T87` → **`A5:T90`** |
| Validations + CF | Mở rộng tới row 90 |
| Dashboard | COUNT ranges → `$90` (85 task total khi mở Excel) |
| Timeline | Formula ranges → row 90 |
| Rủi ro & Quyết định | **D7** ReBAC over OPA; **R9** check fan-out perf |
| ResourceBAC | SQL mẫu sửa `resource_acl`/`principal_*`; checklist J6–J10 = Hoàn thành |
| Chart | Giữ nguyên (`chart1.xml`) |

### Git — lưu ý thêm (cùng working tree, có thể từ batch khác)
Untracked, **không thuộc ReBAC core** nhưng chưa commit:
```
backend/crates/api/src/routes/tenants.rs
backend/crates/api/src/routes/tenant_members.rs
backend/crates/api/tests/tenant_routes.rs
docs/progress/T52_T54.md
docs/5068.pdf
docs/~$GMRAG2_Project_Management.xlsx   # lock file Excel — bỏ qua khi commit
```

Agent tiếp theo nên `git status` và tách commit theo batch (T52–T54 vs ReBAC T64–T85).

---

## 5. Kiến trúc ReBAC (tóm tắt cho agent mới)

```
check_relation(object, relation, principal)
  ├─ eval_this: owner column, visibility=shared, resource_acl tuple, workspace member
  ├─ concentric: owner → editor → viewer
  └─ tuple_to_userset: document.viewer ← workspace.member (qua workspace_id)
```

- Chạy trên **cùng `PgConnection`** với RLS (`SET LOCAL app.tenant_id`).
- `list_documents` dùng **predicate SQL biên dịch tay** (tránh N+1 Check) — nếu đổi `rbac/model.rs` phải đồng bộ predicate.
- ACL API: chỉ owner mới create/revoke grant; không grant `owner` qua tuple (owner = cột DB).

---

## 6. Gợi ý bước tiếp theo (ưu tiên)

1. **Commit** batch ReBAC (T64–T67, T83–T85) — tách khỏi tenants nếu cần.
2. **T61 → T62 → T63** (Sprint 7 chat/graph) — dùng `check_relation` ngay, không viết ACL inline.
3. Hoàn thiện **T83** cho chat/graph sau T61–T63.
4. **Mount `AclShareDialog`** khi FE baseline (T70+) sẵn.
5. Cập nhật **xlsx status + cột TDD** (Test đỏ/xanh/Commit) sau khi commit.

---

## 7. Tài liệu tham chiếu

| File | Mục đích |
|------|----------|
| `docs/5068.pdf` | Paper Zanzibar |
| `docs/progress/T64.md`–`T67.md`, `T83.md`–`T85.md` | Chi tiết TDD từng task |
| `backend/migrations/20260617144756_acl.sql` | Schema gốc `resource_acl` |
| Plan (attached, không sửa) | `rebac_authorization_(t61+)_55f68080.plan.md` |
