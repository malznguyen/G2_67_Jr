import { client, ApiError } from "@/lib/api/client";

/** Hard server-side upload limit (FRONTEND_API_CONTRACT.md §4.2). */
export const MAX_UPLOAD_BYTES = 50 * 1024 * 1024; // 50 MiB

export interface UploadDocumentInput {
  tid: string;
  workspaceId: string;
  file: File;
  visibility: "shared" | "private";
  title?: string;
}

export interface UploadedDocument {
  id: string;
}

export class UploadValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "UploadValidationError";
  }
}

/**
 * Upload a document via `multipart/form-data`.
 *
 * IMPORTANT: do NOT set `Content-Type` — the browser sets the multipart
 * boundary automatically when the body is a `FormData` instance.
 */
export async function uploadDocument(
  input: UploadDocumentInput,
): Promise<UploadedDocument> {
  if (input.file.size > MAX_UPLOAD_BYTES) {
    throw new UploadValidationError("File exceeds the 50 MiB limit");
  }

  const formData = new FormData();
  formData.append("file", input.file, input.file.name || "upload");
  formData.append("visibility", input.visibility);
  formData.append("workspace_id", input.workspaceId);
  if (input.title) {
    formData.append("title", input.title);
  }

  const { data, error } = await client.POST("/tenants/{tid}/documents", {
    params: { path: { tid: input.tid } },
    body: formData,
  });

  if (error || !data) {
    throw error ?? new ApiError(0, "upload-failed", "upload failed");
  }
  return { id: data.id };
}
