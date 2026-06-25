/** Error carrying the backend's stable `{ error: { code, message } }` envelope. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

interface ErrorEnvelope {
  error?: { code?: string; message?: string };
}

/** Parse a non-2xx response body into a typed `ApiError`. */
export async function parseApiError(res: Response): Promise<ApiError> {
  let code = "request-failed";
  let message = `${res.status} ${res.statusText}`;
  try {
    const body = (await res.json()) as ErrorEnvelope;
    if (body.error?.code) code = body.error.code;
    if (body.error?.message) message = body.error.message;
  } catch {
    // Non-JSON body; keep status-derived defaults.
  }
  return new ApiError(res.status, code, message);
}
