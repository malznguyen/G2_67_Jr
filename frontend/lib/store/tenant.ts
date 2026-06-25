"use client";

import { create } from "zustand";
import { persist } from "zustand/middleware";

import { client } from "@/lib/api/client";
import { setActiveTenantResolver } from "@/lib/api/tenant-resolver";

export interface TenantMembership {
  id: string;
  name: string;
  role: "owner" | "member";
}

interface TenantState {
  tenants: TenantMembership[];
  activeTenantId: string | null;
  bootstrapStatus: "idle" | "loading" | "done" | "error";
  bootstrapError: string | null;
  setTenants: (tenants: TenantMembership[]) => void;
  setActiveTenantId: (id: string | null) => void;
  bootstrap: () => Promise<void>;
}

export const useTenantStore = create<TenantState>()(
  persist(
    (set, get) => ({
      tenants: [],
      activeTenantId: null,
      bootstrapStatus: "idle",
      bootstrapError: null,
      setTenants: (tenants) => set({ tenants }),
      setActiveTenantId: (activeTenantId) => set({ activeTenantId }),
      async bootstrap() {
        if (get().bootstrapStatus === "loading") return;
        set({ bootstrapStatus: "loading", bootstrapError: null });
        try {
          const { data, error } = await client.GET("/users/me");
          if (error || !data) {
            throw new Error(
              error ? `bootstrap failed: ${error.error.code}` : "no data",
            );
          }
          const tenants = data.tenants ?? [];
          set({ tenants, bootstrapStatus: "done" });
          if (!get().activeTenantId && tenants.length > 0) {
            set({ activeTenantId: tenants[0].id });
          }
        } catch (err) {
          set({
            bootstrapStatus: "error",
            bootstrapError: err instanceof Error ? err.message : "unknown",
          });
        }
      },
    }),
    {
      name: "gmrag:activeTenantId",
      partialize: (state) => ({ activeTenantId: state.activeTenantId }),
    },
  ),
);

// Wire the store into the openapi-fetch tenant resolver so the API client
// automatically sends the active tenant as `X-Tenant-ID`.
setActiveTenantResolver(() => useTenantStore.getState().activeTenantId);
