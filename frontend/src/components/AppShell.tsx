"use client";

import { useSession, signIn, signOut } from "next-auth/react";

import { useTranslations } from "next-intl";

import { Sidebar } from "@/components/Sidebar";
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { TenantSwitcher } from "@/components/TenantSwitcher";
import { Button } from "@/components/ui/button";

interface AppShellProps {
  tenantId: string;
  workspaceId?: string;
  children: React.ReactNode;
}

export function AppShell({ tenantId, workspaceId, children }: AppShellProps) {
  const t = useTranslations();
  const { data: session } = useSession();

  return (
    <div className="flex min-h-screen bg-background text-foreground">
      <Sidebar tenantId={tenantId} workspaceId={workspaceId} />
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
          <div className="flex items-center gap-3">
            <TenantSwitcher />
          </div>
          <div className="flex items-center gap-3">
            <LanguageSwitcher />
            {session ? (
              <Button
                variant="outline"
                onClick={() => void signOut()}
                className="text-sm"
              >
                {t("auth.signOut")}
              </Button>
            ) : (
              <Button onClick={() => void signIn("keycloak")} className="text-sm">
                {t("auth.signInWithKeycloak")}
              </Button>
            )}
          </div>
        </header>
        <main className="min-w-0 flex-1 p-4">{children}</main>
      </div>
    </div>
  );
}
