import { setRequestLocale } from "next-intl/server";
import { notFound } from "next/navigation";

import { AppShell } from "@/components/AppShell";
import { isLocale } from "@/i18n/config";

export default async function TenantLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string; tid: string }>;
}) {
  const { locale, tid } = await params;
  if (!isLocale(locale)) notFound();
  await setRequestLocale(locale);

  return <AppShell tenantId={tid}>{children}</AppShell>;
}
