"use client";

import { useTenantStore } from "@/lib/store/tenant";
import { Select } from "@/components/ui/select";

/** Tenant picker; only renders when the user belongs to more than one tenant. */
export function TenantSwitcher() {
  const tenants = useTenantStore((s) => s.tenants);
  const activeTenantId = useTenantStore((s) => s.activeTenantId);
  const setActiveTenantId = useTenantStore((s) => s.setActiveTenantId);

  if (tenants.length <= 1) return null;

  return (
    <label className="flex items-center gap-2 text-sm text-muted-foreground">
      <span>Tenant</span>
      <Select
        value={activeTenantId ?? ""}
        onChange={(e) => setActiveTenantId(e.target.value || null)}
      >
        {tenants.map((t) => (
          <option key={t.id} value={t.id}>
            {t.name} ({t.role})
          </option>
        ))}
      </Select>
    </label>
  );
}
