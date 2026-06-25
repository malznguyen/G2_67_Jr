"use client";

import { useTranslations } from "next-intl";

import { Link, usePathname } from "@/i18n/navigation";

interface SidebarProps {
  tenantId: string;
  workspaceId?: string;
}

export function Sidebar({ tenantId, workspaceId }: SidebarProps) {
  const t = useTranslations("nav");
  const pathname = usePathname();

  const base = workspaceId
    ? { tenantId, workspaceId, path: `/t/${tenantId}/w/${workspaceId}` }
    : null;

  const items: Array<{ key: "documents" | "chat" | "graph" | "settings"; href: string }> = base
    ? [
        { key: "documents", href: `${base.path}/documents` },
        { key: "chat", href: `${base.path}/chat` },
        { key: "graph", href: `${base.path}/graph` },
        { key: "settings", href: `${base.path}/settings` },
      ]
    : [
        { key: "documents", href: `/t/${tenantId}` },
        { key: "settings", href: `/t/${tenantId}/settings` },
      ];

  return (
    <nav
      aria-label={t("documents")}
      className="flex h-full w-56 shrink-0 flex-col gap-1 border-r border-border bg-background p-3"
    >
      <span className="mb-2 px-2 text-sm font-semibold tracking-tight text-foreground">
        GMRAG2
      </span>
      {items.map((item) => {
        const active = pathname === item.href || pathname.startsWith(`${item.href}/`);
        return (
          <Link
            key={item.key}
            href={item.href}
            className={`rounded-md px-3 py-2 text-sm transition-colors ${
              active
                ? "bg-secondary text-secondary-foreground"
                : "text-muted-foreground hover:bg-secondary/60 hover:text-foreground"
            }`}
          >
            {t(item.key)}
          </Link>
        );
      })}
    </nav>
  );
}
