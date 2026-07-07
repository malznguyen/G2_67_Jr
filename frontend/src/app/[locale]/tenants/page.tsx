"use client";

import { useEffect, useMemo, useState } from "react";
import { useTranslations } from "next-intl";
import { useSession, signOut } from "next-auth/react";
import { useQueryClient } from "@tanstack/react-query";

import {
  ArrowLeft,
  Building2,
  Check,
  ChevronRight,
  Crown,
  Loader2,
  LogOut,
  Network,
  Plus,
  RefreshCw,
  ServerCrash,
  ShieldAlert,
  ShieldCheck,
  User as UserIcon,
  WifiOff,
} from "lucide-react";

import { client, ApiError } from "@/lib/api/client";
import { useMe, meKeys, type TenantMembership } from "@/lib/hooks/useMe";
import { useTenantStore } from "@/lib/store/tenant";
import { useRouter } from "@/i18n/navigation";
import { GraphLattice } from "@/components/graph-lattice";

type FetchErrorKind = "network" | "unauthorized" | "forbidden" | "server" | "unknown";

function classifyError(err: ApiError | null): FetchErrorKind {
  if (!err) return "unknown";
  // Network failures (fetch rejected, no response) surface with status 0.
  if (err.status === 0) return "network";
  if (err.status === 401) return "unauthorized";
  if (err.status === 403) return "forbidden";
  if (err.status >= 500) return "server";
  return "unknown";
}

/* ------------------------------------------------------------------ */
/* Role badge                                                          */
/* ------------------------------------------------------------------ */

const ROLE_STYLE: Record<
  TenantMembership["role"],
  { icon: typeof Crown; badge: string; label: string }
> = {
  owner: {
    icon: Crown,
    badge:
      "border-[hsl(38,82%,55%,0.4)] bg-[hsl(38,82%,55%,0.14)] text-[hsl(38,90%,72%)]",
    label: "owner",
  },
  admin: {
    icon: ShieldCheck,
    badge:
      "border-[hsl(178,60%,45%,0.4)] bg-[hsl(178,60%,45%,0.14)] text-[hsl(178,72%,68%)]",
    label: "admin",
  },
  member: {
    icon: UserIcon,
    badge:
      "border-[hsl(215,16%,40%,0.5)] bg-[hsl(215,16%,30%,0.35)] text-[hsl(215,16%,78%)]",
    label: "member",
  },
};

function RoleBadge({ role }: { role: TenantMembership["role"] }) {
  const t = useTranslations("tenants");
  const spec = ROLE_STYLE[role];
  const Icon = spec.icon;
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs font-medium ${spec.badge}`}
    >
      <Icon className="h-3.5 w-3.5" aria-hidden="true" />
      {t(`roles.${spec.label}` as never)}
    </span>
  );
}

/* ------------------------------------------------------------------ */
/* Create tenant form                                                  */
/* ------------------------------------------------------------------ */

function CreateTenantForm({
  onSuccess,
  emphasize,
}: {
  onSuccess: (created: TenantMembership) => void;
  emphasize?: boolean;
}) {
  const t = useTranslations("tenants.create");
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError(t("errorNameRequired"));
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      const { data, error: apiErr } = await client.POST("/tenants", {
        body: { name: trimmed },
      });
      if (apiErr || !data) {
        const e = (apiErr as unknown as ApiError) ?? null;
        if (e?.code === "tenant-name-taken" || e?.status === 409) {
          setError(t("errorTaken"));
        } else {
          setError(t("errorGeneric"));
        }
        return;
      }
      onSuccess(data as TenantMembership);
    } catch {
      setError(t("errorGeneric"));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3" noValidate>
      <label className="flex flex-col gap-1.5 text-sm text-[hsl(210,30%,86%)]">
        <span className="font-medium">{t("label")}</span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("placeholder")}
          autoFocus={emphasize}
          maxLength={120}
          className="h-11 w-full rounded-lg border border-[hsl(200,18%,32%)] bg-[hsl(222,22%,9%)] px-3.5 text-sm text-[hsl(210,30%,94%)] placeholder:text-[hsl(215,16%,48%)] transition focus:border-[hsl(178,60%,50%)] focus:outline-none focus:ring-2 focus:ring-[#5fdcd0]/70"
        />
      </label>
      {error ? (
        <p className="text-xs text-[hsl(0,72%,68%)]" role="alert">
          {error}
        </p>
      ) : null}
      <div className="flex items-center gap-2 pt-1">
        <button
          type="submit"
          disabled={submitting}
          className="inline-flex h-11 items-center justify-center gap-2 rounded-lg bg-[hsl(178,60%,45%)] px-5 text-sm font-semibold text-[hsl(222,40%,8%)] transition hover:bg-[hsl(178,64%,50%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0] focus-visible:ring-offset-2 focus-visible:ring-offset-transparent disabled:cursor-not-allowed disabled:opacity-70"
        >
          {submitting ? (
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
          ) : (
            <Plus className="h-4 w-4" aria-hidden="true" />
          )}
          {submitting ? t("submitting") : t("submit")}
        </button>
      </div>
    </form>
  );
}

/* ------------------------------------------------------------------ */
/* Page                                                                */
/* ------------------------------------------------------------------ */

export default function TenantsPage() {
  const t = useTranslations("tenants");
  const tAuth = useTranslations("auth");
  const { data: session, status } = useSession();
  const router = useRouter();
  const queryClient = useQueryClient();

  const { data, isPending, error, refetch, isFetching } = useMe({
    enabled: status === "authenticated",
  });
  const tenants = data?.tenants ?? [];
  const activeTenantId = useTenantStore((s) => s.activeTenantId);
  const setTenants = useTenantStore((s) => s.setTenants);
  const setActiveTenantId = useTenantStore((s) => s.setActiveTenantId);

  const [navigatingId, setNavigatingId] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);

  // Unauthenticated → bounce to login.
  useEffect(() => {
    if (status === "unauthenticated") {
      router.replace("/login");
    }
  }, [status, router]);

  // Keep the shared tenant store in sync with the picker's authoritative view.
  useEffect(() => {
    if (data?.tenants) {
      setTenants(data.tenants);
    }
  }, [data?.tenants, setTenants]);

  const errorKind = useMemo(() => classifyError((error as ApiError | null) ?? null), [error]);

  const isEmpty = !isPending && !error && tenants.length === 0;

  function selectTenant(tenant: TenantMembership) {
    setActiveTenantId(tenant.id);
    setNavigatingId(tenant.id);
    router.replace(`/t/${tenant.id}`);
  }

  function handleCreated(created: TenantMembership) {
    // The store's UserTenantMembership has the identical shape to the create
    // response, so we can extend the list in place and select the new tenant.
    setTenants([...tenants, created]);
    setActiveTenantId(created.id);
    void queryClient.invalidateQueries({ queryKey: meKeys.me });
    setNavigatingId(created.id);
    router.replace(`/t/${created.id}`);
  }

  const email = session?.user?.email ?? data?.user?.email ?? "";

  return (
    <main className="auth-canvas relative min-h-screen overflow-hidden p-5">
      <GraphLattice className="opacity-50" />

      <div className="relative z-10 mx-auto flex min-h-screen w-full max-w-2xl flex-col py-10 sm:py-16">
        {/* Header */}
        <header className="animate-fade-up flex flex-col gap-2">
          <span
            className="auth-accent-text font-mono text-xs uppercase tracking-[0.42em]"
            aria-hidden="true"
          >
            {t("kicker")}
          </span>
          <h1 className="text-balance text-3xl font-semibold tracking-tight text-[hsl(210,30%,94%)] sm:text-4xl">
            {t("title")}
          </h1>
          <p className="text-balance text-sm leading-relaxed text-[hsl(215,16%,68%)]">
            {t("subtitle")}
          </p>
        </header>

        {/* Account bar */}
        <div className="mt-6 flex flex-wrap items-center justify-between gap-2 text-xs text-[hsl(215,16%,62%)]">
          <span className="inline-flex items-center gap-2">
            <UserIcon className="h-3.5 w-3.5 auth-accent-text" aria-hidden="true" />
            {email ? t("signedInAs", { email }) : null}
          </span>
          <button
            type="button"
            onClick={() => void signOut({ callbackUrl: "/login" })}
            className="inline-flex items-center gap-1.5 rounded-md border border-[hsl(200,18%,32%)] px-2.5 py-1.5 text-[hsl(215,16%,72%)] transition hover:border-[hsl(178,60%,40%)] hover:text-[hsl(178,72%,72%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0]/70"
          >
            <LogOut className="h-3.5 w-3.5" aria-hidden="true" />
            {tAuth("signOut")}
          </button>
        </div>

        <div className="auth-divider my-6" role="presentation" />

        {/* Body — states */}
        <div className="flex flex-1 flex-col">
          {status === "loading" || isPending ? (
            <LoadingState label={t("loading")} hint={t("skeleton")} />
          ) : error ? (
            <ErrorState
              kind={errorKind}
              onRetry={() => void refetch()}
              onSignIn={() => void signOut({ callbackUrl: "/login" })}
              isFetching={isFetching}
            />
          ) : isEmpty ? (
            <EmptyState onCreate={handleCreated} emphasize />
          ) : (
            <PopulatedState
              tenants={tenants}
              activeTenantId={activeTenantId}
              navigatingId={navigatingId}
              onSelect={selectTenant}
              showCreate={showCreate}
              onToggleCreate={() => setShowCreate((v) => !v)}
              onCreated={handleCreated}
            />
          )}
        </div>

        <footer className="mt-10 text-center text-xs text-[hsl(215,16%,50%)]">
          <span className="font-mono tracking-[0.18em]" aria-hidden="true">GraphRAG · Enterprise</span>
        </footer>
      </div>
    </main>
  );
}

/* ------------------------------------------------------------------ */
/* States                                                              */
/* ------------------------------------------------------------------ */

function LoadingState({ label, hint }: { label: string; hint: string }) {
  return (
    <div className="animate-fade-up flex flex-col gap-3" aria-busy="true" aria-live="polite">
      <span className="sr-only">{label}</span>
      {[0, 1, 2].map((i) => (
        <div
          key={i}
          className="flex h-[72px] items-center gap-4 rounded-xl border border-[hsl(200,18%,26%)] bg-[hsl(222,22%,11%,0.5)] p-4"
        >
          <div className="h-10 w-10 shrink-0 animate-pulse rounded-lg bg-[hsl(200,18%,26%,0.6)]" />
          <div className="flex-1 space-y-2.5">
            <div className="h-3.5 w-1/3 animate-pulse rounded bg-[hsl(200,18%,26%,0.6)]" />
            <div className="h-3 w-1/2 animate-pulse rounded bg-[hsl(200,18%,24%,0.5)]" />
          </div>
          <div className="h-6 w-20 animate-pulse rounded-md bg-[hsl(200,18%,26%,0.5)]" />
        </div>
      ))}
      <p className="pt-2 text-center text-xs text-[hsl(215,16%,58%)]">{hint}</p>
    </div>
  );
}

const ERROR_META: Record<
  FetchErrorKind,
  { icon: typeof WifiOff; title: string; body: string; action?: "signIn" }
> = {
  network: { icon: WifiOff, title: "networkTitle", body: "networkBody" },
  unauthorized: { icon: ShieldAlert, title: "unauthorizedTitle", body: "unauthorizedBody", action: "signIn" },
  forbidden: { icon: ShieldAlert, title: "forbiddenTitle", body: "forbiddenBody" },
  server: { icon: ServerCrash, title: "serverTitle", body: "serverBody" },
  unknown: { icon: ShieldAlert, title: "unknownTitle", body: "unknownBody" },
};

function ErrorState({
  kind,
  onRetry,
  onSignIn,
  isFetching,
}: {
  kind: FetchErrorKind;
  onRetry: () => void;
  onSignIn: () => void;
  isFetching: boolean;
}) {
  const t = useTranslations("tenants.errors");
  const tCommon = useTranslations("common");
  const meta = ERROR_META[kind];
  const Icon = meta.icon;
  return (
    <div
      role="alert"
      className="animate-fade-up flex flex-col items-center gap-4 rounded-xl border border-[hsl(0,62%,40%,0.4)] bg-[hsl(0,62%,16%,0.18)] p-8 text-center"
    >
      <Icon className="h-9 w-9 text-[hsl(0,72%,64%)]" aria-hidden="true" />
      <div className="space-y-1.5">
        <h2 className="text-lg font-semibold text-[hsl(0,0%,94%)]">{t(meta.title as never)}</h2>
        <p className="mx-auto max-w-md text-sm leading-relaxed text-[hsl(215,16%,70%)]">
          {t(meta.body as never)}
        </p>
      </div>
      <div className="flex items-center gap-2 pt-1">
        {meta.action === "signIn" ? null : (
          <button
            type="button"
            onClick={onRetry}
            disabled={isFetching}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-lg border border-[hsl(200,18%,32%)] px-4 text-sm font-medium text-[hsl(210,30%,90%)] transition hover:border-[hsl(178,60%,45%)] hover:text-[hsl(178,72%,72%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0]/70 disabled:opacity-60"
          >
            <RefreshCw className={`h-4 w-4 ${isFetching ? "animate-spin" : ""}`} aria-hidden="true" />
            {tCommon("retry")}
          </button>
        )}
        {meta.action === "signIn" ? (
          <button
            type="button"
            onClick={onSignIn}
            className="inline-flex h-10 items-center justify-center gap-2 rounded-lg bg-[hsl(178,60%,45%)] px-4 text-sm font-semibold text-[hsl(222,40%,8%)] transition hover:bg-[hsl(178,64%,50%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0] focus-visible:ring-offset-2 focus-visible:ring-offset-transparent"
          >
            <ArrowLeft className="h-4 w-4" aria-hidden="true" />
            {t("unauthorizedAction")}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function EmptyState({
  onCreate,
  emphasize,
}: {
  onCreate: (t: TenantMembership) => void;
  emphasize?: boolean;
}) {
  const t = useTranslations("tenants.create");
  return (
    <div className="animate-fade-up flex flex-col items-center gap-6 rounded-xl border border-[hsl(200,18%,28%)] bg-[hsl(222,22%,11%,0.5)] p-8 text-center">
      <div
        className="flex h-14 w-14 items-center justify-center rounded-2xl border border-[hsl(178,60%,40%,0.4)] bg-[hsl(178,60%,45%,0.12)]"
        aria-hidden="true"
      >
        <Network className="h-7 w-7 auth-accent-text" />
      </div>
      <div className="space-y-2">
        <h2 className="text-xl font-semibold text-[hsl(210,30%,96%)]">{t("heading")}</h2>
        <p className="mx-auto max-w-md text-sm leading-relaxed text-[hsl(215,16%,70%)]">
          {t("description")}
        </p>
      </div>
      <div className="w-full max-w-md text-left">
        <CreateTenantForm onSuccess={onCreate} emphasize={emphasize} />
      </div>
    </div>
  );
}

function PopulatedState({
  tenants,
  activeTenantId,
  navigatingId,
  onSelect,
  showCreate,
  onToggleCreate,
  onCreated,
}: {
  tenants: TenantMembership[];
  activeTenantId: string | null;
  navigatingId: string | null;
  onSelect: (t: TenantMembership) => void;
  showCreate: boolean;
  onToggleCreate: () => void;
  onCreated: (t: TenantMembership) => void;
}) {
  const t = useTranslations("tenants");
  return (
    <div className="animate-fade-up flex flex-col gap-3">
      {/* List header */}
      <div className="flex items-center justify-between px-2 text-xs font-medium uppercase tracking-wider text-[hsl(215,16%,52%)]">
        <span>{t("listHeader")}</span>
        <span>{t("listHeaderRole")}</span>
      </div>

      {/* Tenant rows */}
      <ul className="flex flex-col gap-2.5" role="list">
        {tenants.map((tenant, idx) => {
          const isActive = tenant.id === activeTenantId;
          const isNavigating = navigatingId === tenant.id;
          return (
            <li key={tenant.id} style={{ animationDelay: `${idx * 60}ms` }} className="animate-fade-up">
              <button
                type="button"
                onClick={() => onSelect(tenant)}
                disabled={isNavigating}
                aria-current={isActive ? "true" : undefined}
                className="group flex w-full items-center gap-4 rounded-xl border border-[hsl(200,18%,28%)] bg-[hsl(222,22%,11%,0.5)] p-4 text-left transition-all hover:border-[hsl(178,60%,45%)] hover:bg-[hsl(222,22%,14%,0.6)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0]/80 disabled:cursor-default"
              >
                <span
                  aria-hidden="true"
                  className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border border-[hsl(200,18%,32%)] bg-[hsl(222,22%,9%)] text-[hsl(178,64%,60%)]"
                >
                  <Building2 className="h-5 w-5" />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-base font-medium text-[hsl(210,30%,96%)]">
                    {tenant.name}
                  </span>
                  <span className="mt-1 block truncate text-xs text-[hsl(215,16%,62%)]">
                    {t(`roleHint.${ROLE_STYLE[tenant.role].label}` as never)}
                  </span>
                </span>
                <RoleBadge role={tenant.role} />
                {isActive ? (
                  <span className="inline-flex shrink-0 items-center gap-1.5 text-xs font-medium text-[hsl(178,72%,68%)]">
                    <Check className="h-4 w-4" aria-hidden="true" />
                    <span className="hidden sm:inline">{t("selected")}</span>
                  </span>
                ) : isNavigating ? (
                  <Loader2 className="h-5 w-5 shrink-0 animate-spin text-[hsl(178,64%,60%)]" aria-hidden="true" />
                ) : (
                  <ChevronRight
                    className="h-5 w-5 shrink-0 text-[hsl(215,16%,42%)] transition group-hover:translate-x-0.5 group-hover:text-[hsl(178,64%,68%)]"
                    aria-hidden="true"
                  />
                )}
              </button>
            </li>
          );
        })}
      </ul>

      {/* Secondary create affordance */}
      <div className="mt-2 flex flex-col gap-3">
        {showCreate ? (
          <div className="rounded-xl border border-[hsl(200,18%,28%)] bg-[hsl(222,22%,11%,0.5)] p-4">
            <CreateTenantForm onSuccess={onCreated} />
            <button
              type="button"
              onClick={onToggleCreate}
              className="mt-3 text-xs text-[hsl(215,16%,60%)] transition hover:text-[hsl(210,30%,86%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0]/60"
            >
              {t("create.cancel")}
            </button>
          </div>
        ) : (
          <button
            type="button"
            onClick={onToggleCreate}
            className="inline-flex h-11 items-center justify-center gap-2 rounded-xl border border-dashed border-[hsl(200,18%,34%)] px-4 text-sm font-medium text-[hsl(215,16%,74%)] transition hover:border-[hsl(178,60%,45%)] hover:text-[hsl(178,72%,72%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0]/70"
          >
            <Plus className="h-4 w-4" aria-hidden="true" />
            {t("create.openForm")}
          </button>
        )}
      </div>

      </div>
  );
}
