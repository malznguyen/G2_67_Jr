"use client";

import { useTranslations } from "next-intl";
import { useEffect } from "react";

import { useRouter } from "@/i18n/navigation";

export default function WorkspaceIndexPage({
  params,
}: {
  params: { locale: string; tid: string; wid: string };
}) {
  const t = useTranslations("documents");
  const router = useRouter();
  const { tid, wid } = params;

  useEffect(() => {
    router.replace(`/t/${tid}/w/${wid}/documents`);
  }, [router, tid, wid]);

  return (
    <main className="flex min-h-[60vh] items-center justify-center text-muted-foreground">
      {t("title")}
    </main>
  );
}
