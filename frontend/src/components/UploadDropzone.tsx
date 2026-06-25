"use client";

import { useTranslations } from "next-intl";
import { useCallback, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { uploadDocument, MAX_UPLOAD_BYTES, UploadValidationError } from "@/lib/api/upload";
import { documentsKeys } from "@/lib/hooks/useDocuments";
import { ApiError } from "@/lib/api/client";
import { Button } from "@/components/ui/button";

export interface UploadDropzoneProps {
  tid: string;
  workspaceId: string;
  onUploaded?: (id: string) => void;
}

export function UploadDropzone({ tid, workspaceId, onUploaded }: UploadDropzoneProps) {
  const t = useTranslations();
  const tErrors = useTranslations("errors");
  const inputRef = useRef<HTMLInputElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const inputId = useId();
  const queryClient = useQueryClient();

  const startUpload = useCallback(
    async (file: File) => {
      setError(null);
      if (file.type !== "application/pdf") {
        setError(tErrors("fileType"));
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        setError(tErrors("fileTooLarge"));
        return;
      }
      setBusy(true);
      try {
        const { id } = await uploadDocument({
          tid,
          workspaceId,
          file,
          visibility: "shared",
        });
        await queryClient.invalidateQueries({
          queryKey: documentsKeys.list(tid, workspaceId),
        });
        onUploaded?.(id);
      } catch (e) {
        if (e instanceof UploadValidationError) {
          setError(tErrors("fileTooLarge"));
        } else if (e instanceof ApiError) {
          setError(`${tErrors("uploadFailed")} (${e.code})`);
        } else {
          setError(tErrors("uploadFailed"));
        }
      } finally {
        setBusy(false);
      }
    },
    [tid, workspaceId, onUploaded, queryClient, tErrors],
  );

  function onInputChange(event: React.ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (file) void startUpload(file);
    event.target.value = "";
  }

  function onDrop(event: React.DragEvent<HTMLDivElement>) {
    event.preventDefault();
    setDragging(false);
    const file = event.dataTransfer.files?.[0];
    if (file) void startUpload(file);
  }

  return (
    <div className="flex flex-col gap-2">
      <div
        role="button"
        tabIndex={0}
        aria-labelledby={inputId}
        onClick={() => inputRef.current?.click()}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            inputRef.current?.click();
          }
        }}
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={onDrop}
        className={`flex cursor-pointer flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed p-8 text-center transition-colors ${
          dragging
            ? "border-primary bg-accent"
            : "border-border bg-muted/40 hover:border-primary/60 hover:bg-accent/50"
        }`}
      >
        <span id={inputId} className="text-sm text-muted-foreground">
          {t("documents.dropHere")}
        </span>
        <Button type="button" variant="outline" disabled={busy} size="sm">
          {busy ? t("common.loading") : t("documents.upload")}
        </Button>
        <input
          ref={inputRef}
          type="file"
          accept="application/pdf"
          className="hidden"
          onChange={onInputChange}
        />
      </div>
      {error && (
        <p className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
