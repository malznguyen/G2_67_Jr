import NextAuth, { type DefaultSession } from "next-auth";

declare module "next-auth" {
  interface Session {
    access_token?: string;
    user: DefaultSession["user"] & {
      id?: string;
    };
  }
}

// Server-side issuer (container-internal hostname, e.g. http://keycloak:8080).
// Used to fetch the OIDC discovery document, JWKS, and to exchange the
// authorization code for tokens from the Next.js server (which runs inside the
// container network and cannot resolve the host's `localhost`).
const serverIssuer =
  process.env.KEYCLOAK_ISSUER ??
  `${process.env.NEXT_PUBLIC_KEYCLOAK_URL}/realms/${process.env.NEXT_PUBLIC_KEYCLOAK_REALM}`;

// Public issuer (host-side URL, e.g. http://localhost:8080). The browser must
// be redirected to this origin so the user can reach Keycloak from the host.
const publicIssuer =
  process.env.KEYCLOAK_ISSUER_PUBLIC ??
  `${process.env.NEXT_PUBLIC_KEYCLOAK_URL}/realms/${process.env.NEXT_PUBLIC_KEYCLOAK_REALM}`;

export const { handlers, signIn, signOut, auth } = NextAuth({
  trustHost: true,
  secret: process.env.AUTH_SECRET,
  providers: [
    {
      id: "keycloak",
      name: "Keycloak",
      type: "oidc",
      // `iss` claim in the ID token is emitted by Keycloak using the
      // host-side origin (http://localhost:8080/realms/gmrag) because the
      // browser performs the authorization request via localhost. NextAuth
      // validates the `iss` response parameter against this value, so it must
      // be the PUBLIC issuer — not the container-internal one.
      issuer: publicIssuer,
      // Discovery is fetched server-side from the container-internal origin
      // (the Next.js server cannot resolve the host's `localhost`).
      wellKnown: `${serverIssuer}/.well-known/openid-configuration`,
      clientId: process.env.NEXT_PUBLIC_KEYCLOAK_CLIENT_ID,
      clientSecret: process.env.KEYCLOAK_FRONTEND_CLIENT_SECRET ?? "",
      authorization: {
        // Browser-facing authorization endpoint (host-side origin).
        url: `${publicIssuer}/protocol/openid-connect/auth`,
        params: { scope: "openid email profile" },
      },
      // Server-side token exchange (container-internal).
      token: {
        url: `${serverIssuer}/protocol/openid-connect/token`,
      },
      idToken: true,
    },
  ],
  callbacks: {
    async jwt({ token, account }) {
      if (account?.access_token) {
        token.access_token = account.access_token;
      }
      return token;
    },
    async session({ session, token }) {
      if (token.access_token) {
        session.access_token = token.access_token as string;
      }
      return session;
    },
  },
});
