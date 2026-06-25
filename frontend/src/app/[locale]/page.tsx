"use client";

import { useEffect } from "react";
import { useSession } from "next-auth/react";

import { useRouter } from "@/i18n/navigation";
import { useTenantStore } from "@/lib/store/tenant";

export default function LocaleIndexPage() {
  const { status } = useSession();
  const router = useRouter();
  const activeTenantId = useTenantStore((s) => s.activeTenantId);
  const tenants = useTenantStore((s) => s.tenants);

  useEffect(() => {
    if (status === "loading") return;
    if (status !== "authenticated") {
      router.replace("/login");
      return;
    }
    const target = activeTenantId ?? tenants[0]?.id;
    if (target) {
      router.replace(`/t/${target}`);
    }
  }, [status, activeTenantId, tenants, router]);

  return (
    <main className="flex min-h-screen items-center justify-center bg-background text-foreground">
      <p className="text-muted-foreground">…</p>
    </main>
  );
}
