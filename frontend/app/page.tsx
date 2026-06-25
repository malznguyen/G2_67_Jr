"use client";

import { useSession, signIn, signOut } from "next-auth/react";
import { Button } from "@/components/ui/button";
import { TenantSwitcher } from "@/components/TenantSwitcher";

export default function HomePage() {
  const { data: session, status } = useSession();

  if (status === "loading") {
    return (
      <main className="flex min-h-screen items-center justify-center bg-background text-foreground">
        <p className="text-muted-foreground">Loading…</p>
      </main>
    );
  }

  if (!session) {
    return (
      <main className="flex min-h-screen flex-col items-center justify-center gap-4 bg-background text-foreground">
        <h1 className="text-3xl font-semibold tracking-tight">GMRAG2</h1>
        <p className="text-muted-foreground">Sign in to continue</p>
        <Button onClick={() => signIn("keycloak")}>
          Sign in with Keycloak
        </Button>
      </main>
    );
  }

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-4 bg-background text-foreground">
      <h1 className="text-3xl font-semibold tracking-tight">GMRAG2</h1>
      <p className="text-muted-foreground">
        Signed in as <span className="font-medium text-foreground">{session.user?.email}</span>
      </p>
      <p className="text-xs text-muted-foreground">
        access_token present: {session.access_token ? "yes" : "no"}
      </p>
      <TenantSwitcher />
      <Button variant="outline" onClick={() => signOut()}>
        Sign out
      </Button>
    </main>
  );
}
