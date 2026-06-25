"use client";

import { useEffect } from "react";
import { useSession } from "next-auth/react";

import { useTenantStore } from "@/lib/store/tenant";

/**
 * Calls `GET /users/me` once the session is available and fills the tenant
 * store, auto-selecting the first tenant when none is persisted yet.
 */
export function TenantBootstrapper() {
  const { status } = useSession();
  const bootstrap = useTenantStore((s) => s.bootstrap);
  const bootstrapStatus = useTenantStore((s) => s.bootstrapStatus);

  useEffect(() => {
    if (status === "authenticated" && bootstrapStatus === "idle") {
      void bootstrap();
    }
  }, [status, bootstrapStatus, bootstrap]);

  return null;
}
