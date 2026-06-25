"use client";

import { useQuery } from "@tanstack/react-query";

import { client } from "@/lib/api/client";

export const tenantsKeys = {
  all: ["tenants"] as const,
};

export function useTenants() {
  return useQuery({
    queryKey: tenantsKeys.all,
    async queryFn() {
      const { data, error } = await client.GET("/tenants");
      if (error || !data) {
        throw error ?? new Error("fetch tenants: no data");
      }
      return data.tenants ?? [];
    },
    staleTime: 60 * 1000,
  });
}
