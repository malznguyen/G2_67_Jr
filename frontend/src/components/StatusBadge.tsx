"use client";

import { useTranslations } from "next-intl";

import { Badge, type BadgeProps } from "@/components/ui/badge";
const STATUS_VARIANT: Partial<Record<string, BadgeProps["variant"]>> = {
  uploaded: "muted",
  processing: "warning",
  indexed: "success",
  failed: "danger",
};

export interface StatusBadgeProps {
  status: string;
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
