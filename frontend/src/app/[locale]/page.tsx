"use client";

import { useEffect } from "react";
import { useSession } from "next-auth/react";

import { useRouter } from "@/i18n/navigation";

export default function LocaleIndexPage() {
  const { status } = useSession();
  const router = useRouter();

  useEffect(() => {
    if (status === "loading") return;
    if (status !== "authenticated") {
      router.replace("/login");
      return;
    }
    // Authenticated users always go through the tenant picker first; selecting
    // a tenant there navigates onward to `/t/{tid}`.
    router.replace("/tenants");
  }, [status, router]);

  return (
    <main className="flex min-h-screen items-center justify-center bg-background text-foreground">
      <p className="text-muted-foreground">…</p>
    </main>
  );
}