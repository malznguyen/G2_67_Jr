// ACL (ReBAC) API client (T84) — wires the frontend to the backend
// `routes/acl.rs` grant endpoints (T67). Mirrors the Zanzibar-style relation
// model: a grant shares a resource (`document` | `chat_session`) with a
// subject (`user` | `workspace` group) at a relation (`viewer` | `editor`).
//
// Framework-agnostic: every call takes an explicit `AclClientConfig` carrying
// the API base URL, the active tenant id (sent as `X-Tenant-Id`), and the
// caller's bearer token, so this module has no hidden global state.

export type PrincipalType = "user" | "workspace";

/** Relations a caller may grant. `owner` is implicit and never grantable. */
export type GrantableRelation = "viewer" | "editor";

/** Namespaces that support sharing. */
export type ShareableResource = "document" | "chat_session";

export interface Grant {
  id: string;
  principal_type: PrincipalType;
  principal_id: string;
  /** The backend may also return the implicit `owner` relation when listing. */
  relation: GrantableRelation | "owner";
  created_at: string;
}

export interface AclClientConfig {
  /** API origin/prefix, e.g. "http://localhost:8000" or "/api". */
  baseUrl: string;
  /** Active tenant id, sent as the `X-Tenant-Id` header. */
  tenantId: string;
  /** OIDC access token, sent as `Authorization: Bearer`. */
  token: string;
}

export interface CreateGrantInput {
  resourceType: ShareableResource;
  resourceId: string;
  principalType: PrincipalType;
  principalId: string;
  relation: GrantableRelation;
}

/** Error carrying the backend's stable `{ error: { code, message } }` envelope. */
export class AclError extends Error {
  readonly code: string;
  readonly status: number;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "AclError";
    this.status = status;
    this.code = code;
  }
}

function authHeaders(cfg: AclClientConfig): HeadersInit {
  return {
    Authorization: `Bearer ${cfg.token}`,
    "X-Tenant-Id": cfg.tenantId,
    "Content-Type": "application/json",
  };
}

async function ensureOk(res: Response): Promise<void> {
  if (res.ok) return;
  let code = "request-failed";
  let message = `${res.status} ${res.statusText}`;
  try {
    const body = (await res.json()) as { error?: { code?: string; message?: string } };
    if (body.error?.code) code = body.error.code;
    if (body.error?.message) message = body.error.message;
  } catch {
    // Non-JSON error body; keep the status-derived message.
  }
  throw new AclError(res.status, code, message);
}

/** `GET /tenants/{tid}/acl` — list the grants on a resource. */
export async function listGrants(
  cfg: AclClientConfig,
  resourceType: ShareableResource,
  resourceId: string,
): Promise<Grant[]> {
  const url = new URL(`${cfg.baseUrl}/tenants/${cfg.tenantId}/acl`);
  url.searchParams.set("resource_type", resourceType);
  url.searchParams.set("resource_id", resourceId);
  const res = await fetch(url.toString(), {
    method: "GET",
    headers: authHeaders(cfg),
  });
  await ensureOk(res);
  const body = (await res.json()) as { grants: Grant[] };
  return body.grants ?? [];
}

/** `POST /tenants/{tid}/acl` — share a resource with a subject. */
export async function createGrant(
  cfg: AclClientConfig,
  input: CreateGrantInput,
): Promise<Grant> {
  const res = await fetch(`${cfg.baseUrl}/tenants/${cfg.tenantId}/acl`, {
    method: "POST",
    headers: authHeaders(cfg),
    body: JSON.stringify({
      resource_type: input.resourceType,
      resource_id: input.resourceId,
      principal_type: input.principalType,
      principal_id: input.principalId,
      relation: input.relation,
    }),
  });
  await ensureOk(res);
  return (await res.json()) as Grant;
}

/** `DELETE /tenants/{tid}/acl/{grant_id}` — revoke a grant. */
export async function revokeGrant(cfg: AclClientConfig, grantId: string): Promise<void> {
  const res = await fetch(`${cfg.baseUrl}/tenants/${cfg.tenantId}/acl/${grantId}`, {
    method: "DELETE",
    headers: authHeaders(cfg),
  });
  await ensureOk(res);
}

/** Convenience factory binding the config once. */
export function aclClient(cfg: AclClientConfig) {
  return {
    list: (resourceType: ShareableResource, resourceId: string) =>
      listGrants(cfg, resourceType, resourceId),
    create: (input: CreateGrantInput) => createGrant(cfg, input),
    revoke: (grantId: string) => revokeGrant(cfg, grantId),
  };
}
