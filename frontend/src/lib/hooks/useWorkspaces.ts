"use client";

import { useQuery } from "@tanstack/react-query";

import { client } from "@/lib/api/client";
import { useTenantStore } from "@/lib/store/tenant";

export const workspacesKeys = {
  list: (tid: string) => ["tenants", tid, "workspaces"] as const,
};

export function useWorkspaces(tid?: string) {
  const activeTenantId = useTenantStore((s) => s.activeTenantId);
  const resolvedTid = tid ?? activeTenantId ?? undefined;

  return useQuery({
    queryKey: resolvedTid ? workspacesKeys.list(resolvedTid) : ["tenants", "workspaces", "none"],
    enabled: Boolean(resolvedTid),
    async queryFn() {
      if (!resolvedTid) throw new Error("no tenant id");
      const { data, error } = await client.GET("/tenants/{tid}/workspaces", {
        params: {
          path: { tid: resolvedTid },
          header: { "X-Tenant-ID": resolvedTid },
        },
      });
      if (error || !data) {
        throw error ?? new Error("fetch workspaces: no data");
      }
      return data.workspaces ?? [];
    },
    staleTime: 60 * 1000,
  });
}
