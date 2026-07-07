"use client";

import { useTranslations } from "next-intl";
import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useSession } from "next-auth/react";

import { useDocuments, documentsKeys } from "@/lib/hooks/useDocuments";
import { client, ApiError } from "@/lib/api/client";
import { UploadDropzone } from "@/components/UploadDropzone";
import { StatusBadge } from "@/components/StatusBadge";
import { Button } from "@/components/ui/button";
import { AclShareDialog } from "@/components/AclShareDialog";
import type { components } from "@/lib/api/schema";

type DocumentItem = components["schemas"]["DocumentsResponse"]["documents"][number];

interface ShareTarget {
  doc: DocumentItem;
}

export default function DocumentsPage({
  params,
}: {
  params: Promise<{ locale: string; tid: string; wid: string }>;
}) {
  // Client component reading async params via React.use() in 16; but to keep
  // things simple we resolve tenant/workspace from props synchronously through
  // a wrapper. Next 16 supports async params only in server components.
  // We accept the promise and unwrap with a small client-safe read below.
  return <DocumentsView params={params} />;
}

function DocumentsView({
  params,
}: {
  params: Promise<{ locale: string; tid: string; wid: string }>;
}) {
  const resolved = useResolvedParams(params);
  const { tid, wid } = resolved;

  const t = useTranslations("documents");
  const tCommon = useTranslations("common");
  const tErrors = useTranslations("errors");
  const { data: session } = useSession();
  const queryClient = useQueryClient();
  const [shareTarget, setShareTarget] = useState<ShareTarget | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const { data, isLoading, isError, error } = useDocuments(tid, wid);
  const documents = data?.documents ?? [];

  const deleteMutation = useMutation({
    async mutationFn(docId: string) {
      const { error: err } = await client.DELETE("/tenants/{tid}/documents/{did}", {
        params: {
          path: { tid, did: docId },
          header: { "X-Tenant-ID": tid },
        },
      });
      if (err) throw err;
    },
    onSuccess() {
      void queryClient.invalidateQueries({
        queryKey: documentsKeys.list(tid, wid),
      });
      setConfirmDeleteId(null);
    },
    onError(e) {
      void e;
    },
  });

  const aclConfig = useMemo(() => {
    const token = session?.access_token ?? "";
    return {
      baseUrl: process.env.NEXT_PUBLIC_API_BASE_URL ?? "",
      tenantId: tid,
      token,
    };
  }, [session?.access_token, tid]);

  const hasToken = Boolean(session?.access_token);

  return (
    <section className="flex flex-col gap-4">
      <header className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">
          {t("title")}
        </h1>
      </header>

      <UploadDropzone tid={tid} workspaceId={wid} />

      {isLoading ? (
        <p className="text-sm text-muted-foreground">…</p>
      ) : isError ? (
        <p className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error instanceof ApiError ? `${tErrors("loadFailed")} (${error.code})` : tErrors("loadFailed")}
        </p>
      ) : documents.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("empty")}</p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="min-w-full text-sm">
            <thead className="bg-muted/50 text-left text-muted-foreground">
              <tr>
                <th className="px-3 py-2 font-medium">{t("columns.title")}</th>
                <th className="px-3 py-2 font-medium">{t("columns.visibility")}</th>
                <th className="px-3 py-2 font-medium">{t("columns.status")}</th>
                <th className="px-3 py-2 font-medium">{t("columns.createdAt")}</th>
                <th className="px-3 py-2 font-medium text-right">{t("columns.actions")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {documents.map((doc) => (
                <tr key={doc.id} className="hover:bg-muted/30">
                  <td className="px-3 py-2 text-foreground">{doc.title}</td>
                  <td className="px-3 py-2">
                    <span className="text-muted-foreground">
                      {t(`visibility.${doc.visibility}`)}
                    </span>
                  </td>
                  <td className="px-3 py-2">
                    <StatusBadge status={doc.status} />
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">
                    {new Date(doc.created_at).toLocaleString()}
                  </td>
                  <td className="px-3 py-2 text-right">
                    <div className="inline-flex gap-2">
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={!hasToken}
                        onClick={() => setShareTarget({ doc })}
                      >
                        {t("share")}
                      </Button>
                      <Button
                        size="sm"
                        variant="destructive"
                        onClick={() => setConfirmDeleteId(doc.id)}
                      >
                        {t("delete")}
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {shareTarget && hasToken && (
        <AclShareDialog
          open
          onClose={() => setShareTarget(null)}
          config={aclConfig}
          resourceType="document"
          resourceId={shareTarget.doc.id}
          resourceLabel={shareTarget.doc.title}
        />
      )}

      {confirmDeleteId && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
          role="dialog"
          aria-modal="true"
          onClick={() => setConfirmDeleteId(null)}
        >
          <div
            className="w-full max-w-sm rounded-xl bg-background p-6 shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <p className="mb-4 text-sm text-foreground">{t("deleteConfirm")}</p>
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => setConfirmDeleteId(null)}
              >
                {tCommon("cancel")}
              </Button>
              <Button
                variant="destructive"
                size="sm"
                disabled={deleteMutation.isPending}
                onClick={() => void deleteMutation.mutate(confirmDeleteId)}
              >
                {deleteMutation.isPending ? "…" : t("delete")}
              </Button>
            </div>
            {deleteMutation.isError && (
              <p className="mt-3 text-sm text-destructive">{tErrors("deleteFailed")}</p>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

// Resolve async params in a client component. Next 16 exposes `React.use` for
// promises; we re-export a tiny helper to keep the surface stable.
import * as React from "react";

function useResolvedParams(
  params: Promise<{ locale: string; tid: string; wid: string }>,
): { locale: string; tid: string; wid: string } {
  return React.use(params);
}
