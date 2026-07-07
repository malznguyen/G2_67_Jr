"use client";

import { useQuery } from "@tanstack/react-query";

import { client, ApiError } from "@/lib/api/client";
import type { components } from "@/lib/api/schema";

export type MeResponse = components["schemas"]["MeResponse"];
export type UserProfile = components["schemas"]["UserProfile"];
export type TenantMembership = components["schemas"]["UserTenantMembership"];

export const meKeys = {
  me: ["me"] as const,
};

/**
 * Fetches the authenticated user's profile + tenant memberships via the typed
 * API client. The error is normalized to an {@link ApiError} so callers can
 * branch on `status` (401 vs 5xx vs network) for tailored messaging.
 *
 * `retry: false` so 401/403 don't hammer the backend; the picker surfaces a
 * "Sign in again" affordance instead.
 */
export function useMe(options?: { enabled?: boolean }) {
  return useQuery<MeResponse, ApiError>({
    queryKey: meKeys.me,
    enabled: options?.enabled ?? true,
    retry: false,
    staleTime: 30 * 1000,
    async queryFn(): Promise<MeResponse> {
      const { data, error } = await client.GET("/users/me");
      if (error || !data) {
        throw (error as unknown as ApiError) ?? new Error("no data");
      }
      return data;
    },
  });
}