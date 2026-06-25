/**
 * Client-side access token holder.
 *
 * `setClientToken` is called from <Providers> once the NextAuth session is
 * available on the client; the openapi-fetch middleware reads it via
 * {@link getClientToken}. Keeps `@/auth` (server-only) out of the client bundle.
 */
let clientToken: string | null = null;

export function setClientToken(token: string | null): void {
  clientToken = token;
}

export function getClientToken(): string | null {
  return clientToken;
}
