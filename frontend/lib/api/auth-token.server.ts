import "server-only";

import { auth } from "@/auth";

/**
 * Server-side access token getter. Returns the Keycloak access token from the
 * NextAuth session, or `null` when the caller is unauthenticated.
 *
 * Importing this module pulls in `@/auth` (server-only); do NOT import it from
 * client components — use {@link setClientToken} instead.
 */
export async function getServerToken(): Promise<string | null> {
  const session = await auth();
  return session?.access_token ?? null;
}
