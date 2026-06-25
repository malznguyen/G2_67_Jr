"use client";

import { useEffect, useState } from "react";
import { SessionProvider, useSession } from "next-auth/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { setClientToken } from "@/lib/api/auth-token.client";
import { TenantBootstrapper } from "@/components/tenant-bootstrapper";

function SessionTokenBridge() {
  const { data: session } = useSession();
  useEffect(() => {
    setClientToken(session?.access_token ?? null);
  }, [session?.access_token]);
  return null;
}

export function Providers({ children }: { children: React.ReactNode }) {
  const [queryClient] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: { staleTime: 60 * 1000, retry: 1 },
        },
      }),
  );

  return (
    <SessionProvider>
      <QueryClientProvider client={queryClient}>
        <SessionTokenBridge />
        <TenantBootstrapper />
        {children}
      </QueryClientProvider>
    </SessionProvider>
  );
}
