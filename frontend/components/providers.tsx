"use client";

import { useEffect, useState } from "react";
import { SessionProvider, useSession } from "next-auth/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { NextIntlClientProvider } from "next-intl";

import { setClientToken } from "@/lib/api/auth-token.client";
import { TenantBootstrapper } from "@/components/tenant-bootstrapper";

function SessionTokenBridge() {
  const { data: session } = useSession();
  useEffect(() => {
    setClientToken(session?.access_token ?? null);
  }, [session?.access_token]);
  return null;
}

export interface ProvidersProps {
  children: React.ReactNode;
  locale: string;
  messages: Record<string, unknown>;
}

export function Providers({ children, locale, messages }: ProvidersProps) {
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
      <NextIntlClientProvider locale={locale} messages={messages}>
        <QueryClientProvider client={queryClient}>
          <SessionTokenBridge />
          <TenantBootstrapper />
          {children}
        </QueryClientProvider>
      </NextIntlClientProvider>
    </SessionProvider>
  );
}
