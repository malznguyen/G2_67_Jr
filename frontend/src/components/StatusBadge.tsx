"use client";

import { useTranslations } from "next-intl";

import { Badge, type BadgeProps } from "@/components/ui/badge";
import type { components_schemas } from "@/lib/api/schema";

type DocumentStatus = components_schemas["DocumentStatus"];

const STATUS_VARIANT: Record<DocumentStatus, BadgeProps["variant"]> = {
  uploaded: "muted",
  processing: "warning",
  indexed: "success",
  failed: "danger",
};

export interface StatusBadgeProps {
  status: DocumentStatus;
  className?: string;
}

export function StatusBadge({ status, className }: StatusBadgeProps) {
  const t = useTranslations("documents.status");
  return (
    <Badge variant={STATUS_VARIANT[status] ?? "muted"} className={className}>
      {t(status)}
    </Badge>
  );
}
