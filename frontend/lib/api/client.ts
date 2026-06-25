import createClient, { type Middleware } from "openapi-fetch";

import type { paths } from "./schema";
import { getClientToken } from "./auth-token.client";
import { getActiveTenantId } from "./tenant-resolver";
import { parseApiError } from "./errors";

const tenantHeader = process.env.NEXT_PUBLIC_TENANT_HEADER ?? "X-Tenant-ID";

/** Attaches `Authorization: Bearer` and `X-Tenant-ID` to every request. */
const authHeadersMiddleware: Middleware = {
  async onRequest({ request }) {
    const token = getClientToken();
    if (token) {
      request.headers.set("Authorization", `Bearer ${token}`);
    }
    const tenantId = getActiveTenantId();
    if (tenantId) {
      request.headers.set(tenantHeader, tenantId);
    }
    return request;
  },
};

/** Throws an `ApiError` for non-2xx responses, parsing the error envelope. */
const errorMiddleware: Middleware = {
  async onResponse({ response }) {
    if (response.ok) return undefined;
    throw await parseApiError(response);
  },
};

export const client = createClient<paths>({
  baseUrl: process.env.NEXT_PUBLIC_API_BASE_URL ?? "",
  headers: { Accept: "application/json" },
});

client.use(authHeadersMiddleware);
client.use(errorMiddleware);

export { ApiError } from "./errors";
