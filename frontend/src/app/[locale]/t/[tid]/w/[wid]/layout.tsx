import { setRequestLocale } from "next-intl/server";
import { notFound } from "next/navigation";

import { AppShell } from "@/components/AppShell";
import { isLocale } from "@/i18n/config";

export default async function WorkspaceLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string; tid: string; wid: string }>;
}) {
  const { locale, tid, wid } = await params;
  if (!isLocale(locale)) notFound();
  await setRequestLocale(locale);

  return (
    <AppShell tenantId={tid} workspaceId={wid}>
      {children}
    </AppShell>
  );
}
