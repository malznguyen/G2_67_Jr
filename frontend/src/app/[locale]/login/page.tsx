"use client";

import { useTranslations } from "next-intl";
import { signIn, useSession } from "next-auth/react";
import { useEffect } from "react";

import { Button } from "@/components/ui/button";
import { useRouter } from "@/i18n/navigation";

export default function LoginPage() {
  const t = useTranslations("auth");
  const { data: session, status } = useSession();
  const router = useRouter();

  useEffect(() => {
    if (status === "authenticated") {
      // Authenticated users land on the tenant root (active tenant bootstrapped later).
      router.replace("/");
    }
  }, [status, router]);

  if (status === "loading") {
    return (
      <main className="flex min-h-screen items-center justify-center bg-background text-foreground">
        <p className="text-muted-foreground">{t("unauthenticated")}</p>
      </main>
    );
  }

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-4 bg-background text-foreground">
      <h1 className="text-3xl font-semibold tracking-tight">{t("signInTitle")}</h1>
      <p className="text-muted-foreground">{t("signInPrompt")}</p>
      <Button onClick={() => void signIn("keycloak")}>
        {t("signInWithKeycloak")}
      </Button>
    </main>
  );
}
