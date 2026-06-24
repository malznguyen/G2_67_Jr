# T85+ Implementation Plan

**Audit:** T84C — Frontend Foundation Readiness  
**Date:** 2026-06-23  
**Architecture reference:** [FRONTEND_ARCHITECTURE.md](./FRONTEND_ARCHITECTURE.md)  
**API reference:** [API_INVENTORY.md](./API_INVENTORY.md)

---

## Purpose

This plan turns the T84C architecture audit into **sequenced, assignable work** for T85 and subsequent frontend tasks. Each phase has entry criteria, deliverables, acceptance tests, and explicit out-of-scope items.

**Do not reorder phases** — later phases depend on auth, API client, and shell from earlier phases.

---

## Phase overview

| Phase | Name | Est. effort | Depends on |
|------:|------|-------------|------------|
| 1 | Frontend foundation | 2–3 days | T84A/B complete |
| 2 | Authentication | 2–3 days | Phase 1 |
| 3 | Dashboard shell | 2 days | Phase 2 |
| 4 | Settings | 2–3 days | Phase 3 |
| 5 | ACL | 1 day | Phase 3 (uses existing T84 code) |
| 6 | Documents | 3–4 days | Phase 3 |
| 7 | Chat | 4–5 days | Phase 3, 6 (workspace context) |
| 8 | Usage | 2 days | Phase 3 |

**Total MVP estimate:** 18–23 dev-days (1 engineer)

---

## Phase 1 — Frontend foundation

### Goal

A frontend engineer can `pnpm install && pnpm dev`, regenerate API types, and import typed clients — without making tooling decisions.

### Entry criteria

- [x] Backend running with `/openapi.json` (T84A)
- [x] CORS enabled (T84B)
- [x] `.env.example` has correct `NEXT_PUBLIC_API_BASE_URL`

### Tasks

#### 1.1 Dependencies

```bash
cd frontend
pnpm add @tanstack/react-query @tanstack/react-table zustand zod react-hook-form @hookform/resolvers
pnpm add openapi-fetch
pnpm add -D openapi-typescript
pnpm add @microsoft/fetch-event-source next-themes sonner
pnpm add lucide-react class-variance-authority clsx tailwind-merge
# shadcn init + core components (see FRONTEND_ARCHITECTURE.md)
```

#### 1.2 Package metadata fixes

- Update `package.json` description to "Next.js 16 App Router"
- Align `eslint-config-next` to 16.x (already at 16.2.9)
- Add scripts:

```json
{
  "openapi:generate": "openapi-typescript http://localhost:8088/openapi.json -o src/lib/api/schema.d.ts",
  "typecheck": "tsc --noEmit"
}
```

#### 1.3 Folder scaffold

Create empty structure per [FRONTEND_ARCHITECTURE.md](./FRONTEND_ARCHITECTURE.md#folder-structure):

- `src/lib/api/` — `client.ts`, `errors.ts`, `headers.ts`, `enums.ts`
- `src/lib/query/` — `keys.ts`, `provider.tsx`
- `src/lib/store/` — `app-store.ts`
- `src/features/` — empty feature dirs
- `components/ui/` — shadcn components

Move `frontend/lib/acl.ts` → `src/lib/acl.ts` (or keep path, update tsconfig `@/` alias).

#### 1.4 API client core

Implement:

| File | Responsibility |
|------|----------------|
| `errors.ts` | `parseApiError(res)`, `ApiError` class (mirror `AclError`) |
| `headers.ts` | `buildHeaders({ token, tenantId })` |
| `client.ts` | `createApiClient(getToken, getTenantId)` wrapping openapi-fetch |
| `enums.ts` | Zod schemas: `DocumentStatus`, `DocumentVisibility`, `TenantRole`, `AclRelation`, … |

Regenerate `schema.d.ts` from live OpenAPI.

#### 1.5 Providers in root layout

```tsx
// app/layout.tsx — wrap children
<SessionProvider>          // Phase 2 stub OK initially
  <QueryProvider>
    <ThemeProvider>
      {children}
      <Toaster />
    </ThemeProvider>
  </QueryProvider>
</SessionProvider>
```

#### 1.6 Developer docs

Create `docs/KEYCLOAK_DEV_SETUP.md` (realm, clients, audience mapper, test user).

Optional: commit `docs/openapi.snapshot.json` for offline typegen in CI.

### Deliverables

- [ ] All dependencies installed
- [ ] `pnpm openapi:generate` produces `schema.d.ts`
- [ ] `pnpm typecheck` passes
- [ ] shadcn/ui initialized with button, dialog, input, table, toast
- [ ] Query provider wired
- [ ] Keycloak dev guide written

### Acceptance tests

1. `pnpm dev` — app loads at `:3000`
2. Import type `components["schemas"]["MeResponse"]` in a test file — compiles
3. `apiFetch` against `GET /health` returns 200 (no auth)

### Out of scope

- Login UI
- Tenant-scoped pages
- Real data fetching

---

## Phase 2 — Authentication

### Goal

Users log in via Keycloak, receive a session, and the app can attach Bearer tokens to API calls.

### Entry criteria

- Phase 1 complete
- Keycloak realm configured per dev guide

### Tasks

#### 2.1 Auth.js configuration

| File | Purpose |
|------|---------|
| `src/lib/auth/config.ts` | Keycloak provider, callbacks |
| `app/api/auth/[...nextauth]/route.ts` | Auth.js route handler |
| `src/lib/auth/session.ts` | `getAccessToken()` helper |

**Callbacks:**

- `jwt` — store `access_token`, `refresh_token`, `expires_at`
- `session` — expose `session.accessToken` to client (or use server-only token fetch)

**Audience requirement:** Keycloak client must map `gmrag-backend` into token `aud`. Document verification step in dev guide.

#### 2.2 Middleware

```typescript
// middleware.ts
export const config = { matcher: ["/t/:path*", "/select-tenant"] };
// Redirect to /login if no session
```

Public paths: `/login`, `/api/auth/*`, `/api/health`.

#### 2.3 Login page

- `app/login/page.tsx` — "Sign in with Keycloak" → `signIn("keycloak")`
- Redirect to `/select-tenant` or `/t/{defaultTenant}/dashboard` after login

#### 2.4 Bootstrap hook

```typescript
// hooks/use-bootstrap.ts
// 1. useSession()
// 2. useQuery(queryKeys.me, GET /users/me)
// 3. Sync tenants to validate activeTenantId in Zustand
```

#### 2.5 Select tenant page

- `app/select-tenant/page.tsx` — list tenants from `/users/me`
- On select → `setActiveTenantId` → redirect `/t/{tid}/dashboard`

#### 2.6 API config hook

```typescript
// hooks/use-api-config.ts
// Returns { baseUrl, tenantId, token } or throws if incomplete
```

Used by all feature hooks and `acl.ts`.

### Deliverables

- [ ] Login/logout works against local Keycloak
- [ ] Middleware protects `/t/*`
- [ ] `GET /users/me` succeeds from browser
- [ ] `useApiConfig()` returns valid token + tenant

### Acceptance tests

1. Unauthenticated visit to `/t/xxx/dashboard` → redirect `/login`
2. After login, `/users/me` returns user + tenants in network tab
3. Logout clears session and redirects to `/login`
4. Token refresh does not force re-login within 30 min session

### Out of scope

- Tenant creation UI (can use API/Swagger)
- Role-based UI hiding (Phase 3+)

---

## Phase 3 — Dashboard shell

### Goal

Authenticated users land in a consistent tenant console with navigation, tenant switcher, and owner-only nav items hidden.

### Entry criteria

- Phase 2 complete
- User has at least one tenant membership

### Tasks

#### 3.1 Tenant layout

`app/t/[tid]/layout.tsx`:

1. Validate `tid` matches Zustand `activeTenantId` (or sync)
2. Verify membership via cached `/users/me` data
3. Render `AppShell` with sidebar + header

#### 3.2 App shell components

| Component | Location |
|-----------|----------|
| `AppShell` | `components/layout/app-shell.tsx` |
| `Sidebar` | `components/layout/sidebar.tsx` |
| `Header` | `components/layout/header.tsx` |
| `TenantSwitcher` | `components/layout/tenant-switcher.tsx` |
| `UserMenu` | `components/layout/user-menu.tsx` |

#### 3.3 Navigation map

| Item | Path | Visible when |
|------|------|--------------|
| Dashboard | `/t/[tid]/dashboard` | always |
| Documents | `/t/[tid]/documents` | always |
| Chat | `/t/[tid]/chat` | always |
| Workspaces | `/t/[tid]/workspaces` | always |
| Settings | `/t/[tid]/settings` | `role === "owner"` |
| Usage | `/t/[tid]/usage` | owner |

Use `useIsOwner(tid)` hook reading from `/users/me` cache.

#### 3.4 Dashboard page

- Placeholder cards: tenant name, workspace count, document count (optional queries)
- Quick links to Documents, Chat

#### 3.5 Error boundaries

- `app/t/[tid]/error.tsx` — API 403/404 friendly messages
- `app/t/[tid]/not-found.tsx`

#### 3.6 Workspaces list (minimal)

- `app/t/[tid]/workspaces/page.tsx` — table from `GET .../workspaces`
- Needed as navigation target before Documents phase

### Deliverables

- [ ] Tenant layout with sidebar navigation
- [ ] Tenant switcher changes `tid` in URL + Zustand
- [ ] Owner-only nav items hidden for members
- [ ] Dashboard placeholder page

### Acceptance tests

1. Switch tenant → URL updates, API calls use new `X-Tenant-ID`
2. Member user does not see Settings/Usage nav
3. Invalid `tid` in URL → redirect `/select-tenant`
4. Mobile: sidebar collapses (sheet)

### Out of scope

- Full workspace CRUD forms (inline in Phase 6 if needed)
- Settings, documents, chat content

---

## Phase 4 — Settings

### Goal

Tenant owners configure LLM/BYOK and manage tenant members.

### Entry criteria

- Phase 3 complete
- Test user is tenant **owner**

### Tasks

#### 4.1 Settings layout

`app/t/[tid]/settings/layout.tsx` — tabs: General, LLM, Members

Owner guard: redirect members to dashboard with toast.

#### 4.2 LLM settings (BYOK)

| Item | Detail |
|------|--------|
| API | `GET/PUT /tenants/{tid}/settings/llm` |
| Hook | `useLlmSettings(tid)`, `useUpdateLlmSettings()` |
| Form | `LlmSettingsForm` — provider select (`ollama` \| `openai`), model, base URL, API key (password field), dimensions |
| Display | Show `api_key_masked`, `has_api_key` — never show raw key from GET |

Zod schema mirrors `PutLlmSettingsRequest`.

#### 4.3 Tenant members

| Item | Detail |
|------|--------|
| API | `GET/POST/DELETE .../members` |
| UI | Member table, invite dialog (email + role) |
| Limitation | **No accept-invite flow** — show token in success toast for MVP or skip invite UI |

#### 4.4 Tenant rename (optional)

- `PATCH /tenants/{tid}` — simple name form on General tab

### Deliverables

- [ ] Owner can view/update LLM settings
- [ ] Owner can list/invite/remove members
- [ ] Member users cannot access `/settings` (403 redirect)

### Acceptance tests

1. PUT LLM settings → GET returns updated provider/model
2. Invite member → appears in list (pending status)
3. Non-owner gets redirected from settings routes

### Out of scope

- Invitation accept workflow (no backend endpoint)
- Quota editing (read-only in Phase 8)

---

## Phase 5 — ACL

### Goal

Resource owners can share documents and chat sessions via the existing ReBAC UI.

### Entry criteria

- Phase 3 complete
- Existing `AclShareDialog` + `lib/acl.ts` (T84)

### Tasks

#### 5.1 Refactor ACL module

- Move to `features/acl/`
- Update imports to use `useApiConfig()` instead of manual `AclClientConfig` prop drilling
- Keep `AclClientConfig` interface — populate from hook

#### 5.2 Integrate share button

| Surface | Location | Condition |
|---------|----------|-----------|
| Document detail | `/t/[tid]/documents/[did]` | User is owner (403 on POST = hide button) |
| Chat session | `/t/[tid]/chat/[sid]` | User is owner |

#### 5.3 Share dialog UX polish

- Replace raw HTML selects with shadcn Select
- Toast on grant/revoke success
- Map `AclError` codes to user messages

#### 5.4 React Query integration

```typescript
useGrants(tid, resourceType, resourceId)
useCreateGrant() / useRevokeGrant() with invalidation
```

Can wrap existing `acl.ts` functions.

### Deliverables

- [ ] Share dialog opens from document detail (Phase 6 page)
- [ ] Share dialog opens from chat session header (Phase 7 page)
- [ ] List/create/revoke grants works E2E

### Acceptance tests

1. Owner shares document with user UUID → grant appears in list
2. Revoke grant → disappears
3. Non-owner does not see Share button (or gets 403 inline)

### Out of scope

- Workspace picker for workspace principal (UUID text input OK for MVP)
- Bulk sharing

---

## Phase 6 — Documents

### Goal

Users list, upload, preview, and delete documents with processing status polling.

### Entry criteria

- Phase 3 complete (workspace list available)
- At least one workspace exists

### Tasks

#### 6.1 Documents list page

- `GET /tenants/{tid}/documents?workspace_id=`
- Workspace filter dropdown
- `DataTable`: title, status badge, visibility, created_at
- **Polling:** refetch every 5s while any doc has `status === "processing"` or `"uploaded"`

#### 6.2 Upload flow

| Item | Detail |
|------|--------|
| API | `POST .../documents` multipart |
| Component | `UploadDropzone` — drag/drop, max 50 MiB |
| Fields | `file`, `visibility`, `workspace_id`, optional `title` |
| Errors | Handle `429 quota-exceeded` with toast |

Implement in `lib/api/upload.ts` (not openapi-fetch).

#### 6.3 Document detail / preview

- `GET .../documents/{did}/preview`
- Show metadata + chunk list (max 50 chunks — show notice)
- Delete button (owner only → 403 handling)
- Share button → `AclShareDialog`

#### 6.4 Status badges

| Status | Color |
|--------|-------|
| `uploaded` | gray |
| `processing` | yellow + spinner |
| `indexed` | green |
| `failed` | red |

Use zod enum for runtime validation.

#### 6.5 Empty states

- No documents → CTA to upload
- No workspace selected → prompt to pick workspace

### Deliverables

- [ ] List documents with workspace filter
- [ ] Upload document → appears in list with processing status
- [ ] Preview page with chunks
- [ ] Delete document (owner)
- [ ] Status polling until terminal state

### Acceptance tests

1. Upload PDF → status transitions uploaded → processing → indexed (poll)
2. Preview shows chunk content
3. Delete removes from list
4. Quota exceeded shows user-friendly error

### Out of scope

- Pagination / infinite scroll
- Full-text search
- Batch upload

---

## Phase 7 — Chat

### Goal

Users create chat sessions, send messages, and receive streamed RAG responses with citations.

### Entry criteria

- Phase 3 complete
- Phase 6 recommended (workspace-scoped retrieval)
- LLM configured (Phase 4) or platform default works

### Tasks

#### 7.1 Session list

- `GET/POST/DELETE .../chat_sessions`
- Create session dialog: title, optional workspace, optional model
- Session list sorted by `updated_at DESC` (server order)

#### 7.2 Chat page layout

`app/t/[tid]/chat/[sid]/page.tsx`:

- Header: session title, workspace badge, share button (Phase 5)
- Message list area
- Input bar with send button

#### 7.3 SSE stream implementation

| File | Detail |
|------|--------|
| `lib/api/sse.ts` | `postChatStream(config, sid, message, callbacks, signal)` |
| `hooks/use-chat-stream.ts` | State machine: idle → streaming → done/error |

Use `@microsoft/fetch-event-source` — **not** `EventSource`.

Parse discriminated union:

```typescript
type ChatSseEvent =
  | { type: "text"; content: string }
  | { type: "citation"; index: number; point_id: string; document_id: string; chunk_index: number; filename?: string }
  | { type: "citation_unknown"; index: number }
  | { type: "done"; finish_reason?: string }
  | { type: "error"; code: string; message: string };
```

#### 7.4 Message UI

- User bubble (immediate on send)
- Assistant bubble (streaming text append)
- `CitationChip` — clickable, links to document preview if accessible
- Loading indicator during stream

#### 7.5 History limitation

**No `GET .../messages` endpoint.** MVP behavior:

- Persist messages in component state only for current session visit
- On page reload → empty history (show banner: "Previous messages are not loaded yet")
- Accumulate during SSE in session

#### 7.6 Error handling

- Pre-stream HTTP error → toast with `error.code`
- In-stream `{ type: "error" }` → inline error bubble + retry button
- Abort on navigate away

### Deliverables

- [ ] Create/list/delete chat sessions
- [ ] Send message → streamed response with citations
- [ ] Cancel in-flight stream on unmount
- [ ] Share session via ACL dialog

### Acceptance tests

1. Create session → send "hello" → receive streamed text
2. RAG query returns at least one citation chip (with indexed docs in workspace)
3. `persist-failed` SSE error shows user message, partial text preserved
4. Delete session removes from list

### Out of scope

- Message history reload (blocked on backend R2)
- Multi-model picker UX (optional field only)
- Graph context visualization in chat

---

## Phase 8 — Usage

### Goal

Tenant owners view usage metrics, quota limits, and audit logs.

### Entry criteria

- Phase 3 complete
- Test user is tenant **owner**

### Tasks

#### 8.1 Usage page layout

`app/t/[tid]/usage/page.tsx` — tabs: Overview, Audit log

Owner guard same as Settings.

#### 8.2 Quota display

- `GET .../quotas` → `QuotaBar` components for documents, workspaces, storage, members
- Compare against usage totals where available

#### 8.3 Usage charts

- `GET .../metering/usage` → `{ metric, total }[]`
- `UsageChart` — recharts bar chart
- Common metrics: document count, storage bytes, chat completions (per backend metric names)

#### 8.4 Audit log table

- `GET .../audit_logs` — max 100 rows (server cap)
- Banner: "Showing latest 100 entries"
- Columns: timestamp, actor, action, resource_type, resource_id
- Expand row for `metadata` JSON

#### 8.5 React Query hooks

```typescript
useQuotas(tid)
useUsage(tid)
useAuditLogs(tid)
```

### Deliverables

- [ ] Quota bars with utilization percentages
- [ ] Usage bar chart
- [ ] Audit log table with metadata expand
- [ ] Member cannot access page

### Acceptance tests

1. Owner sees quota values matching API
2. Usage chart renders without error for empty tenant
3. Audit log shows recent actions after document upload/delete
4. Member redirected from `/usage`

### Out of scope

- Audit log pagination/filters (no backend params)
- Export CSV
- Billing integration

---

## Cross-phase quality gates

Run before marking any phase complete:

| Check | Command |
|-------|---------|
| Typecheck | `pnpm typecheck` |
| Lint | `pnpm lint` |
| Build | `pnpm build` |
| API types fresh | `pnpm openapi:generate` + no git diff (CI) |

## Post-MVP backlog (not T85)

| Item | Backend dep | Priority |
|------|-------------|----------|
| Chat message history | `GET .../messages` | P1 |
| Keycloak realm import script | Infra | P1 |
| Cursor pagination on lists | Backend | P2 |
| Invitation accept flow | Backend | P2 |
| E2E tests (Playwright) | T78/T79 | P2 |
| Graph visualization page | — | P2 |
| Virtual scroll for large lists | Pagination | P3 |

---

## Workbook task mapping

The PM workbook defines T70–T77 as an alternate numbering. Map as follows:

| Workbook | This plan |
|----------|-----------|
| T70 Auth | Phase 2 |
| T71 apiFetch | Phase 1.4 |
| T72 TenantContext | Phase 2.5 + 3.1 |
| T73 Workspace layout | Phase 3 + 6 |
| T74 Client wrappers | Phase 1.4 + per-feature hooks |
| T75 ACL mount + upload | Phase 5 + 6 |
| T76 ChatPanel SSE | Phase 7 |
| T77 Graph/quota/settings | Phase 4 + 8 |

---

## Definition of done (entire T85 track)

When Phases 1–8 are complete:

- [ ] Owner can log in, select tenant, configure LLM, invite members
- [ ] User can upload documents, wait for indexing, preview chunks
- [ ] User can chat with RAG streaming and citations
- [ ] Owner can share documents/sessions via ACL
- [ ] Owner can view usage/quota/audit
- [ ] All API calls use generated types + standard error handling
- [ ] No architectural decisions left undocumented

**FRONTEND FOUNDATION READY** was established in T84C. **FRONTEND MVP READY** is achieved when this plan is complete.

---

*Plan authored: T84C. Execute starting T85 Phase 1.*
