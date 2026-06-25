"use client";

import { useQuery } from "@tanstack/react-query";

import { client } from "@/lib/api/client";
import { useTenantStore } from "@/lib/store/tenant";
import type { components_schemas } from "@/lib/api/schema";

type DocumentsResponse = components_schemas["DocumentsResponse"];
export type DocumentItem = DocumentsResponse["documents"][number];

export const documentsKeys = {
  list: (tid: string, wid: string) =>
    ["tenants", tid, "workspaces", wid, "documents"] as const,
};

const POLL_INTERVAL = 3000;
const PENDING_STATUSES: ReadonlySet<string> = new Set(["uploaded", "processing"]);

export function useDocuments(tid: string | undefined, wid: string | undefined) {
  const activeTenantId = useTenantStore((s) => s.activeTenantId);
  const resolvedTid = tid ?? activeTenantId ?? undefined;

  return useQuery<DocumentsResponse>({
    queryKey:
      resolvedTid && wid
        ? documentsKeys.list(resolvedTid, wid)
        : ["tenants", "workspaces", "documents", "none"],
    enabled: Boolean(resolvedTid && wid),
    refetchInterval: (query) => {
      const docs = query.state.data?.documents ?? [];
      return docs.some((d) => PENDING_STATUSES.has(d.status)) ? POLL_INTERVAL : false;
    },
    async queryFn(): Promise<DocumentsResponse> {
      if (!resolvedTid || !wid) throw new Error("missing tenant or workspace id");
      const { data, error } = await client.GET("/tenants/{tid}/documents", {
        params: { path: { tid: resolvedTid }, query: { workspace_id: wid } },
      });
      if (error || !data) {
        throw error ?? new Error("fetch documents: no data");
      }
      return data;
    },
    staleTime: 30 * 1000,
  });
}
