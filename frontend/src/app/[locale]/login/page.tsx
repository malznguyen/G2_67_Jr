"use client";

import { Suspense, useEffect, useMemo, useState } from "react";
import { useTranslations } from "next-intl";
import { signIn, useSession } from "next-auth/react";
import { useSearchParams } from "next/navigation";

import { LoaderCircle, LogIn, ShieldCheck, AlertTriangle } from "lucide-react";

import { useRouter } from "@/i18n/navigation";
import { GraphLattice } from "@/components/graph-lattice";

function LoginInner() {
  const t = useTranslations("auth");
  const tErr = useTranslations("auth.errors");
  const { status } = useSession();
  const router = useRouter();
  const searchParams = useSearchParams();
  const [redirecting, setRedirecting] = useState(false);

  // If a session already exists, go straight to the tenant picker.
  useEffect(() => {
    if (status === "authenticated") {
      router.replace("/tenants");
    }
  }, [status, router]);

  const errorCode = searchParams.get("error");
  const errorKey = useMemo(() => {
    if (!errorCode) return "Default";
    return tErr.has(errorCode as never) ? (errorCode as string) : "Default";
  }, [errorCode, tErr]);
  const showError = Boolean(errorCode) && status !== "authenticated";

  const handleSignIn = () => {
    setRedirecting(true);
    void signIn("keycloak", { callbackUrl: "/tenants" });
  };

  const initializing = status === "loading" || redirecting;

  return (
    <main className="auth-canvas relative flex min-h-screen items-center justify-center overflow-hidden p-5">
      <GraphLattice className="opacity-70" />

      <section
        className="auth-card animate-fade-up relative z-10 w-full max-w-md rounded-xl p-8 sm:p-10"
        aria-labelledby="login-title"
      >
        <header className="flex flex-col gap-3 text-center">
          <span
            className="auth-accent-text font-mono text-sm uppercase tracking-[0.42em]"
            aria-hidden="true"
          >
            {t("signInTitle")}
          </span>
          <h1
            id="login-title"
            className="text-balance text-2xl font-semibold tracking-tight text-[hsl(210,30%,94%)] sm:text-[1.7rem]"
          >
            {t("welcomeBack")}
          </h1>
          <p
            className="text-balance text-sm leading-relaxed text-[hsl(215,16%,68%)]"
          >
            {t("welcomeBody")}
          </p>
        </header>

        {showError ? (
          <div
            role="alert"
            className="animate-fade-up mt-6 flex items-start gap-3 rounded-lg border border-[hsl(0,62%,45%,0.45)] bg-[hsl(0,62%,22%,0.20)] p-3.5 text-sm text-[hsl(0,0%,94%)]"
          >
            <AlertTriangle
              className="mt-0.5 h-4 w-4 shrink-0 text-[hsl(0,70%,62%)]"
              aria-hidden="true"
            />
            <p className="leading-relaxed">{tErr(errorKey as never)}</p>
          </div>
        ) : null}

        <div className="mt-7 flex flex-col gap-3">
          <button
            type="button"
            onClick={handleSignIn}
            disabled={initializing}
            className="inline-flex h-12 w-full items-center justify-center gap-2.5 rounded-lg bg-[hsl(178,60%,45%)] text-sm font-semibold text-[hsl(222,40%,8%)] shadow-[0_1px_0_hsl(0_0%_100%_/_0.2)_inset,0_18px_40px_-16px_hsl(178_70%_40%_/_0.7)] transition-all hover:bg-[hsl(178,64%,50%)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#5fdcd0] focus-visible:ring-offset-2 focus-visible:ring-offset-transparent disabled:cursor-not-allowed disabled:opacity-70"
          >
            {initializing ? (
              <LoaderCircle className="h-5 w-5 animate-spin" aria-hidden="true" />
            ) : (
              <LogIn className="h-5 w-5" aria-hidden="true" />
            )}
            <span>{initializing ? t("redirecting") : t("signInWithKeycloak")}</span>
          </button>

          <p className="flex items-center justify-center gap-2 pt-2 text-center text-xs leading-relaxed text-[hsl(215,16%,62%)]">
            <ShieldCheck className="h-3.5 w-3.5 auth-accent-text" aria-hidden="true" />
            {t("secureNote")}
          </p>
        </div>

        <div className="auth-divider my-6" role="presentation" />

        <footer className="text-center text-xs text-[hsl(215,16%,58%)]">
          <span className="font-mono tracking-[0.18em]" aria-hidden="true">GraphRAG · Enterprise</span>
        </footer>
      </section>
    </main>
  );
}

export default function LoginPage() {
  // `useSearchParams` must sit inside a Suspense boundary in Next 16 so the
  // page can still be statically pre-rendered.
  return (
    <Suspense
      fallback={
        <main className="auth-canvas relative flex min-h-screen items-center justify-center">
          <LoaderCircle
            className="h-6 w-6 animate-spin text-[hsl(178,70%,62%)]"
            aria-hidden="true"
          />
        </main>
      }
    >
      <LoginInner />
    </Suspense>
  );
}