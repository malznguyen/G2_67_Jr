"use client";

import { useTranslations } from "next-intl";

export default function TenantIndexPage() {
  const t = useTranslations("workspace");
  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-xl font-semibold tracking-tight text-foreground">
        GMRAG2
      </h2>
      <p className="text-muted-foreground">{t("select")}</p>
    </div>
  );
}
