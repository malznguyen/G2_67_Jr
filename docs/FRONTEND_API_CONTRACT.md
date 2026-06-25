# FRONTEND API CONTRACT — GMRAG 2.0

> **Phiên bản tài liệu:** T84D (Phase 3 & 4 đã tích hợp)
> **Dành cho:** Dev Frontend
> **Cập nhật lần cuối:** 2026-06-24

---

## Mục lục

1. [Tổng quan](#1-tổng-quan)
2. [Authentication & Headers bắt buộc](#2-authentication--headers-bắt-buộc)
3. [Quy ước chung](#3-quy-ước-chung)
4. [Nhóm API: Documents](#4-nhóm-api-documents)
5. [Nhóm API: Chat Sessions](#5-nhóm-api-chat-sessions)
6. [Nhóm API: Graph](#6-nhóm-api-graph)
7. [Nhóm API: Workspaces](#7-nhóm-api-workspaces)
8. [Nhóm API: Tenants](#8-nhóm-api-tenants)
9. [Nhóm API: Users](#9-nhóm-api-users)
10. [Nhóm API: Settings](#10-nhóm-api-settings)
11. [Nhóm API: Metering & Audit](#11-nhóm-api-metering--audit)
12. [Nhóm API: ACL (Kiểm soát truy cập)](#12-nhóm-api-acl-kiểm-soát-truy-cập)
13. [Nhóm API: Workspace Members](#13-nhóm-api-workspace-members)
14. [Nhóm API: Tenant Members](#14-nhóm-api-tenant-members)
15. [Schemas & JSON Examples chi tiết](#15-schemas--json-examples-chi-tiết)
16. [Hướng dẫn xử lý SSE Stream](#16-hướng-dẫn-xử-lý-sse-stream)
17. [Error Handling](#17-error-handling)
18. [Lưu ý quan trọng cho T85 Frontend](#18-lưu-ý-quan-trọng-cho-t85-frontend)

---

## 1. Tổng quan

GMRAG 2.0 là hệ thống RAG (Retrieval-Augmented Generation) đa tenant. Backend viết bằng Rust, cung cấp REST API và SSE (Server-Sent Events) cho chat streaming. Kiểm soát truy cập theo mô hình **ReBAC** (Relationship-Based Access Control) tương tự Zanzibar của Google.

**Những thay đổi nổi bật từ T84D cần biết:**
- ✅ `GET /chat_sessions/{sid}/messages` — Endpoint **MỚI** để lấy lịch sử tin nhắn (Phase 3)
- ✅ SSE `citation` event giờ có thêm `page_start` và `page_end` (Phase 3.1)
- ✅ `GET /workspaces/{wid}/graph` hỗ trợ **cursor pagination** (Phase 4)
- ✅ Backend tự động inject chat history vào LLM context — Frontend **không cần** gửi history trong body

---

## 2. Authentication & Headers bắt buộc

Mọi request đến các route tenant-scoped đều **phải** có 2 header sau:

| Header | Giá trị | Mô tả |
|--------|---------|-------|
| `Authorization` | `Bearer <jwt_token>` | JWT access token của người dùng |
| `X-Tenant-ID` | `<tenant_uuid>` | UUID của tenant, phải khớp với `{tid}` trong URL path |

### Ví dụ request headers

```http
GET /tenants/550e8400-e29b-41d4-a716-446655440000/documents HTTP/1.1
Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
X-Tenant-ID: 550e8400-e29b-41d4-a716-446655440000
Content-Type: application/json
```

### Lưu ý

- Nếu thiếu `Authorization` → HTTP `401 Unauthorized`
- Nếu `X-Tenant-ID` không khớp với `{tid}` trong path → HTTP `400 Bad Request`
- JWT hết hạn → HTTP `401 Unauthorized` (cần refresh token và retry)

---

## 3. Quy ước chung

### Base URL

```
https://<host>/
```

> Hỏi team DevOps về host cụ thể cho từng môi trường (dev / staging / prod).

### Content-Type

| Loại request | Content-Type |
|---|---|
| Hầu hết các request | `application/json` |
| Upload tài liệu | `multipart/form-data` |
| Nhận SSE stream | `text/event-stream` (server gửi, client chỉ cần `Accept: text/event-stream`) |

### UUID

Tất cả các `id` field đều là UUID v4 dạng string, ví dụ:
```
550e8400-e29b-41d4-a716-446655440000
```

### Timestamps

Tất cả timestamp đều theo chuẩn **RFC3339 UTC**, ví dụ:
```
2026-06-23T10:00:00Z
```

### Phân trang

| API | Kiểu phân trang |
|-----|----------------|
| Hầu hết list API | Không phân trang (trả toàn bộ) |
| `GET /workspaces/{wid}/graph` | **Cursor-based pagination** (xem [mục 6](#6-nhóm-api-graph)) |

---

## 4. Nhóm API: Documents

### 4.1 Liệt kê tài liệu

```
GET /tenants/{tid}/documents?workspace_id={wid}
```

**Query params:**

| Param | Bắt buộc | Mô tả |
|-------|----------|-------|
| `workspace_id` | Có | UUID của workspace cần lấy tài liệu |

**Response `200`:**

```json
[
  {
    "id": "uuid",
    "title": "Báo cáo Q4 2025",
    "filename": "bao_cao_q4.pdf",
    "visibility": "shared",
    "workspace_id": "uuid",
    "owner_id": "uuid",
    "created_at": "2026-06-23T10:00:00Z"
  }
]
```

---

### 4.2 Upload tài liệu

```
POST /tenants/{tid}/documents
Content-Type: multipart/form-data
```

**Form fields:**

| Field | Kiểu | Bắt buộc | Mô tả |
|-------|------|----------|-------|
| `file` | binary | Có | File PDF, tối đa **50 MiB**, phải có filename |
| `visibility` | string | Có | `"shared"` hoặc `"private"` |
| `workspace_id` | string (UUID) | Có | UUID của workspace |
| `title` | string | Không | Tiêu đề hiển thị; mặc định = tên file |

**Ví dụ với `fetch` API:**

```javascript
const formData = new FormData();
formData.append('file', file, file.name); // file phải có filename
formData.append('visibility', 'shared');
formData.append('workspace_id', workspaceId);
formData.append('title', 'Báo cáo Q4 2025'); // optional

const response = await fetch(`/tenants/${tid}/documents`, {
  method: 'POST',
  headers: {
    'Authorization': `Bearer ${token}`,
    'X-Tenant-ID': tid,
    // KHÔNG set Content-Type thủ công — browser tự set với boundary
  },
  body: formData,
});
```

**Response `201`:**

```json
{ "id": "550e8400-e29b-41d4-a716-446655440000" }
```

**Lưu ý:**
- Giới hạn file: **50 MiB**
- Nếu vượt quota storage hoặc số lượng document → `429 Too Many Requests`
- **Đừng** set `Content-Type` header thủ công khi dùng `FormData` — browser sẽ tự thêm `boundary`

---

### 4.3 Xóa tài liệu

```
DELETE /tenants/{tid}/documents/{doc_id}
```

> Chỉ chủ sở hữu (`owner`) mới được xóa tài liệu của mình.

**Response `204`:** Không có body.

**Lỗi có thể gặp:**
- `404` — Tài liệu không tồn tại hoặc bạn không có quyền xem (ReBAC)

---

### 4.4 Preview chunks của tài liệu

```
GET /tenants/{tid}/documents/{doc_id}/preview
```

**Response `200`:**

```json
{
  "document_id": "uuid",
  "chunks": [
    {
      "index": 0,
      "text": "Nội dung đoạn văn bản thứ nhất...",
      "page_start": 1,
      "page_end": 2
    },
    {
      "index": 1,
      "text": "Nội dung đoạn văn bản thứ hai...",
      "page_start": 3,
      "page_end": 3
    }
  ]
}
```

---

## 5. Nhóm API: Chat Sessions

### 5.1 Liệt kê chat sessions

```
GET /tenants/{tid}/chat_sessions
```

**Response `200`:**

```json
[
  {
    "id": "uuid",
    "title": "Cuộc hội thoại về báo cáo Q4",
    "workspace_id": "uuid",
    "model": "deepseek-chat",
    "created_at": "2026-06-23T10:00:00Z",
    "updated_at": "2026-06-23T12:30:00Z"
  }
]
```

---

### 5.2 Tạo chat session mới

```
POST /tenants/{tid}/chat_sessions
Content-Type: application/json
```

**Request body:**

```json
{
  "title": "Cuộc hội thoại mới",
  "workspace_id": "uuid",
  "model": "deepseek-chat"
}
```

| Field | Bắt buộc | Mô tả |
|-------|----------|-------|
| `title` | Không | Tiêu đề session; mặc định do backend tự đặt |
| `workspace_id` | Không | Nếu `null` hoặc bỏ qua → chat **không có RAG** (không tra cứu tài liệu) |
| `model` | Không | Tên model LLM; mặc định theo cấu hình tenant |

**Response `201`:**

```json
{ "id": "550e8400-e29b-41d4-a716-446655440000" }
```

---

### 5.3 Xóa chat session

```
DELETE /tenants/{tid}/chat_sessions/{sid}
```

**Response `204`:** Không có body.

---

### 5.4 Lấy lịch sử tin nhắn *(MỚI — T84D Phase 3)*

```
GET /tenants/{tid}/chat_sessions/{sid}/messages
```

**Response `200`:**

```json
{
  "messages": [
    {
      "id": "uuid",
      "role": "user",
      "content": "Báo cáo Q4 nói về điều gì?",
      "token_count": null,
      "created_at": "2026-06-23T10:00:00Z"
    },
    {
      "id": "uuid",
      "role": "assistant",
      "content": "Báo cáo Q4 đề cập đến doanh thu tăng 25%...",
      "token_count": null,
      "created_at": "2026-06-23T10:00:05Z"
    }
  ]
}
```

**Chi tiết field:**

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `id` | string (UUID) | ID duy nhất của tin nhắn |
| `role` | string | `"user"` hoặc `"assistant"` |
| `content` | string | Nội dung tin nhắn |
| `token_count` | number \| null | Số token (hiện tại luôn là `null`, chờ Phase 2) |
| `created_at` | string (RFC3339) | Thời điểm tạo |

> **Sắp xếp:** `created_at ASC` — tin nhắn cũ nhất ở đầu, mới nhất ở cuối.

**Mục đích sử dụng:** Dùng endpoint này để **render lại lịch sử hội thoại** khi user reload trang hoặc mở lại session cũ.

**Lỗi có thể gặp:**
- `404` — Session không tồn tại hoặc bạn không có quyền `viewer` trên session này (ReBAC)

---

### 5.5 Chat (SSE Stream)

```
POST /tenants/{tid}/chat_sessions/{sid}/chat
Content-Type: application/json
Accept: text/event-stream
```

**Request body:**

```json
{ "message": "Câu hỏi của người dùng" }
```

> **Quan trọng:** Chỉ gửi câu hỏi hiện tại. Backend tự động load `N` tin nhắn gần nhất vào LLM context — **không cần** gửi lại toàn bộ lịch sử.

**Response:** SSE stream với `Content-Type: text/event-stream`

Xem chi tiết tại [mục 16 — Hướng dẫn xử lý SSE Stream](#16-hướng-dẫn-xử-lý-sse-stream).

---

## 6. Nhóm API: Graph

### 6.1 Lấy dữ liệu graph (có cursor pagination) *(MỚI — T84D Phase 4)*

```
GET /tenants/{tid}/workspaces/{wid}/graph?cursor={cursor}&limit={limit}
```

**Query params:**

| Param | Bắt buộc | Mặc định | Mô tả |
|-------|----------|----------|-------|
| `cursor` | Không | (bắt đầu từ đầu) | Cursor từ response trước |
| `limit` | Không | `200` | Số node tối đa mỗi trang, tối đa `500` |

**Cursor format:** `{RFC3339_timestamp}:{UUID}` — chuỗi opaque, **không cần parse**, chỉ cần truyền lại nguyên vẹn.

Ví dụ cursor: `2026-06-23T10:00:00Z:550e8400-e29b-41d4-a716-446655440000`

---

**Response `200`:**

```json
{
  "nodes": [
    {
      "id": "uuid",
      "kind": "Entity",
      "label": "Tên thực thể",
      "properties": {
        "description": "Mô tả thêm..."
      },
      "created_at": "2026-06-23T10:00:00Z"
    }
  ],
  "edges": [
    {
      "id": "uuid",
      "src_node_id": "uuid",
      "dst_node_id": "uuid",
      "kind": "RELATES_TO",
      "weight": 0.85,
      "properties": {},
      "created_at": "2026-06-23T10:00:00Z"
    }
  ],
  "next_cursor": "2026-06-23T11:00:00Z:uuid-of-last-node"
}
```

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `nodes` | array | Danh sách node trong trang hiện tại |
| `edges` | array | Danh sách cạnh kết nối trong trang hiện tại |
| `next_cursor` | string \| null | Cursor để lấy trang tiếp theo; `null` = đã hết dữ liệu |

---

### Cách phân trang đúng

```javascript
async function fetchAllGraphData(tid, wid) {
  const allNodes = [];
  const allEdges = [];
  let cursor = null;

  do {
    const url = cursor
      ? `/tenants/${tid}/workspaces/${wid}/graph?cursor=${encodeURIComponent(cursor)}&limit=200`
      : `/tenants/${tid}/workspaces/${wid}/graph?limit=200`;

    const res = await fetch(url, { headers: authHeaders });
    const data = await res.json();

    allNodes.push(...data.nodes);
    allEdges.push(...data.edges);
    cursor = data.next_cursor;

  } while (cursor !== null);

  return { nodes: allNodes, edges: allEdges };
}
```

**⚠️ Lưu ý ACL quan trọng:**
- `next_cursor` được tính **trước** khi áp dụng bộ lọc ACL
- Mỗi trang có thể có **ít node hơn** `limit` sau khi lọc quyền
- **Đừng dừng phân trang khi `nodes.length < limit`** — chỉ dừng khi `next_cursor === null`

---

## 7. Nhóm API: Workspaces

### 7.1 Liệt kê workspaces

```
GET /tenants/{tid}/workspaces
```

**Response `200`:**

```json
[
  {
    "id": "uuid",
    "name": "Dự án Alpha",
    "description": "Workspace cho nhóm Alpha",
    "created_at": "2026-06-01T00:00:00Z"
  }
]
```

---

### 7.2 Tạo workspace

```
POST /tenants/{tid}/workspaces
Content-Type: application/json
```

**Request body:**

```json
{
  "name": "Dự án Beta",
  "description": "Mô tả workspace mới"
}
```

**Response `201`:**

```json
{ "id": "uuid" }
```

---

### 7.3 Cập nhật workspace

```
PATCH /tenants/{tid}/workspaces/{wid}
Content-Type: application/json
```

**Request body** (tất cả field đều optional):

```json
{
  "name": "Tên mới",
  "description": "Mô tả mới"
}
```

**Response `200`:** Object workspace đã cập nhật.

---

### 7.4 Xóa workspace

```
DELETE /tenants/{tid}/workspaces/{wid}
```

**Response `204`:** Không có body.

---

## 8. Nhóm API: Tenants

### 8.1 Liệt kê tenants

```
GET /tenants
```

> Không cần `X-Tenant-ID` cho endpoint này (không phải tenant-scoped).

**Response `200`:** Danh sách tenant mà user hiện tại có quyền truy cập.

---

### 8.2 Tạo tenant

```
POST /tenants
Content-Type: application/json
```

**Response `201`:**

```json
{ "id": "uuid" }
```

---

### 8.3 Cập nhật tenant

```
PATCH /tenants/{tid}
Content-Type: application/json
```

**Response `200`:** Object tenant đã cập nhật.

---

### 8.4 Xóa tenant

```
DELETE /tenants/{tid}
```

**Response `204`:** Không có body.

---

## 9. Nhóm API: Users

### 9.1 Lấy thông tin người dùng hiện tại

```
GET /users/me
```

> Không cần `X-Tenant-ID`.

**Response `200`:**

```json
{
  "id": "uuid",
  "email": "user@example.com",
  "display_name": "Nguyễn Văn A",
  "created_at": "2026-01-01T00:00:00Z"
}
```

---

## 10. Nhóm API: Settings

### 10.1 Lấy cấu hình LLM của tenant

```
GET /tenants/{tid}/settings/llm
```

> Chỉ tenant **owner** mới được gọi endpoint này.

**Response `200`:**

```json
{
  "configured": true,
  "provider": "ollama",
  "model": "nomic-embed-text",
  "base_url": "http://ollama:11434",
  "dimensions": 768,
  "enabled": true,
  "llm_model": "deepseek-chat",
  "llm_base_url": "https://api.deepseek.com/v1",
  "has_api_key": false,
  "api_key_masked": "sk-…ab12"
}
```

**Chi tiết field:**

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `configured` | boolean | Tenant đã có cấu hình LLM hay chưa |
| `provider` | string \| null | `"ollama"` hoặc `"openai"` (embedding provider) |
| `model` | string \| null | Tên model embedding |
| `base_url` | string \| null | Endpoint của embedding provider |
| `dimensions` | number \| null | Số chiều vector embedding (thường `768`) |
| `enabled` | boolean \| null | Bật/tắt BYOK embedding cho tenant |
| `llm_model` | string \| null | Tên model LLM dùng cho chat (ví dụ `deepseek-chat`) |
| `llm_base_url` | string \| null | Endpoint của LLM provider |
| `has_api_key` | boolean | Tenant đã set API key hay chưa (không bao giờ trả raw key) |
| `api_key_masked` | string \| null | API key đã mask (chỉ hiện khi `has_api_key = true`) |

> **Lưu ý:** Không có trường `temperature`, `max_tokens`, hay `api_key_set`. Trường `api_key_set` ở phiên bản tài liệu cũ tương đương `has_api_key` hiện tại.

---

### 10.2 Cập nhật cấu hình LLM

```
PUT /tenants/{tid}/settings/llm
Content-Type: application/json
```

> Chỉ tenant **owner** mới được gọi. API key (nếu gửi) sẽ được **mã hóa AES-256-GCM** trước khi lưu.

**Request body:**

```json
{
  "provider": "openai",
  "model": "text-embedding-3-small",
  "base_url": null,
  "api_key": "sk-...",
  "dimensions": 768,
  "enabled": true,
  "llm_model": "deepseek-chat",
  "llm_base_url": "https://api.deepseek.com/v1"
}
```

| Field | Bắt buộc | Mô tả |
|-------|----------|-------|
| `provider` | Có | `"ollama"` hoặc `"openai"` |
| `model` | Có | Tên model embedding |
| `base_url` | Không | Endpoint embedding provider |
| `api_key` | Không | API key raw (sẽ được mã hóa khi lưu); gửi `null`/bỏ qua để giữ key cũ |
| `dimensions` | Không | Số chiều vector (default `768`) |
| `enabled` | Không | Bật/tắt BYOK embedding |
| `llm_model` | Không | Model LLM cho chat |
| `llm_base_url` | Không | Endpoint LLM provider |

**Response `200`:** Object settings đã cập nhật (cùng shape với `GET`, key trả về dạng `api_key_masked`).

---

## 11. Nhóm API: Metering & Audit

### 11.1 Lấy thống kê sử dụng

```
GET /tenants/{tid}/usage
```

> Chỉ tenant **owner** mới được gọi.

**Response `200`:**

```json
{
  "usage": [
    { "metric": "llm_tokens", "total": 1500 },
    { "metric": "embedding_tokens", "total": 8200 },
    { "metric": "document_count", "total": 42 },
    { "metric": "storage_used_bytes", "total": 104857600 }
  ]
}
```

**Chi tiết field:**

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `usage` | array | Danh sách metric; mỗi item có `{ metric: string, total: number }` |
| `usage[].metric` | string | Tên metric (ví dụ `llm_tokens`, `embedding_tokens`, `document_count`, `storage_used_bytes`) |
| `usage[].total` | number | Tổng giá trị tích lũy của metric |

> **Lưu ý:** Không phải object phẳng như phiên bản cũ. Mỗi metric là một phần tử trong mảng `usage`. Không có trường `period_start`/`period_end`.

---

### 11.2 Lấy quota của tenant

```
GET /tenants/{tid}/quota
```

> Chỉ tenant **owner** mới được gọi.

**Response `200`:**

```json
{
  "configured": true,
  "max_documents": 500,
  "max_workspaces": 10,
  "max_storage_bytes": 5368709120,
  "max_members": 50,
  "updated_at": "2026-06-23T10:00:00Z"
}
```

**Chi tiết field:**

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `configured` | boolean | Tenant đã có row quota hay dùng default |
| `max_documents` | number | Giới hạn số document |
| `max_workspaces` | number | Giới hạn số workspace |
| `max_storage_bytes` | number | Giới hạn dung lượng storage (bytes) |
| `max_members` | number | Giới hạn số thành viên tenant |
| `updated_at` | string (RFC3339) \| null | Lần cập nhật quota cuối |

> **Đổi tên field so với phiên bản cũ:** `max_document_count` → `max_documents`, `max_workspace_count` → `max_workspaces`, `max_member_count` → `max_members`. Thêm `configured` và `updated_at`.

---

### 11.3 Lấy audit logs

```
GET /tenants/{tid}/audit_logs
```

> Chỉ tenant **owner** mới được gọi.

**Response `200`:**

```json
[
  {
    "id": "uuid",
    "actor_id": "uuid",
    "action": "document.upload",
    "resource_type": "document",
    "resource_id": "uuid",
    "metadata": {},
    "created_at": "2026-06-23T10:00:00Z"
  }
]
```

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `id` | string (UUID) | ID log entry |
| `actor_id` | string (UUID) \| null | User thực hiện action |
| `action` | string | Tên action (ví dụ `document.upload`, `acl.grant.create`) |
| `resource_type` | string \| null | Loại resource bị tác động |
| `resource_id` | string (UUID) \| null | ID resource bị tác động |
| `metadata` | object \| null | Thông tin bổ sung (JSON tùy chọn) |
| `created_at` | string (RFC3339) | Thời điểm tạo |

---

## 12. Nhóm API: ACL (Kiểm soát truy cập)

### 12.1 Liệt kê grants

```
GET /tenants/{tid}/acl/grants
```

**Response `200`:** Danh sách ACL grant hiện tại của tenant.

---

### 12.2 Tạo grant mới

```
POST /tenants/{tid}/acl/grants
Content-Type: application/json
```

**Request body:**

```json
{
  "subject_id": "uuid",
  "subject_type": "user",
  "resource_id": "uuid",
  "resource_type": "workspace",
  "relation": "viewer"
}
```

**Response `201`:**

```json
{ "id": "uuid" }
```

---

### 12.3 Xóa grant

```
DELETE /tenants/{tid}/acl/grants/{grant_id}
```

**Response `204`:** Không có body.

---

## 13. Nhóm API: Workspace Members

### 13.1 Liệt kê thành viên workspace

```
GET /tenants/{tid}/workspaces/{wid}/members
```

**Response `200`:**

```json
[
  {
    "user_id": "uuid",
    "email": "user@example.com",
    "display_name": "Nguyễn Văn A",
    "role": "editor",
    "joined_at": "2026-06-01T00:00:00Z"
  }
]
```

---

### 13.2 Thêm thành viên vào workspace

```
POST /tenants/{tid}/workspaces/{wid}/members
Content-Type: application/json
```

**Request body:**

```json
{
  "user_id": "uuid",
  "role": "viewer"
}
```

**Response `201`:** Object member vừa được thêm.

---

### 13.3 Xóa thành viên khỏi workspace

```
DELETE /tenants/{tid}/workspaces/{wid}/members/{uid}
```

**Response `204`:** Không có body.

---

## 14. Nhóm API: Tenant Members

### 14.1 Liệt kê thành viên tenant

```
GET /tenants/{tid}/members
```

**Response `200`:** Danh sách member của tenant.

---

### 14.2 Mời thành viên mới

```
POST /tenants/{tid}/members/invite
Content-Type: application/json
```

**Request body:**

```json
{
  "email": "newuser@example.com",
  "role": "member"
}
```

**Response `201`:** Thông tin lời mời đã gửi.

---

### 14.3 Xóa thành viên khỏi tenant

```
DELETE /tenants/{tid}/members/{uid}
```

**Response `204`:** Không có body.

---

## 15. Schemas & JSON Examples chi tiết

### Schema: ErrorResponse

Mọi lỗi HTTP đều trả về cùng cấu trúc:

```json
{
  "error": {
    "code": "not-found",
    "message": "resource not found"
  }
}
```

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `error.code` | string | Mã lỗi dạng kebab-case |
| `error.message` | string | Mô tả lỗi (tiếng Anh, dùng để log) |

---

### Schema: ChatSseEvent

Mỗi SSE event được gửi theo format:

```
data: <JSON>\n\n
```

Có **5 loại event**, phân biệt qua field `type`:

#### Loại 1: `text` — Fragment nội dung câu trả lời

```json
{
  "type": "text",
  "content": "Đây là một đoạn nội dung..."
}
```

#### Loại 2: `citation` — Trích dẫn nguồn từ tài liệu *(có page metadata từ T84D)*

```json
{
  "type": "citation",
  "index": 1,
  "point_id": "uuid",
  "document_id": "uuid",
  "chunk_index": 3,
  "filename": "bao_cao_q4.pdf",
  "page_start": 12,
  "page_end": 14
}
```

| Field | Kiểu | Mô tả |
|-------|------|-------|
| `index` | number | Số thứ tự citation trong câu trả lời (bắt đầu từ 1) |
| `point_id` | string (UUID) | ID của vector point trong Qdrant |
| `document_id` | string (UUID) | ID tài liệu gốc |
| `chunk_index` | number | Vị trí chunk trong tài liệu (0-based) |
| `filename` | string \| null | Tên file tài liệu gốc, `null` nếu không tra được |
| `page_start` | number \| null | Trang bắt đầu của chunk (1-based), `null` nếu không xác định |
| `page_end` | number \| null | Trang kết thúc của chunk (1-based), `null` nếu không xác định |

#### Loại 3: `citation_unknown` — Citation không tra được nguồn

```json
{
  "type": "citation_unknown",
  "index": 2
}
```

Hiển thị dưới dạng citation mờ/không có link, không panic.

#### Loại 4: `done` — Kết thúc stream

```json
{
  "type": "done",
  "finish_reason": "stop"
}
```

Các giá trị `finish_reason` có thể gặp: `"stop"`, `"length"`, `"content_filter"`.

#### Loại 5: `error` — Lỗi trong quá trình stream

```json
{
  "type": "error",
  "code": "stream-failed",
  "message": "Upstream LLM timeout"
}
```

> Đây **không phải** HTTP error — kết nối HTTP vẫn `200` nhưng stream báo lỗi qua event này.

---

### Schema: WorkspaceGraphResponse

```json
{
  "nodes": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "kind": "Entity",
      "label": "Nguyễn Văn A",
      "properties": {
        "description": "Giám đốc điều hành"
      },
      "created_at": "2026-06-23T10:00:00Z"
    }
  ],
  "edges": [
    {
      "id": "660e8400-e29b-41d4-a716-446655440000",
      "src_node_id": "550e8400-e29b-41d4-a716-446655440000",
      "dst_node_id": "770e8400-e29b-41d4-a716-446655440000",
      "kind": "MANAGES",
      "weight": 0.95,
      "properties": {},
      "created_at": "2026-06-23T10:00:00Z"
    }
  ],
  "next_cursor": "2026-06-23T11:00:00Z:550e8400-e29b-41d4-a716-446655440000"
}
```

---

### Schema: ChatMessagesResponse

```json
{
  "messages": [
    {
      "id": "uuid-1",
      "role": "user",
      "content": "Doanh thu Q4 là bao nhiêu?",
      "token_count": null,
      "created_at": "2026-06-23T10:00:00Z"
    },
    {
      "id": "uuid-2",
      "role": "assistant",
      "content": "Theo báo cáo Q4, doanh thu đạt 125 tỷ đồng [chunk:1], tăng 25% so với Q3 [chunk:2].",
      "token_count": null,
      "created_at": "2026-06-23T10:00:05Z"
    }
  ]
}
```

> **Lưu ý:** Các tag `[chunk:N]` trong `content` của `assistant` là artifact từ LLM. Backend **đã** chuyển đổi sang SSE `citation` events trong lúc streaming. Khi render từ lịch sử (GET messages), frontend nên strip hoặc render các tag này thành footnote citation tương ứng dựa theo context hiển thị.

---

## 16. Hướng dẫn xử lý SSE Stream

### Tổng quan luồng hoạt động

```
Frontend                          Backend
   |                                  |
   |-- POST /chat_sessions/{sid}/chat -->|
   |   { "message": "Câu hỏi..." }    |
   |                                  |-- Gọi LLM với history + RAG
   |<-- HTTP 200, text/event-stream --|
   |<-- data: {"type":"text","content":"Đây "} --|
   |<-- data: {"type":"text","content":"là "} --|
   |<-- data: {"type":"citation","index":1,...} --|
   |<-- data: {"type":"text","content":"câu trả lời."} --|
   |<-- data: {"type":"done","finish_reason":"stop"} --|
```

### Cơ chế `[chunk:N]` tag — Frontend KHÔNG cần xử lý

Backend (`DeepseekTokenParser`) tự động:
1. Nhận stream token từ LLM
2. Phát hiện tag `[chunk:1]`, `[chunk:2]`, v.v. trong luồng text
3. Tra cứu metadata từ danh sách `chunks` (indexed từ 1)
4. Phát SSE event `citation` với đầy đủ thông tin
5. Strip tag `[chunk:N]` khỏi luồng `text` event

Frontend **chỉ** nhận sự kiện đã được parse sẵn — không cần regex hay parser riêng.

### Ví dụ implementation với EventSource

```javascript
async function sendMessage(tid, sid, message, onText, onCitation, onDone, onError) {
  const response = await fetch(`/tenants/${tid}/chat_sessions/${sid}/chat`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${token}`,
      'X-Tenant-ID': tid,
      'Accept': 'text/event-stream',
    },
    body: JSON.stringify({ message }),
  });

  if (!response.ok) {
    // Lỗi HTTP thật sự (401, 404, 500, v.v.)
    const errorData = await response.json();
    onError(errorData.error);
    return;
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });

    // Tách từng SSE event (phân cách bởi \n\n)
    const events = buffer.split('\n\n');
    buffer = events.pop(); // Giữ lại fragment cuối chưa hoàn chỉnh

    for (const event of events) {
      if (!event.startsWith('data: ')) continue;

      const jsonStr = event.slice(6).trim();
      if (!jsonStr) continue;

      let parsed;
      try {
        parsed = JSON.parse(jsonStr);
      } catch {
        console.warn('SSE parse error:', jsonStr);
        continue;
      }

      switch (parsed.type) {
        case 'text':
          onText(parsed.content);
          break;

        case 'citation':
          onCitation({
            index: parsed.index,
            documentId: parsed.document_id,
            filename: parsed.filename,        // có thể null
            chunkIndex: parsed.chunk_index,
            pageStart: parsed.page_start,  // có thể null
            pageEnd: parsed.page_end,      // có thể null
          });
          break;

        case 'citation_unknown':
          // Hiển thị citation mờ không có link
          onCitation({ index: parsed.index, unknown: true });
          break;

        case 'done':
          onDone(parsed.finish_reason);
          return;

        case 'error':
          onError({ code: parsed.code, message: parsed.message });
          return;
      }
    }
  }
}
```

### Gợi ý render Citation trong UI

```javascript
function renderCitation(citation) {
  if (citation.unknown) {
    return `<span class="citation citation--unknown">[?]</span>`;
  }

  const filename = citation.filename ?? '—';  // filename có thể là null
  const pageInfo = citation.pageStart
    ? `tr. ${citation.pageStart}${citation.pageEnd !== citation.pageStart ? `–${citation.pageEnd}` : ''}`
    : '';

  return `
    <a class="citation" href="/documents/${citation.documentId}/preview#chunk-${citation.chunkIndex}">
      [${citation.index}] ${filename} ${pageInfo}
    </a>
  `;
}
```

---

## 17. Error Handling

### Bảng HTTP Status Codes

| Status Code | Ý nghĩa | Hành động đề xuất |
|-------------|---------|-------------------|
| `200` | Thành công | Xử lý response bình thường |
| `201` | Tạo mới thành công | Lấy `id` từ response body |
| `204` | Xóa/cập nhật thành công | Không có body, cập nhật UI local |
| `400` | Request không hợp lệ | Hiển thị thông báo lỗi cụ thể từ `error.message` |
| `401` | Token không hợp lệ hoặc hết hạn | Refresh token, nếu thất bại → đăng xuất |
| `404` | Không tìm thấy hoặc bị từ chối quyền | Hiển thị "Không tìm thấy" (xem lưu ý ReBAC) |
| `429` | Vượt quota | Hiển thị thông báo vượt giới hạn, hướng dẫn liên hệ admin |
| `500` | Lỗi server nội bộ | Hiển thị thông báo lỗi chung, log chi tiết để debug |

### Lỗi ReBAC: 404 thay vì 403

Hệ thống dùng **`404` thay cho `403`** khi người dùng bị từ chối quyền. Đây là thiết kế **cố ý** nhằm tránh rò rỉ thông tin về sự tồn tại của resource.

**Các tình huống trả 404 do bị từ chối quyền:**

| Tình huống | Endpoint bị ảnh hưởng |
|-----------|----------------------|
| Không phải member của workspace | `GET /workspaces/{wid}/graph` |
| Không có quan hệ `viewer` trên chat session | `GET /chat_sessions/{sid}/messages`, `POST /chat_sessions/{sid}/chat` |

**Lưu ý đặc biệt với Graph API:**
- Node bị ẩn do ACL **không** làm API trả `404`
- Node bị ẩn đơn giản là **không xuất hiện** trong `nodes` array
- Vẫn tiếp tục phân trang bình thường bằng `next_cursor`

### Template xử lý lỗi chuẩn

```javascript
async function apiRequest(url, options = {}) {
  const response = await fetch(url, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${getToken()}`,
      'X-Tenant-ID': getCurrentTenantId(),
      ...options.headers,
    },
  });

  if (response.status === 204) {
    return null; // No content
  }

  const data = await response.json();

  if (!response.ok) {
    const error = data?.error || { code: 'unknown', message: 'Lỗi không xác định' };

    switch (response.status) {
      case 400:
        throw new ValidationError(error.message);
      case 401:
        await refreshToken();
        throw new AuthError(error.message);
      case 404:
        throw new NotFoundError(error.message);
      case 429:
        throw new QuotaExceededError(error.message);
      default:
        throw new ServerError(error.message);
    }
  }

  return data;
}
```

---

## 18. Lưu ý quan trọng cho T85 Frontend

### ✅ Checklist tích hợp

#### Authentication
- [ ] Tất cả request đều có `Authorization: Bearer <token>`
- [ ] Tất cả request tenant-scoped đều có `X-Tenant-ID` khớp với `{tid}` trong URL
- [ ] Có logic auto-refresh token khi nhận `401`

#### Upload tài liệu
- [ ] Dùng `FormData`, **không** set `Content-Type` header thủ công
- [ ] Hiển thị progress bar (dùng `XMLHttpRequest` nếu cần track upload progress)
- [ ] Validate kích thước file phía client trước khi upload (< 50 MiB)
- [ ] Xử lý `429` khi vượt quota storage/document

#### Chat & SSE
- [ ] Chỉ gửi `{ "message": "..." }` — **không** gửi history trong body
- [ ] Parse SSE event theo từng loại `type`
- [ ] Xử lý `citation_unknown` không crash (hiển thị fallback)
- [ ] `page_start` và `page_end` có thể là `null` — handle gracefully
- [ ] Xử lý event `error` trong stream (không phải HTTP error)
- [ ] Dừng đọc stream khi nhận event `done`

#### Lịch sử chat
- [ ] Dùng `GET /messages` để load lại history khi user mở session cũ
- [ ] Messages được sắp xếp `created_at ASC` — render từ cũ đến mới
- [ ] `token_count` luôn là `null` ở phase hiện tại — không hiển thị

#### Graph phân trang
- [ ] Tiếp tục phân trang khi `next_cursor !== null` — **không dừng** khi `nodes.length < limit`
- [ ] Encode cursor khi đưa vào URL: `encodeURIComponent(cursor)`
- [ ] Cursor là chuỗi opaque — **không parse** nội dung bên trong

#### Error handling
- [ ] Coi `404` là "không tìm thấy hoặc không có quyền" — không phân biệt
- [ ] Hiển thị thông báo thân thiện cho user khi `429`
- [ ] Log đầy đủ `error.code` và `error.message` để debug

---

### 🔢 Giới hạn & Hằng số quan trọng

| Hằng số | Giá trị | Mô tả |
|---------|---------|-------|
| File upload tối đa | **50 MiB** | Giới hạn cứng phía server |
| Graph limit mặc định | **200** | Số node/edge mỗi trang |
| Graph limit tối đa | **500** | Giới hạn cứng |
| Chat history inject | **10 tin nhắn gần nhất** | Backend tự xử lý, không cần gửi |

---

### 📋 Tóm tắt endpoint mới từ T84D

| Endpoint | Phase | Mục đích |
|----------|-------|---------|
| `GET /chat_sessions/{sid}/messages` | Phase 3 | Load lại lịch sử khi reload trang |
| SSE `citation.page_start/page_end` | Phase 3.1 | Hiển thị số trang trong citation |
| `GET /workspaces/{wid}/graph?cursor=...` | Phase 4 | Phân trang cursor cho graph lớn |

---

*Tài liệu này được tạo tự động từ codebase GMRAG 2.0 tại T84D. Mọi thắc mắc liên hệ team Backend.*
